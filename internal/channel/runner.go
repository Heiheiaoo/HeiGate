package channel

import (
	"context"
	"fmt"
	"log/slog"
	"strings"
	"sync"
	"time"

	"github.com/Heiheiaoo/heigate/internal/probe"
)

// ProbeRunner schedules health probes for local channels. Each channel owns
// one timer and one in-flight slot, so a slow upstream can never pile up
// overlapping probes for the same channel.
type ProbeRunner struct {
	store  *Store
	ctx    context.Context
	cancel context.CancelFunc

	mu       sync.Mutex
	tasks    map[int64]context.CancelFunc
	inFlight map[int64]struct{}
}

func NewProbeRunner(store *Store) *ProbeRunner {
	ctx, cancel := context.WithCancel(context.Background())
	return &ProbeRunner{
		store:    store,
		ctx:      ctx,
		cancel:   cancel,
		tasks:    make(map[int64]context.CancelFunc),
		inFlight: make(map[int64]struct{}),
	}
}

// Start loads enabled channels and schedules each one. Existing channels run
// one probe immediately, then continue at their configured interval.
func (r *ProbeRunner) Start(ctx context.Context) error {
	if r == nil || r.store == nil {
		return nil
	}
	channels, err := r.store.List(ctx)
	if err != nil {
		return err
	}
	for _, channel := range channels {
		r.Schedule(channel)
	}
	return nil
}

// Schedule creates or replaces the timer for one channel. Disabled channels
// are unscheduled and never generate upstream traffic.
func (r *ProbeRunner) Schedule(channel *Channel) {
	if r == nil || channel == nil || channel.ID == 0 {
		return
	}
	if !channel.Enabled || channel.IntervalSeconds <= 0 {
		r.Unschedule(channel.ID)
		return
	}
	interval := time.Duration(channel.IntervalSeconds) * time.Second
	if interval < 15*time.Second {
		interval = 15 * time.Second
	}
	r.Unschedule(channel.ID)
	ctx, cancel := context.WithCancel(r.ctx)
	r.mu.Lock()
	r.tasks[channel.ID] = cancel
	r.mu.Unlock()
	go r.runScheduled(ctx, channel.ID, interval)
}

// ProbeOnce runs a single probe in the background, independent of the timer
// set up by Schedule. A newly created channel would otherwise show "unknown"
// until its first interval elapses, and a manual-only channel (interval 0)
// has no timer at all, so it would never be probed without this.
func (r *ProbeRunner) ProbeOnce(channel *Channel) {
	if r == nil || channel == nil || channel.ID == 0 || !channel.Enabled {
		return
	}
	if channel.IntervalSeconds > 0 {
		// Schedule already fires an immediate probe; a second one would just
		// duplicate upstream traffic.
		return
	}
	go r.runOne(r.ctx, channel.ID)
}

func (r *ProbeRunner) Unschedule(id int64) {
	if r == nil {
		return
	}
	r.mu.Lock()
	cancel := r.tasks[id]
	delete(r.tasks, id)
	r.mu.Unlock()
	if cancel != nil {
		cancel()
	}
}

func (r *ProbeRunner) Stop() {
	if r == nil {
		return
	}
	r.cancel()
	r.mu.Lock()
	r.tasks = make(map[int64]context.CancelFunc)
	r.mu.Unlock()
}

func (r *ProbeRunner) runScheduled(ctx context.Context, id int64, base time.Duration) {
	for {
		r.runOne(ctx, id)
		timer := time.NewTimer(r.nextInterval(ctx, id, base))
		select {
		case <-ctx.Done():
			timer.Stop()
			return
		case <-timer.C:
		}
	}
}

// nextInterval re-reads the channel each tick so the wait reflects the streak
// the probe just updated. Capturing the interval once would freeze the backoff
// and stop a recovered channel from being re-checked promptly.
func (r *ProbeRunner) nextInterval(ctx context.Context, id int64, base time.Duration) time.Duration {
	ch, err := r.store.Get(ctx, id)
	if err != nil {
		return base
	}
	return AdaptiveProbeInterval(base, ch.ProbeStreak)
}

// probeBackoffCap bounds how far a quiet channel's interval is stretched. Two
// hours is a backstop, not the primary health signal: real traffic trips the
// circuit breaker within seconds, so the probe only has to notice a channel
// that nothing is routing to. A lower cap would do nothing for the common
// case of a channel already configured to probe every 30 minutes.
const probeBackoffCap = 2 * time.Hour

// AdaptiveProbeInterval returns how long to wait before the next automatic
// probe, given how many consecutive fully-healthy probes the channel has had.
// Every probe is a real billed inference request and some upstreams charge a
// large fixed input prompt on each one, so a stable channel is polled less
// often. Any non-operational result resets the streak, dropping the interval
// straight back to the configured value so recovery is noticed promptly.
func AdaptiveProbeInterval(base time.Duration, streak int) time.Duration {
	if base <= 0 {
		return base
	}
	var factor float64
	switch {
	case streak >= 20:
		factor = 4
	case streak >= 10:
		factor = 2
	case streak >= 5:
		factor = 1.5
	default:
		return base
	}
	interval := time.Duration(float64(base) * factor)
	// The cap is a ceiling on escalation, never a shortening: a channel the
	// user already configured slower than the cap keeps its own cadence.
	limit := probeBackoffCap
	if base > limit {
		limit = base
	}
	if interval > limit {
		return limit
	}
	return interval
}

func (r *ProbeRunner) runOne(ctx context.Context, id int64) {
	_, _ = r.ProbeAutomatic(ctx, id)
}

// ProbeNow runs the lightweight channel health check: only the primary model
// is called, because it is the model that drives the channel headline status,
// routing priority, circuit breaker, and adaptive schedule. Extra models are
// available through ProbeModelNow or ProbeAllModelsNow on explicit demand.
func (r *ProbeRunner) ProbeNow(ctx context.Context, id int64) ([]*probe.Result, error) {
	return r.probeNow(ctx, id, "", false)
}

// ProbeAutomatic is the scheduler-facing name for a single primary-model
// sample. All probe modes are single-attempt; the next click or tick is the
// next sample.
func (r *ProbeRunner) ProbeAutomatic(ctx context.Context, id int64) ([]*probe.Result, error) {
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
		return nil, nil
	}
	defer r.release(id)

	channel, err := r.store.Get(ctx, id)
	if err != nil {
		return nil, err
	}
	if !channel.Enabled {
		r.Unschedule(id)
		return nil, nil
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
	if probeErr != nil {
		if persistPrimary {
			r.persistProbeError(id, probedPrimary, probeErr)
		}
		return nil, probeErr
	}
	if len(results) == 0 {
		return results, nil
	}
	if persistPrimary {
		r.persistProbeResults(id, probedPrimary, results)
	}
	return results, nil
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
			cooldown := FailureCooldown(FailureInfo{Class: FailureProvider}, fresh.FailureCount)
			until := time.Now().UTC().Add(cooldown)
			fresh.CooldownUntil = &until
			if fresh.FailureCount >= 3 {
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
