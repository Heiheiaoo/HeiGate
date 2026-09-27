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
	RouteSmartQuality = "smart_quality" // Dynamic quality scoring
	RouteLatency      = "latency"       // Lowest latency first
	RouteRoundRobin   = "round_robin"   // Load balancing round robin
)

// Router resolves a model to enabled local channels and provides
// a retry loop for protocol adapters. It owns no HTTP protocol code; callers
// supply the actual forward function for Anthropic/OpenAI/Responses.
type Router struct {
	store    *Store
	mu       sync.Mutex
	turn     map[string]int
	halfOpen map[int64]struct{}
}

func NewRouter(store *Store) *Router {
	return &Router{store: store, turn: make(map[string]int), halfOpen: make(map[int64]struct{})}
}

// Candidates returns enabled channels that advertise the requested model.
// Failed channels re-enter the pool as single-request recovery probes after
// their cooldown expires, so recovery does not depend on manual probing.
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
	now := time.Now()
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
		if channel.CooldownUntil != nil && now.Before(*channel.CooldownUntil) {
			cooldown = append(cooldown, channel)
			continue
		}
		if channel.CircuitState == CircuitOpen || (channel.FailureCount > 0 && channel.LastStatus == probe.StatusError) {
			// Lazy half-open recovery: after the channel's cooldown expires, let
			// one real request test it. This also recovers failures that have not
			// reached the circuit-open threshold without requiring manual probing.
			channel.CircuitState = CircuitHalfOpen
			_ = r.store.UpdateRuntime(context.Background(), channel.ID, func(fresh *Channel) {
				cooldownExpired := fresh.CooldownUntil == nil || !now.Before(*fresh.CooldownUntil)
				if cooldownExpired && (fresh.CircuitState == CircuitOpen || (fresh.FailureCount > 0 && fresh.LastStatus == probe.StatusError)) {
					fresh.CircuitState = CircuitHalfOpen
				}
			})
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
		// A half-open circuit is a single recovery probe, not a concurrency
		// bucket. Candidates can be built by several requests at once, so claim
		// the persisted half-open channel immediately before forwarding it.
		halfOpenClaimed := channel.CircuitState == CircuitHalfOpen && r.claimHalfOpen(channel.ID)
		if channel.CircuitState == CircuitHalfOpen && !halfOpenClaimed {
			continue
		}
		if halfOpenClaimed {
			defer r.releaseHalfOpen(channel.ID)
		}
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
			// Protocol forwarders can supply the measured time to first byte on
			// the channel snapshot. Do not substitute total request duration when
			// it is absent; keep the last actual TTFT measurement instead.
			ttft := channel.LastTTFTMs
			channel.LastCheckedAt = timePtr(time.Now().UTC())
			_ = r.store.UpdateRuntime(context.Background(), channel.ID, func(fresh *Channel) {
				fresh.LastStatus = probe.StatusOperational
				fresh.LastError = ""
				fresh.FailureCount = 0
				fresh.CircuitState = CircuitClosed
				fresh.CooldownUntil = nil
				fresh.LastLatencyMs = &latency
				if ttft != nil && *ttft > 0 {
					ttftCopy := *ttft
					fresh.LastTTFTMs = &ttftCopy
				}
				fresh.LastCheckedAt = timePtr(time.Now().UTC())
			})
			return channel, nil
		}
		lastErr = err
		failureInfo := FailureFromError(err)
		// A client-class failure (4xx other than 408/429) is the request's
		// fault. Counting it would let one malformed request bench a healthy
		// channel for 30s and drop it out of the routing pool, so only the
		// failure is returned.
		if FailureIsChannelFault(failureInfo) {
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
			channel.LastTTFTMs = nil
			channel.LastCheckedAt = timePtr(time.Now().UTC())
			_ = r.store.UpdateRuntime(context.Background(), channel.ID, func(fresh *Channel) {
				fresh.FailureCount++
				fresh.LastStatus = probe.StatusError
				fresh.LastError = FailureLabel(failureInfo) + ": " + err.Error()
				cooldown := FailureCooldown(failureInfo, fresh.FailureCount)
				until := time.Now().UTC().Add(cooldown)
				fresh.CooldownUntil = &until
				if FailureIsCircuitEligible(failureInfo) && fresh.FailureCount >= 3 {
					fresh.CircuitState = CircuitOpen
				} else if fresh.CircuitState == "" {
					fresh.CircuitState = CircuitClosed
				}
				fresh.LastLatencyMs = &latency
				fresh.LastTTFTMs = nil
				fresh.LastCheckedAt = timePtr(time.Now().UTC())
			})
		}
	}
	if lastErr == nil {
		lastErr = fmt.Errorf("all candidate channels are currently recovering")
	}
	return nil, fmt.Errorf("all desktop channels failed for model %q: %w", model, lastErr)
}

func (r *Router) claimHalfOpen(id int64) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.halfOpen[id]; exists {
		return false
	}
	r.halfOpen[id] = struct{}{}
	return true
}

func (r *Router) releaseHalfOpen(id int64) {
	r.mu.Lock()
	delete(r.halfOpen, id)
	r.mu.Unlock()
}

func (r *Router) sortCandidates(channels []*Channel, model, strategy string) {
	sort.SliceStable(channels, func(i, j int) bool {
		// Recovery probes should get a chance promptly under every routing
		// strategy. RouteCandidates still enforces one in-flight half-open call.
		if (channels[i].CircuitState == CircuitHalfOpen) != (channels[j].CircuitState == CircuitHalfOpen) {
			return channels[i].CircuitState == CircuitHalfOpen
		}
		switch strategy {
		case RouteLatency:
			return responsivenessOf(channels[i]) < responsivenessOf(channels[j])
		case RouteRoundRobin:
			return responsivenessOf(channels[i]) < responsivenessOf(channels[j])
		case RouteSmartQuality:
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
		score += 20000
	} else if channel.CircuitState == CircuitOpen {
		score -= 8000
	}

	score -= channel.FailureCount * 1500

	if strings.Contains(channel.LastError, "429") || strings.Contains(channel.LastError, "Too Many") {
		score -= 3000
	}

	lat := responsivenessOf(channel)
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

// responsivenessOf is the routing sort key: time to first token when the probe
// measured it, total latency otherwise. Clients consume streams, so the wait
// they feel is the first token, not the end of generation.
func responsivenessOf(channel *Channel) int {
	if channel.LastTTFTMs != nil && *channel.LastTTFTMs > 0 {
		return *channel.LastTTFTMs
	}
	return latencyOf(channel)
}
