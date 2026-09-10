package service

import (
	"context"
	"fmt"
	"sort"
	"strings"
	"sync"
	"time"
)

const (
	DesktopRoutePriority     = "priority"      // Legacy alias for smart_quality
	DesktopRouteSmartQuality = "smart_quality" // Dynamic quality scoring
	DesktopRouteLatency      = "latency"        // Lowest latency first
	DesktopRouteRoundRobin   = "round_robin"    // Load balancing round robin
)

// DesktopChannelRouter resolves a model to enabled local channels and provides
// a retry loop for protocol adapters. It owns no HTTP protocol code; callers
// supply the actual forward function for Anthropic/OpenAI/Responses.
type DesktopChannelRouter struct {
	store *DesktopChannelStore
	mu    sync.Mutex
	turn  map[string]int
}

func NewDesktopChannelRouter(store *DesktopChannelStore) *DesktopChannelRouter {
	return &DesktopChannelRouter{store: store, turn: make(map[string]int)}
}

// Candidates returns enabled channels that advertise the requested model.
// Recently failed channels remain visible only when no healthy candidate exists,
// making recovery possible without a separate circuit-breaker table.
func (r *DesktopChannelRouter) Candidates(ctx context.Context, model, strategy string) ([]*DesktopChannel, error) {
	return r.CandidatesMatching(ctx, model, strategy, nil)
}

// CandidatesMatching applies an optional protocol predicate before health and
// priority selection. This matters when a model has a healthy Chat channel
// and a degraded Responses channel: filtering after selection would hide the
// only usable Responses route.
func (r *DesktopChannelRouter) CandidatesMatching(ctx context.Context, model, strategy string, matches func(*DesktopChannel) bool) ([]*DesktopChannel, error) {
	if r == nil || r.store == nil {
		return nil, fmt.Errorf("desktop channel router is not configured")
	}
	channels, err := r.store.List(ctx)
	if err != nil {
		return nil, err
	}
	model = strings.TrimSpace(model)
	healthy := make([]*DesktopChannel, 0)
	degraded := make([]*DesktopChannel, 0)
	failed := make([]*DesktopChannel, 0)
	cooldown := make([]*DesktopChannel, 0)
	for _, channel := range channels {
		if !channel.Enabled {
			continue
		}
		if model != "" && !desktopChannelSupportsModel(channel, model) {
			continue
		}
		if matches != nil && !matches(channel) {
			continue
		}
		if channel.CooldownUntil != nil && time.Now().Before(*channel.CooldownUntil) {
			cooldown = append(cooldown, channel)
			continue
		}
		if channel.CircuitState == DesktopCircuitOpen {
			// Lazy half-open recovery: the first request after the cooldown is
			// allowed through as a probe instead of requiring a timer goroutine.
			channel.CircuitState = DesktopCircuitHalf
			_ = r.store.Save(context.Background(), channel)
		}
		if channel.CircuitState == DesktopCircuitHalf {
			healthy = append(healthy, channel)
			continue
		}
		switch channel.LastStatus {
		case MonitorStatusOperational, "healthy", "", "unknown":
			healthy = append(healthy, channel)
		case MonitorStatusDegraded:
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
func (r *DesktopChannelRouter) Route(ctx context.Context, model, strategy string, forward func(context.Context, *DesktopChannel) error) (*DesktopChannel, error) {
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
func (r *DesktopChannelRouter) RouteCandidates(ctx context.Context, model string, candidates []*DesktopChannel, forward func(context.Context, *DesktopChannel) error) (*DesktopChannel, error) {
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
			channel.LastStatus = MonitorStatusOperational
			channel.LastError = ""
			channel.FailureCount = 0
			channel.CircuitState = DesktopCircuitClosed
			channel.CooldownUntil = nil
			channel.LastLatencyMs = &latency
			channel.LastCheckedAt = desktopTimePtr(time.Now().UTC())
			_ = r.store.Save(context.Background(), channel)
			return channel, nil
		}
		lastErr = err
		failureInfo := DesktopFailureFromError(err)
		channel.FailureCount++
		channel.LastStatus = MonitorStatusError
		channel.LastError = DesktopFailureLabel(failureInfo) + ": " + err.Error()
		cooldown := DesktopFailureCooldown(failureInfo, channel.FailureCount)
		until := time.Now().UTC().Add(cooldown)
		channel.CooldownUntil = &until
		if DesktopFailureIsCircuitEligible(failureInfo) && channel.FailureCount >= 3 {
			channel.CircuitState = DesktopCircuitOpen
		} else if channel.CircuitState == "" {
			channel.CircuitState = DesktopCircuitClosed
		}
		channel.LastLatencyMs = &latency
		channel.LastCheckedAt = desktopTimePtr(time.Now().UTC())
		_ = r.store.Save(context.Background(), channel)
	}
	return nil, fmt.Errorf("all desktop channels failed for model %q: %w", model, lastErr)
}

func (r *DesktopChannelRouter) sortCandidates(channels []*DesktopChannel, model, strategy string) {
	sort.SliceStable(channels, func(i, j int) bool {
		switch strategy {
		case DesktopRouteLatency:
			return desktopLatency(channels[i]) < desktopLatency(channels[j])
		case DesktopRouteRoundRobin:
			return desktopLatency(channels[i]) < desktopLatency(channels[j])
		case DesktopRouteSmartQuality, DesktopRoutePriority:
			fallthrough
		default:
			return desktopQualityScore(channels[i]) > desktopQualityScore(channels[j])
		}
	})
	if strategy == DesktopRouteRoundRobin && len(channels) > 1 {
		r.mu.Lock()
		start := r.turn[model] % len(channels)
		r.turn[model]++
		r.mu.Unlock()
		rotated := append(append([]*DesktopChannel{}, channels[start:]...), channels[:start]...)
		copy(channels, rotated)
	}
}

func desktopQualityScore(channel *DesktopChannel) int {
	score := 0
	switch channel.LastStatus {
	case MonitorStatusOperational, "healthy":
		score += 10000
	case MonitorStatusDegraded:
		score += 5000
	case "unknown", "":
		score += 3000
	default:
		score += 0
	}

	if channel.CircuitState == DesktopCircuitHalf {
		score -= 2000
	} else if channel.CircuitState == DesktopCircuitOpen {
		score -= 8000
	}

	score -= channel.FailureCount * 1500

	if strings.Contains(channel.LastError, "429") || strings.Contains(channel.LastError, "Too Many") {
		score -= 3000
	}

	lat := desktopLatency(channel)
	if lat < 1000000 {
		score -= lat
	}

	return score
}

func desktopChannelSupportsModel(channel *DesktopChannel, model string) bool {
	if model == "" {
		return false
	}
	return ChannelSupportsModelOrCanonical(channel, model)
}

func desktopLatency(channel *DesktopChannel) int {
	if channel.LastLatencyMs == nil || *channel.LastLatencyMs <= 0 {
		return int(^uint(0) >> 1)
	}
	return *channel.LastLatencyMs
}
