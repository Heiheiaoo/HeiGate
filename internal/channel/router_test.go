package channel

import (
	"context"
	"database/sql"
	"errors"
	"sync/atomic"
	"testing"
	"time"

	"github.com/Heiheiaoo/heigate/internal/probe"
	_ "modernc.org/sqlite"
)

func TestChannelRouterFailover(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_test?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewStore(db)
	if err != nil {
		t.Fatal(err)
	}
	first := &Channel{Name: "first", Provider: probe.ProviderOpenAI, Endpoint: "https://one.example", APIKey: "key-1", PrimaryModel: "gpt-test", Enabled: true, LastStatus: probe.StatusOperational}
	second := &Channel{Name: "second", Provider: probe.ProviderOpenAI, Endpoint: "https://two.example", APIKey: "key-2", PrimaryModel: "gpt-test", Enabled: true, LastStatus: probe.StatusOperational}
	if err := store.Save(context.Background(), first); err != nil {
		t.Fatal(err)
	}
	if err := store.Save(context.Background(), second); err != nil {
		t.Fatal(err)
	}
	router := NewRouter(store)
	attempts := 0
	selected, err := router.Route(context.Background(), "gpt-test", RouteSmartQuality, func(_ context.Context, channel *Channel) error {
		attempts++
		if channel.Name == "first" {
			return errors.New("upstream unavailable")
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if selected.Name != "second" || attempts != 2 {
		t.Fatalf("expected second channel after one failure, selected=%v attempts=%d", selected.Name, attempts)
	}
	failed, err := store.Get(context.Background(), first.ID)
	if err != nil {
		t.Fatal(err)
	}
	if failed.LastStatus != probe.StatusError {
		t.Fatalf("expected failed channel status to persist, got %q", failed.LastStatus)
	}
}

func TestChannelRouterCooldownAndRecovery(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_cooldown?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewStore(db)
	if err != nil {
		t.Fatal(err)
	}
	cooldown := time.Now().UTC().Add(5 * time.Minute)
	blocked := &Channel{Name: "blocked", Provider: probe.ProviderOpenAI, Endpoint: "https://blocked.example", APIKey: "key", PrimaryModel: "gpt-test", Enabled: true, LastStatus: probe.StatusOperational, CooldownUntil: &cooldown}
	available := &Channel{Name: "available", Provider: probe.ProviderOpenAI, Endpoint: "https://available.example", APIKey: "key", PrimaryModel: "gpt-test", Enabled: true, LastStatus: probe.StatusOperational}
	if err := store.Save(context.Background(), blocked); err != nil {
		t.Fatal(err)
	}
	if err := store.Save(context.Background(), available); err != nil {
		t.Fatal(err)
	}
	router := NewRouter(store)
	candidates, err := router.Candidates(context.Background(), "gpt-test", RouteSmartQuality)
	if err != nil {
		t.Fatal(err)
	}
	if len(candidates) != 1 || candidates[0].Name != "available" {
		t.Fatalf("expected cooldown channel to be skipped, got %#v", candidates)
	}
	if _, err := router.Route(context.Background(), "gpt-test", RouteSmartQuality, func(_ context.Context, _ *Channel) error {
		return errors.New("temporary outage")
	}); err == nil {
		t.Fatal("expected route failure")
	}
	updated, err := store.Get(context.Background(), available.ID)
	if err != nil {
		t.Fatal(err)
	}
	if updated.FailureCount != 1 || updated.CooldownUntil == nil {
		t.Fatalf("expected failure cooldown to persist: %#v", updated)
	}
}

func TestChannelRouterAllowsOnlyOneHalfOpenRequest(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_half_open?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewStore(db)
	if err != nil {
		t.Fatal(err)
	}
	ch := &Channel{Name: "recovering", Provider: probe.ProviderOpenAI, Endpoint: "https://recovering.example", APIKey: "k", PrimaryModel: "gpt-test", Enabled: true, LastStatus: probe.StatusError, CircuitState: CircuitHalfOpen}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}
	router := NewRouter(store)
	candidates := []*Channel{ch}
	started := make(chan struct{})
	release := make(chan struct{})
	var calls atomic.Int32
	firstDone := make(chan error, 1)
	go func() {
		_, routeErr := router.RouteCandidates(context.Background(), "gpt-test", candidates, func(context.Context, *Channel) error {
			calls.Add(1)
			close(started)
			<-release
			return nil
		})
		firstDone <- routeErr
	}()
	<-started
	_, secondErr := router.RouteCandidates(context.Background(), "gpt-test", candidates, func(context.Context, *Channel) error {
		calls.Add(1)
		return nil
	})
	if secondErr == nil {
		t.Fatal("concurrent half-open request should be skipped")
	}
	close(release)
	if err := <-firstDone; err != nil {
		t.Fatal(err)
	}
	if got := calls.Load(); got != 1 {
		t.Fatalf("half-open channel forwarded %d concurrent requests, want 1", got)
	}
}

func TestFailureClassification(t *testing.T) {
	if info := FailureFromError(context.Canceled); info.Class != FailureClient {
		t.Fatalf("context cancellation must be client-owned, got %#v", info)
	}
	info := ClassifyStatus(429)
	if info.Class != FailureRateLimit {
		t.Fatalf("expected rate limit classification, got %#v", info)
	}
	if FailureCooldown(info, 1) != 15*time.Second {
		t.Fatalf("rate limit without Retry-After should cool down for 15s, got %s", FailureCooldown(info, 1))
	}
	if FailureCooldown(info, 3) != time.Minute {
		t.Fatalf("repeated rate limits should back off, got %s", FailureCooldown(info, 3))
	}
	info.RetryAfter = 42 * time.Second
	if FailureCooldown(info, 1) != 42*time.Second {
		t.Fatalf("rate limit should honour Retry-After, got %s", FailureCooldown(info, 1))
	}
	if FailureIsRetryable(info) {
		t.Fatal("rate limits must not be retried immediately on the same channel")
	}
	serverInfo := ClassifyStatus(503)
	if !FailureIsCircuitEligible(serverInfo) {
		t.Fatal("expected 503 to be circuit eligible")
	}
	timeoutInfo := ClassifyStatus(408)
	if timeoutInfo.Class != FailureNetwork {
		t.Fatalf("expected 408 to be network failure, got %#v", timeoutInfo)
	}
}

func TestChannelRouterCanonicalMatching(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_canonical?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewStore(db)
	if err != nil {
		t.Fatal(err)
	}

	// Channel 1: Nexus has "glm-5.3-flash-free"
	nexus := &Channel{
		Name:         "Nexus",
		Provider:     probe.ProviderOpenAI,
		Endpoint:     "https://nexus.example",
		APIKey:       "key-nexus",
		PrimaryModel: "glm-5.3-flash-free",
		Enabled:      true,
		LastStatus:   probe.StatusOperational,
	}
	// Channel 2: B.AI has "glm-5.3-flash"
	bai := &Channel{
		Name:         "B.AI",
		Provider:     probe.ProviderOpenAI,
		Endpoint:     "https://bai.example",
		APIKey:       "key-bai",
		PrimaryModel: "glm-5.3-flash",
		Enabled:      true,
		LastStatus:   probe.StatusOperational,
	}
	// Channel 3: 芝公益站 has "z-ai/glm-5.3-free"
	zhi := &Channel{
		Name:         "芝公益站",
		Provider:     probe.ProviderOpenAI,
		Endpoint:     "https://zhi.example",
		APIKey:       "key-zhi",
		PrimaryModel: "z-ai/glm-5.3-free",
		Enabled:      true,
		LastStatus:   probe.StatusOperational,
	}

	if err := store.Save(context.Background(), nexus); err != nil {
		t.Fatal(err)
	}
	if err := store.Save(context.Background(), bai); err != nil {
		t.Fatal(err)
	}
	if err := store.Save(context.Background(), zhi); err != nil {
		t.Fatal(err)
	}

	router := NewRouter(store)

	// Client requests standard name "glm-5.3-flash"
	candidates, err := router.Candidates(context.Background(), "glm-5.3-flash", RouteSmartQuality)
	if err != nil {
		t.Fatalf("expected candidates for glm-5.3-flash, got err: %v", err)
	}
	if len(candidates) != 3 {
		t.Fatalf("expected all 3 channels to match canonical glm-5.3-flash, got %d", len(candidates))
	}
	if candidates[0].Name != "Nexus" || candidates[1].Name != "B.AI" || candidates[2].Name != "芝公益站" {
		t.Fatalf("unexpected candidate order: %#v", candidates)
	}

	// Verify target model resolution for each candidate
	expectedModels := map[string]string{
		"Nexus": "glm-5.3-flash-free",
		"B.AI":  "glm-5.3-flash",
		"芝公益站":  "z-ai/glm-5.3-free",
	}
	for _, c := range candidates {
		target, _, ok := FindChannelModelForRequest(c, "glm-5.3-flash")
		if !ok || target != expectedModels[c.Name] {
			t.Errorf("channel %s: expected target model %q, got %q (ok=%v)", c.Name, expectedModels[c.Name], target, ok)
		}
	}
}

// A client-class failure (400) is the request's fault. It must not count
// against the channel, otherwise one malformed request benches a healthy
// channel for 30s and removes it from the routing pool.
func TestRouteClientFaultDoesNotPenalizeChannel(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_client_fault?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewStore(db)
	if err != nil {
		t.Fatal(err)
	}
	ch := &Channel{
		Name: "healthy", Provider: probe.ProviderOpenAI, Endpoint: "https://x.example",
		APIKey: "k", PrimaryModel: "gpt-test", Enabled: true, LastStatus: probe.StatusOperational,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	router := NewRouter(store)
	_, err = router.Route(context.Background(), "gpt-test", RouteSmartQuality, func(_ context.Context, _ *Channel) error {
		return &upstreamStatusStub{status: 400}
	})
	if err == nil {
		t.Fatal("expected the client error to surface")
	}

	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.FailureCount != 0 {
		t.Fatalf("client fault counted against the channel: failure_count=%d", loaded.FailureCount)
	}
	if loaded.CooldownUntil != nil {
		t.Fatal("client fault set a cooldown, benching a healthy channel")
	}
	if loaded.LastStatus == probe.StatusError {
		t.Fatal("client fault marked the channel as errored")
	}
}

// A transport failure is the channel's fault and must still be counted.
func TestRouteChannelFaultStillPenalizes(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_channel_fault?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewStore(db)
	if err != nil {
		t.Fatal(err)
	}
	ch := &Channel{
		Name: "flaky", Provider: probe.ProviderOpenAI, Endpoint: "https://y.example",
		APIKey: "k", PrimaryModel: "gpt-test", Enabled: true, LastStatus: probe.StatusOperational,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	router := NewRouter(store)
	_, err = router.Route(context.Background(), "gpt-test", RouteSmartQuality, func(_ context.Context, _ *Channel) error {
		return errors.New("connection refused")
	})
	if err == nil {
		t.Fatal("expected the transport error to surface")
	}

	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.FailureCount != 1 {
		t.Fatalf("transport failure not counted: failure_count=%d", loaded.FailureCount)
	}
	if loaded.CooldownUntil == nil {
		t.Fatal("transport failure should set a cooldown")
	}
}

// upstreamStatusStub carries a status that FailureFromError classifies.
type upstreamStatusStub struct{ status int }

func (e *upstreamStatusStub) Error() string { return "stub upstream error" }

func (e *upstreamStatusStub) FailureInfo() FailureInfo { return ClassifyStatus(e.status) }

// A 429 must bench the channel for a while. Without a cooldown the router
// promotes it to a half-open probe at the front of the queue, so every request
// would hit the rate-limited upstream first before failing over.
func TestRateLimitedChannelIsNotTriedFirstOnEveryRequest(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_rate_limit?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewStore(db)
	if err != nil {
		t.Fatal(err)
	}
	limited := &Channel{Name: "limited", Provider: probe.ProviderOpenAI, Endpoint: "https://limited.example", APIKey: "k", PrimaryModel: "m", Enabled: true, LastStatus: probe.StatusOperational}
	healthy := &Channel{Name: "healthy", Provider: probe.ProviderOpenAI, Endpoint: "https://healthy.example", APIKey: "k", PrimaryModel: "m", Enabled: true, LastStatus: probe.StatusOperational}
	for _, ch := range []*Channel{limited, healthy} {
		if err := store.Save(context.Background(), ch); err != nil {
			t.Fatal(err)
		}
	}
	router := NewRouter(store)
	limitedHits := 0
	for i := 0; i < 10; i++ {
		_, err := router.Route(context.Background(), "m", RouteSmartQuality, func(_ context.Context, ch *Channel) error {
			if ch.Name == "limited" {
				limitedHits++
				return &upstreamStatusStub{status: 429}
			}
			return nil
		})
		if err != nil {
			t.Fatal(err)
		}
	}
	if limitedHits != 1 {
		t.Fatalf("rate-limited channel was tried %d times across 10 requests, want 1", limitedHits)
	}
}
