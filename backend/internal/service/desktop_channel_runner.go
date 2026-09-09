package service

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"time"
)

// DesktopChannelProbeRunner schedules challenge probes for local channels.
// Each channel owns one timer and one in-flight slot, matching the behavior of
// ChannelMonitorRunner without the SaaS settings and Redis dependencies.
type DesktopChannelProbeRunner struct {
	store  *DesktopChannelStore
	ctx    context.Context
	cancel context.CancelFunc

	mu       sync.Mutex
	tasks    map[int64]context.CancelFunc
	inFlight map[int64]struct{}
}

func NewDesktopChannelProbeRunner(store *DesktopChannelStore) *DesktopChannelProbeRunner {
	ctx, cancel := context.WithCancel(context.Background())
	return &DesktopChannelProbeRunner{
		store:    store,
		ctx:      ctx,
		cancel:   cancel,
		tasks:    make(map[int64]context.CancelFunc),
		inFlight: make(map[int64]struct{}),
	}
}

// Start loads enabled channels and schedules each one. Existing channels run
// one probe immediately, then continue at their configured interval.
func (r *DesktopChannelProbeRunner) Start(ctx context.Context) error {
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
func (r *DesktopChannelProbeRunner) Schedule(channel *DesktopChannel) {
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

func (r *DesktopChannelProbeRunner) Unschedule(id int64) {
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

func (r *DesktopChannelProbeRunner) Stop() {
	if r == nil {
		return
	}
	r.cancel()
	r.mu.Lock()
	r.tasks = make(map[int64]context.CancelFunc)
	r.mu.Unlock()
}

func (r *DesktopChannelProbeRunner) runScheduled(ctx context.Context, id int64, interval time.Duration) {
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

func (r *DesktopChannelProbeRunner) runOne(ctx context.Context, id int64) {
	_, _ = r.ProbeNow(ctx, id)
}

// ProbeNow runs one challenge probe and persists the primary result. It is
// shared by the scheduler and the local API's manual probe endpoint.
func (r *DesktopChannelProbeRunner) ProbeNow(ctx context.Context, id int64) ([]*CheckResult, error) {
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

	probeCtx, cancel := context.WithTimeout(ctx, monitorRequestTimeout+monitorPingTimeout+monitorRunOneBuffer)
	defer cancel()
	results, probeErr := ProbeChannel(probeCtx, ChannelProbeConfig{
		Provider:     channel.Provider,
		APIMode:      channel.APIMode,
		Endpoint:     channel.Endpoint,
		APIKey:       channel.APIKey,
		PrimaryModel: channel.PrimaryModel,
		ExtraModels:  channel.ExtraModels,
	})
	if probeErr != nil {
		failureInfo := DesktopFailureInfo{Class: DesktopFailureNetwork}
		channel.FailureCount++
		channel.LastStatus = MonitorStatusError
		channel.LastError = probeErr.Error()
		cooldown := DesktopFailureCooldown(failureInfo, channel.FailureCount)
		until := time.Now().UTC().Add(cooldown)
		channel.CooldownUntil = &until
		if channel.FailureCount >= 3 {
			channel.CircuitState = DesktopCircuitOpen
		}
		channel.LastLatencyMs = nil
		channel.LastPingLatencyMs = nil
		channel.LastCheckedAt = desktopTimePtr(time.Now().UTC())
		if err := r.store.Save(context.Background(), channel); err != nil {
			slog.Warn("desktop channel probe result save failed", "channel_id", id, "error", err)
		}
		return nil, probeErr
	}
	if len(results) == 0 {
		return results, nil
	}

	var okCount, degradedCount, errCount int
	var totalLatency int
	var firstErr string
	for _, r := range results {
		if r.Status == MonitorStatusOperational {
			okCount++
			if r.LatencyMs != nil {
				totalLatency += *r.LatencyMs
			}
		} else if r.Status == MonitorStatusDegraded {
			degradedCount++
			if r.LatencyMs != nil {
				totalLatency += *r.LatencyMs
			}
			if firstErr == "" && r.Message != "" {
				firstErr = fmt.Sprintf("[%s] %s", r.Model, r.Message)
			}
		} else {
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
		channel.LastStatus = MonitorStatusError
		channel.LastError = firstErr
		channel.FailureCount++
		channel.LastLatencyMs = nil
	} else if errCount > 0 || degradedCount > 0 {
		channel.LastStatus = MonitorStatusDegraded
		if errCount > 0 {
			channel.LastError = fmt.Sprintf("%d/%d 模型异常: %s", errCount, len(results), firstErr)
		} else {
			channel.LastError = firstErr
		}
		if okCount+degradedCount > 0 {
			avgLatency := totalLatency / (okCount + degradedCount)
			channel.LastLatencyMs = &avgLatency
		}
	} else {
		channel.LastStatus = MonitorStatusOperational
		channel.LastError = ""
		channel.FailureCount = 0
		channel.CircuitState = DesktopCircuitClosed
		channel.CooldownUntil = nil
		if okCount > 0 {
			avgLatency := totalLatency / okCount
			channel.LastLatencyMs = &avgLatency
		}
	}
	if err := r.store.Save(context.Background(), channel); err != nil {
		slog.Warn("desktop channel probe result save failed", "channel_id", id, "error", err)
	}
	return results, nil
}

func (r *DesktopChannelProbeRunner) acquire(id int64) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.inFlight[id]; exists {
		return false
	}
	r.inFlight[id] = struct{}{}
	return true
}

func (r *DesktopChannelProbeRunner) release(id int64) {
	r.mu.Lock()
	delete(r.inFlight, id)
	r.mu.Unlock()
}

func desktopTimePtr(value time.Time) *time.Time { return &value }
