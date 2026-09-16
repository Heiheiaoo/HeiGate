package channel

import (
	"context"
	"fmt"
	"log/slog"
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

func (r *ProbeRunner) runScheduled(ctx context.Context, id int64, interval time.Duration) {
	r.runOne(ctx, id)
	timer := time.NewTimer(interval)
	defer timer.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-timer.C:
			r.runOne(ctx, id)
			timer.Reset(interval)
		}
	}
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

	probeCtx, cancel := context.WithTimeout(ctx, probe.RunBudget())
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

	var okCount, degradedCount, errCount int
	var totalLatency, totalTTFT int
	var firstErr string
	for _, r := range results {
		switch r.Status {
		case probe.StatusOperational:
			okCount++
			if r.LatencyMs != nil {
				totalLatency += *r.LatencyMs
			}
			if r.TTFTMs != nil {
				totalTTFT += *r.TTFTMs
			}
		case probe.StatusDegraded:
			degradedCount++
			if r.LatencyMs != nil {
				totalLatency += *r.LatencyMs
			}
			if r.TTFTMs != nil {
				totalTTFT += *r.TTFTMs
			}
			if firstErr == "" && r.Message != "" {
				firstErr = fmt.Sprintf("[%s] %s", r.Model, r.Message)
			}
		default:
			errCount++
			if firstErr == "" && r.Message != "" {
				firstErr = fmt.Sprintf("[%s] %s", r.Model, r.Message)
			}
		}
	}

	now := time.Now().UTC()
	channel.LastCheckedAt = &now
	if len(results) > 0 {
		channel.LastPingLatencyMs = results[0].PingLatencyMs
	}

	if errCount == len(results) {
		channel.LastStatus = probe.StatusError
		channel.LastError = firstErr
		channel.FailureCount++
		channel.LastLatencyMs = nil
		channel.LastTTFTMs = nil
		// Same bookkeeping as the transport-failure path above and the forward
		// path in Router.RouteCandidates: without it a channel whose probes all
		// fail keeps a closed circuit and no cooldown forever, so it stays a
		// routing candidate and is retried on every request.
		cooldown := FailureCooldown(FailureInfo{Class: FailureProvider}, channel.FailureCount)
		until := time.Now().UTC().Add(cooldown)
		channel.CooldownUntil = &until
		if channel.FailureCount >= 3 {
			channel.CircuitState = CircuitOpen
		}
	} else if errCount > 0 || degradedCount > 0 {
		channel.LastStatus = probe.StatusDegraded
		if errCount > 0 {
			channel.LastError = fmt.Sprintf("%d/%d 模型异常: %s", errCount, len(results), firstErr)
		} else {
			channel.LastError = firstErr
		}
		if okCount+degradedCount > 0 {
			avgLatency := totalLatency / (okCount + degradedCount)
			channel.LastLatencyMs = &avgLatency
			if totalTTFT > 0 {
				avgTTFT := totalTTFT / (okCount + degradedCount)
				channel.LastTTFTMs = &avgTTFT
			}
		}
	} else {
		channel.LastStatus = probe.StatusOperational
		channel.LastError = ""
		channel.FailureCount = 0
		channel.CircuitState = CircuitClosed
		channel.CooldownUntil = nil
		if okCount > 0 {
			avgLatency := totalLatency / okCount
			channel.LastLatencyMs = &avgLatency
			if totalTTFT > 0 {
				avgTTFT := totalTTFT / okCount
				channel.LastTTFTMs = &avgTTFT
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
