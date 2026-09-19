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
	_, _ = r.ProbeNow(ctx, id)
}

// ProbeNow runs one challenge probe and persists the primary result. It is
// shared by the scheduler and the local API's manual probe endpoint.
func (r *ProbeRunner) ProbeNow(ctx context.Context, id int64) ([]*probe.Result, error) {
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

	modelCount := 1 + len(channel.ExtraModels)
	probeCtx, cancel := context.WithTimeout(ctx, probe.RunBudgetFor(modelCount))
	defer cancel()
	results, probeErr := probe.Run(probeCtx, probe.Config{
		Provider:     channel.Provider,
		APIMode:      channel.APIMode,
		Endpoint:     channel.Endpoint,
		APIKey:       channel.APIKey,
		PrimaryModel: channel.PrimaryModel,
		ExtraModels:  channel.ExtraModels,
	})
	if probeErr != nil {
		failureInfo := FailureInfo{Class: FailureNetwork}
		channel.FailureCount++
		channel.LastStatus = probe.StatusError
		channel.LastError = probeErr.Error()
		cooldown := FailureCooldown(failureInfo, channel.FailureCount)
		until := time.Now().UTC().Add(cooldown)
		channel.CooldownUntil = &until
		if channel.FailureCount >= 3 {
			channel.CircuitState = CircuitOpen
		}
		channel.LastLatencyMs = nil
		channel.LastTTFTMs = nil
		channel.LastPingLatencyMs = nil
		channel.ProbeStreak = 0
		channel.LastCheckedAt = timePtr(time.Now().UTC())
		if err := r.store.UpdateRuntime(context.Background(), id, func(fresh *Channel) {
			copyRuntimeState(fresh, channel)
		}); err != nil {
			slog.Warn("desktop channel probe result save failed", "channel_id", id, "error", err)
		}
		return nil, probeErr
	}
	if len(results) == 0 {
		return results, nil
	}

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
	for _, r := range results {
		if r.Model == channel.PrimaryModel {
			primary = r
			break
		}
	}
	if primary == nil && len(results) > 0 {
		primary = results[0]
	}
	issueText := strings.Join(issues, "; ")

	now := time.Now().UTC()
	channel.LastCheckedAt = &now
	if len(results) > 0 {
		channel.LastPingLatencyMs = results[0].PingLatencyMs
	}

	if primary != nil {
		switch primary.Status {
		case probe.StatusOperational:
			channel.LastStatus = probe.StatusOperational
			channel.LastLatencyMs = primary.LatencyMs
			channel.LastTTFTMs = primary.TTFTMs
			channel.LastError = issueText
			channel.ProbeStreak++
			channel.FailureCount = 0
			channel.CircuitState = CircuitClosed
			channel.CooldownUntil = nil
		case probe.StatusDegraded:
			channel.LastStatus = probe.StatusDegraded
			channel.LastLatencyMs = primary.LatencyMs
			channel.LastTTFTMs = primary.TTFTMs
			if primary.Message != "" {
				channel.LastError = fmt.Sprintf("[%s] %s", primary.Model, primary.Message)
			} else {
				channel.LastError = issueText
			}
		default:
			// Primary model failed / error. The channel status is error based on the primary model.
			channel.LastStatus = probe.StatusError
			channel.LastLatencyMs = nil
			channel.LastTTFTMs = nil
			if primary.Message != "" {
				channel.LastError = fmt.Sprintf("[%s] %s", primary.Model, primary.Message)
			} else if issueText != "" {
				channel.LastError = issueText
			} else {
				channel.LastError = fmt.Sprintf("[%s] 探活失败", primary.Model)
			}
			channel.FailureCount++
			channel.ProbeStreak = 0
			cooldown := FailureCooldown(FailureInfo{Class: FailureProvider}, channel.FailureCount)
			until := time.Now().UTC().Add(cooldown)
			channel.CooldownUntil = &until
			if channel.FailureCount >= 3 {
				channel.CircuitState = CircuitOpen
			}
		}
	}
	if err := r.store.UpdateRuntime(context.Background(), id, func(fresh *Channel) {
		copyRuntimeState(fresh, channel)
	}); err != nil {
		slog.Warn("desktop channel probe result save failed", "channel_id", id, "error", err)
	}
	return results, nil
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
