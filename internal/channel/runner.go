package channel

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"sync"
	"time"

	"github.com/Heiheiaoo/heigate/internal/probe"
)

var (
	ErrProbeBusy       = errors.New("probe already in progress")
	ErrChannelDisabled = errors.New("channel is disabled")
)

// ProbeRunner schedules health probes for local channels. Each channel owns
// one timer and one in-flight slot, so a slow upstream can never pile up
// overlapping probes for the same channel.
type ProbeRunner struct {
	store *Store

	mu       sync.Mutex
	inFlight map[int64]struct{}
}

func NewProbeRunner(store *Store) *ProbeRunner {
	return &ProbeRunner{
		store:    store,
		inFlight: make(map[int64]struct{}),
	}
}

// Start is retained for the desktop host lifecycle. Probes are manual-only;
// startup never schedules upstream requests.
func (r *ProbeRunner) Start(ctx context.Context) error {
	return nil
}

// Stop is retained for the desktop host lifecycle. Manual probes are bounded
// by their request context and do not own background scheduler goroutines.
func (r *ProbeRunner) Stop() {
}

// ProbeNow runs the lightweight channel health check: only the primary model
// is called, because it is the model that drives the channel headline status,
// routing priority, circuit breaker, and adaptive schedule. Extra models are
// available through ProbeModelNow or ProbeAllModelsNow on explicit demand.
func (r *ProbeRunner) ProbeNow(ctx context.Context, id int64) ([]*probe.Result, error) {
	return r.probeNow(ctx, id, "", false)
}

// ProbeModelNow checks one explicitly selected model. When it is the channel's
// current primary model, the normal health/circuit bookkeeping is persisted;
// checking an extra model is observational and does not bench the whole channel.
func (r *ProbeRunner) ProbeModelNow(ctx context.Context, id int64, model string) ([]*probe.Result, error) {
	model = strings.TrimSpace(model)
	if model == "" {
		return r.ProbeNow(ctx, id)
	}
	return r.probeNow(ctx, id, model, false)
}

// ProbeAllModelsNow performs an explicit deep scan of every configured model.
// Models are still checked sequentially by probe.Run to avoid upstream bursts.
func (r *ProbeRunner) ProbeAllModelsNow(ctx context.Context, id int64) ([]*probe.Result, error) {
	return r.probeNow(ctx, id, "", true)
}

func (r *ProbeRunner) probeNow(ctx context.Context, id int64, targetModel string, allModels bool) ([]*probe.Result, error) {
	if r == nil || r.store == nil {
		return nil, context.Canceled
	}
	if !r.acquire(id) {
		return nil, ErrProbeBusy
	}
	defer r.release(id)

	channel, err := r.store.Get(ctx, id)
	if err != nil {
		return nil, err
	}
	if !channel.Enabled {
		return nil, ErrChannelDisabled
	}
	probedPrimary := channel.PrimaryModel
	cfg := probe.Config{
		Provider: channel.Provider,
		APIMode:  channel.APIMode,
		Endpoint: channel.Endpoint,
		APIKey:   channel.APIKey,
	}
	modelCount := 1
	persistPrimary := true
	switch {
	case targetModel != "":
		cfg.PrimaryModel = targetModel
		persistPrimary = targetModel == probedPrimary
	case allModels:
		cfg.PrimaryModel = probedPrimary
		cfg.ExtraModels = channel.ExtraModels
		modelCount += len(channel.ExtraModels)
	default:
		cfg.PrimaryModel = probedPrimary
	}

	probeCtx, cancel := context.WithTimeout(ctx, probe.RunBudgetFor(modelCount))
	defer cancel()
	results, probeErr := probe.Run(probeCtx, cfg)
	// A browser refresh, window close, or client-side timeout cancels the
	// request context. That is not evidence of an upstream failure and must not
	// increment failure_count or open a circuit.
	if ctx.Err() != nil {
		return results, ctx.Err()
	}
	if probeErr != nil {
		if persistPrimary {
			r.persistProbeError(id, probedPrimary, probeErr)
		}
		return nil, probeErr
	}
	if len(results) == 0 {
		return results, nil
	}
	for _, result := range results {
		h := ModelHealthFromProbe(result)
		if err := r.store.SetModelHealth(context.Background(), id, h); err != nil {
			slog.Warn("desktop model probe result save failed", "channel_id", id, "model", result.Model, "error", err)
		}
	}
	if persistPrimary {
		r.persistProbeResults(id, probedPrimary, results)
	}
	return results, nil
}

func probeFailureInfo(result *probe.Result) FailureInfo {
	if result == nil {
		return FailureInfo{Class: FailureProvider}
	}
	message := strings.ToLower(result.Message)
	for _, marker := range []string{"429", "rate limit", "rate_limit", "rpm", "tpm", "quota exceeded", "too many requests"} {
		if strings.Contains(message, marker) {
			return FailureInfo{Class: FailureRateLimit, StatusCode: 429}
		}
	}
	return FailureInfo{Class: FailureProvider}
}

func ModelHealthFromProbe(result *probe.Result) ModelHealth {
	if result == nil {
		return ModelHealth{}
	}
	return ModelHealth{Model: result.Model, Status: result.Status, LatencyMs: result.LatencyMs, TTFTMs: result.TTFTMs, Error: result.Message, CheckedAt: &result.CheckedAt}
}

func (r *ProbeRunner) persistProbeError(id int64, expectedPrimary string, probeErr error) {
	if err := r.store.UpdateRuntime(context.Background(), id, func(fresh *Channel) {
		// A user may change the primary model while a slow probe is in flight.
		// A result for the old model must never become the new model's status.
		if fresh.PrimaryModel != expectedPrimary {
			return
		}
		fresh.FailureCount++
		fresh.LastStatus = probe.StatusError
		fresh.LastError = probeErr.Error()
		cooldown := FailureCooldown(FailureInfo{Class: FailureNetwork}, fresh.FailureCount)
		until := time.Now().UTC().Add(cooldown)
		fresh.CooldownUntil = &until
		if fresh.FailureCount >= 3 {
			fresh.CircuitState = CircuitOpen
		}
		fresh.LastLatencyMs = nil
		fresh.LastTTFTMs = nil
		fresh.LastPingLatencyMs = nil
		fresh.ProbeStreak = 0
		fresh.LastCheckedAt = timePtr(time.Now().UTC())
	}); err != nil {
		slog.Warn("desktop channel probe result save failed", "channel_id", id, "error", err)
	}
}

func (r *ProbeRunner) persistProbeResults(id int64, expectedPrimary string, results []*probe.Result) {
	// Non-operational models are surfaced in the channel's message regardless of
	// which one is primary; the headline status and latency come from the primary
	// alone (resolved below), so per-model counters and latency sums aren't needed.
	var issues []string
	for _, r := range results {
		if r.Status != probe.StatusOperational && r.Message != "" {
			issues = append(issues, fmt.Sprintf("[%s] %s", r.Model, r.Message))
		}
	}

	// The channel's headline status describes its primary model, which is the
	// default routing target. Judging the channel by its worst model meant one
	// stale or rate-limited entry in a long model list marked the whole channel
	// degraded even when its primary served fine — which also kept the channel
	// out of the healthiest routing bucket and blocked adaptive probe backoff.
	// Problems in the other models are carried in the message instead.
	var primary *probe.Result
	for _, result := range results {
		if result.Model == expectedPrimary {
			primary = result
			break
		}
	}
	if primary == nil && len(results) > 0 {
		primary = results[0]
	}
	issueText := strings.Join(issues, "; ")
	if primary == nil {
		return
	}

	if err := r.store.UpdateRuntime(context.Background(), id, func(fresh *Channel) {
		if fresh.PrimaryModel != expectedPrimary {
			return
		}
		now := time.Now().UTC()
		fresh.LastCheckedAt = &now
		fresh.LastPingLatencyMs = primary.PingLatencyMs
		switch primary.Status {
		case probe.StatusOperational:
			fresh.LastStatus = probe.StatusOperational
			fresh.LastLatencyMs = primary.LatencyMs
			fresh.LastTTFTMs = primary.TTFTMs
			fresh.LastError = issueText
			fresh.ProbeStreak++
			fresh.FailureCount = 0
			fresh.CircuitState = CircuitClosed
			fresh.CooldownUntil = nil
		case probe.StatusDegraded:
			fresh.LastStatus = probe.StatusDegraded
			fresh.LastLatencyMs = primary.LatencyMs
			fresh.LastTTFTMs = primary.TTFTMs
			// The request did complete successfully; it is slow, not broken.
			// Clear stale failure/circuit state so a recovered but high-latency
			// channel is not shown as both serving and still熔断中.
			fresh.FailureCount = 0
			fresh.CircuitState = CircuitClosed
			fresh.CooldownUntil = nil
			if primary.Message != "" {
				fresh.LastError = fmt.Sprintf("[%s] %s", primary.Model, primary.Message)
			} else {
				fresh.LastError = issueText
			}
		default:
			// Primary model failed / error. The channel status is error based on the primary model.
			fresh.LastStatus = probe.StatusError
			fresh.LastLatencyMs = nil
			fresh.LastTTFTMs = nil
			if primary.Message != "" {
				fresh.LastError = fmt.Sprintf("[%s] %s", primary.Model, primary.Message)
			} else if issueText != "" {
				fresh.LastError = issueText
			} else {
				fresh.LastError = fmt.Sprintf("[%s] 探活失败", primary.Model)
			}
			fresh.FailureCount++
			fresh.ProbeStreak = 0
			failureInfo := probeFailureInfo(primary)
			cooldown := FailureCooldown(failureInfo, fresh.FailureCount)
			until := time.Now().UTC().Add(cooldown)
			fresh.CooldownUntil = &until
			if FailureIsCircuitEligible(failureInfo) && fresh.FailureCount >= 3 {
				fresh.CircuitState = CircuitOpen
			}
		}
	}); err != nil {
		slog.Warn("desktop channel probe result save failed", "channel_id", id, "error", err)
	}
}

func (r *ProbeRunner) acquire(id int64) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.inFlight[id]; exists {
		return false
	}
	r.inFlight[id] = struct{}{}
	return true
}

func (r *ProbeRunner) release(id int64) {
	r.mu.Lock()
	delete(r.inFlight, id)
	r.mu.Unlock()
}

func timePtr(value time.Time) *time.Time { return &value }
