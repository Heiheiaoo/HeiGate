package channel

import (
	"context"
	"fmt"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/Heiheiaoo/heigate/internal/probe"
)

const (
	RoutePriority     = "priority"      // Legacy alias for smart_quality
	RouteSmartQuality = "smart_quality" // Dynamic quality scoring
	RouteLatency      = "latency"       // Lowest latency first
	RouteRoundRobin   = "round_robin"   // Load balancing round robin
)

// Router resolves a model to enabled local channels and provides
// a retry loop for protocol adapters. It owns no HTTP protocol code; callers
// supply the actual forward function for Anthropic/OpenAI/Responses.
type Router struct {
	store *Store
	mu    sync.Mutex
	turn  map[string]int
}

func NewRouter(store *Store) *Router {
	return &Router{store: store, turn: make(map[string]int)}
}

// Candidates returns enabled channels that advertise the requested model.
// Recently failed channels remain visible only when no healthy candidate exists,
// making recovery possible without a separate circuit-breaker table.
func (r *Router) Candidates(ctx context.Context, model, strategy string) ([]*Channel, error) {
	return r.CandidatesMatching(ctx, model, strategy, nil)
}

// CandidatesMatching applies an optional protocol predicate before health and
// priority selection. This matters when a model has a healthy Chat channel
// and a degraded Responses channel: filtering after selection would hide the
// only usable Responses route.
func (r *Router) CandidatesMatching(ctx context.Context, model, strategy string, matches func(*Channel) bool) ([]*Channel, error) {
	if r == nil || r.store == nil {
		return nil, fmt.Errorf("desktop channel router is not configured")
	}
	channels, err := r.store.List(ctx)
	if err != nil {
		return nil, err
	}
	model = strings.TrimSpace(model)
	healthy := make([]*Channel, 0)
	degraded := make([]*Channel, 0)
	failed := make([]*Channel, 0)
	cooldown := make([]*Channel, 0)
	for _, channel := range channels {
		if !channel.Enabled {
			continue
		}
		if model != "" && !supportsModel(channel, model) {
			continue
		}
		if matches != nil && !matches(channel) {
			continue
		}
		if channel.CooldownUntil != nil && time.Now().Before(*channel.CooldownUntil) {
			cooldown = append(cooldown, channel)
			continue
		}
		if channel.CircuitState == CircuitOpen {
			// Lazy half-open recovery: the first request after the cooldown is
			// allowed through as a probe instead of requiring a timer goroutine.
			channel.CircuitState = CircuitHalfOpen
			_ = r.store.Save(context.Background(), channel)
		}
		if channel.CircuitState == CircuitHalfOpen {
			healthy = append(healthy, channel)
			continue
		}
		switch channel.LastStatus {
		case probe.StatusOperational, "healthy", "", "unknown":
			healthy = append(healthy, channel)
		case probe.StatusDegraded:
			degraded = append(degraded, channel)
		default:
			failed = append(failed, channel)
		}
	}
	if len(healthy) == 0 {
		healthy = degraded
	}
	if len(healthy) == 0 {
		healthy = failed
	}
	if len(healthy) == 0 {
		healthy = cooldown
	}
	if len(healthy) == 0 {
		return nil, fmt.Errorf("no enabled channel supports model %q", model)
	}
	r.sortCandidates(healthy, model, strategy)
	return healthy, nil
}

// Route tries each candidate in order. A failed attempt is persisted as an
// error so later requests prefer another provider; a successful attempt clears
// the transient failure and records a fresh latency.
func (r *Router) Route(ctx context.Context, model, strategy string, forward func(context.Context, *Channel) error) (*Channel, error) {
	if forward == nil {
		return nil, fmt.Errorf("desktop forward function is nil")
	}
	candidates, err := r.Candidates(ctx, model, strategy)
	if err != nil {
		return nil, err
	}
	return r.RouteCandidates(ctx, model, candidates, forward)
}

// RouteCandidates applies the same failover bookkeeping as Route to a caller-
// supplied, already filtered candidate list. Desktop protocol adapters use
// this when one model is exposed through multiple wire protocols.
func (r *Router) RouteCandidates(ctx context.Context, model string, candidates []*Channel, forward func(context.Context, *Channel) error) (*Channel, error) {
	if forward == nil {
		return nil, fmt.Errorf("desktop forward function is nil")
	}
	if len(candidates) == 0 {
		return nil, fmt.Errorf("no enabled channel supports model %q", model)
	}
	var lastErr error
	for _, channel := range candidates {
		started := time.Now()
		err := forward(ctx, channel)
		latency := int(time.Since(started) / time.Millisecond)
		if err == nil {
			channel.LastStatus = probe.StatusOperational
			channel.LastError = ""
			channel.FailureCount = 0
			channel.CircuitState = CircuitClosed
			channel.CooldownUntil = nil
			channel.LastLatencyMs = &latency
			channel.LastCheckedAt = timePtr(time.Now().UTC())
			_ = r.store.Save(context.Background(), channel)
			return channel, nil
		}
		lastErr = err
		failureInfo := FailureFromError(err)
		channel.FailureCount++
		channel.LastStatus = probe.StatusError
		channel.LastError = FailureLabel(failureInfo) + ": " + err.Error()
		cooldown := FailureCooldown(failureInfo, channel.FailureCount)
		until := time.Now().UTC().Add(cooldown)
		channel.CooldownUntil = &until
		if FailureIsCircuitEligible(failureInfo) && channel.FailureCount >= 3 {
			channel.CircuitState = CircuitOpen
		} else if channel.CircuitState == "" {
			channel.CircuitState = CircuitClosed
		}
		channel.LastLatencyMs = &latency
		channel.LastCheckedAt = timePtr(time.Now().UTC())
		_ = r.store.Save(context.Background(), channel)
	}
	return nil, fmt.Errorf("all desktop channels failed for model %q: %w", model, lastErr)
}

func (r *Router) sortCandidates(channels []*Channel, model, strategy string) {
	sort.SliceStable(channels, func(i, j int) bool {
		switch strategy {
		case RouteLatency:
			return latencyOf(channels[i]) < latencyOf(channels[j])
		case RouteRoundRobin:
			return latencyOf(channels[i]) < latencyOf(channels[j])
		case RouteSmartQuality, RoutePriority:
			fallthrough
		default:
			return qualityScore(channels[i]) > qualityScore(channels[j])
		}
	})
	if strategy == RouteRoundRobin && len(channels) > 1 {
		r.mu.Lock()
		start := r.turn[model] % len(channels)
		r.turn[model]++
		r.mu.Unlock()
		rotated := append(append([]*Channel{}, channels[start:]...), channels[:start]...)
		copy(channels, rotated)
	}
}

func qualityScore(channel *Channel) int {
	score := 0
	switch channel.LastStatus {
	case probe.StatusOperational, "healthy":
		score += 10000
	case probe.StatusDegraded:
		score += 5000
	case "unknown", "":
		score += 3000
	default:
		score += 0
	}

	if channel.CircuitState == CircuitHalfOpen {
		score -= 2000
	} else if channel.CircuitState == CircuitOpen {
		score -= 8000
	}

	score -= channel.FailureCount * 1500

	if strings.Contains(channel.LastError, "429") || strings.Contains(channel.LastError, "Too Many") {
		score -= 3000
	}

	lat := latencyOf(channel)
	if lat < 1000000 {
		score -= lat
	}

	return score
}

func supportsModel(channel *Channel, model string) bool {
	if model == "" {
		return false
	}
	return ChannelSupportsModelOrCanonical(channel, model)
}

func latencyOf(channel *Channel) int {
	if channel.LastLatencyMs == nil || *channel.LastLatencyMs <= 0 {
		return int(^uint(0) >> 1)
	}
	return *channel.LastLatencyMs
}
