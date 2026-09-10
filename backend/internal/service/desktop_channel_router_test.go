package service

import (
	"context"
	"database/sql"
	"errors"
	"testing"
	"time"

	_ "modernc.org/sqlite"
)

func TestDesktopChannelRouterFailover(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_test?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewDesktopChannelStore(db)
	if err != nil {
		t.Fatal(err)
	}
	first := &DesktopChannel{Name: "first", Provider: MonitorProviderOpenAI, Endpoint: "https://one.example", APIKey: "key-1", PrimaryModel: "gpt-test", Enabled: true, Priority: 1, LastStatus: MonitorStatusOperational}
	second := &DesktopChannel{Name: "second", Provider: MonitorProviderOpenAI, Endpoint: "https://two.example", APIKey: "key-2", PrimaryModel: "gpt-test", Enabled: true, Priority: 2, LastStatus: MonitorStatusOperational}
	if err := store.Save(context.Background(), first); err != nil {
		t.Fatal(err)
	}
	if err := store.Save(context.Background(), second); err != nil {
		t.Fatal(err)
	}
	router := NewDesktopChannelRouter(store)
	attempts := 0
	selected, err := router.Route(context.Background(), "gpt-test", DesktopRoutePriority, func(_ context.Context, channel *DesktopChannel) error {
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
	if failed.LastStatus != MonitorStatusError {
		t.Fatalf("expected failed channel status to persist, got %q", failed.LastStatus)
	}
}

func TestDesktopChannelRouterCooldownAndRecovery(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_cooldown?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewDesktopChannelStore(db)
	if err != nil {
		t.Fatal(err)
	}
	cooldown := time.Now().UTC().Add(5 * time.Minute)
	blocked := &DesktopChannel{Name: "blocked", Provider: MonitorProviderOpenAI, Endpoint: "https://blocked.example", APIKey: "key", PrimaryModel: "gpt-test", Enabled: true, Priority: 1, LastStatus: MonitorStatusOperational, CooldownUntil: &cooldown}
	available := &DesktopChannel{Name: "available", Provider: MonitorProviderOpenAI, Endpoint: "https://available.example", APIKey: "key", PrimaryModel: "gpt-test", Enabled: true, Priority: 2, LastStatus: MonitorStatusOperational}
	if err := store.Save(context.Background(), blocked); err != nil {
		t.Fatal(err)
	}
	if err := store.Save(context.Background(), available); err != nil {
		t.Fatal(err)
	}
	router := NewDesktopChannelRouter(store)
	candidates, err := router.Candidates(context.Background(), "gpt-test", DesktopRoutePriority)
	if err != nil {
		t.Fatal(err)
	}
	if len(candidates) != 1 || candidates[0].Name != "available" {
		t.Fatalf("expected cooldown channel to be skipped, got %#v", candidates)
	}
	if _, err := router.Route(context.Background(), "gpt-test", DesktopRoutePriority, func(_ context.Context, _ *DesktopChannel) error {
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

func TestDesktopFailureClassification(t *testing.T) {
	info := DesktopClassifyStatus(429)
	if info.Class != DesktopFailureRateLimit {
		t.Fatalf("expected rate limit classification, got %#v", info)
	}
	if DesktopFailureCooldown(info, 1) != 30*time.Second {
		t.Fatalf("unexpected default rate limit cooldown")
	}
	serverInfo := DesktopClassifyStatus(503)
	if !DesktopFailureIsCircuitEligible(serverInfo) {
		t.Fatal("expected 503 to be circuit eligible")
	}
}

func TestDesktopChannelRouterCanonicalMatching(t *testing.T) {
	db, err := sql.Open("sqlite", "file:desktop_router_canonical?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	store, err := NewDesktopChannelStore(db)
	if err != nil {
		t.Fatal(err)
	}

	// Channel 1: Nexus has "glm-5.3-flash-free"
	nexus := &DesktopChannel{
		Name:         "Nexus",
		Provider:     MonitorProviderOpenAI,
		Endpoint:     "https://nexus.example",
		APIKey:       "key-nexus",
		PrimaryModel: "glm-5.3-flash-free",
		Enabled:      true,
		Priority:     1,
		LastStatus:   MonitorStatusOperational,
	}
	// Channel 2: B.AI has "glm-5.3-flash"
	bai := &DesktopChannel{
		Name:         "B.AI",
		Provider:     MonitorProviderOpenAI,
		Endpoint:     "https://bai.example",
		APIKey:       "key-bai",
		PrimaryModel: "glm-5.3-flash",
		Enabled:      true,
		Priority:     2,
		LastStatus:   MonitorStatusOperational,
	}
	// Channel 3: 芝公益站 has "z-ai/glm-5.3-free"
	zhi := &DesktopChannel{
		Name:         "芝公益站",
		Provider:     MonitorProviderOpenAI,
		Endpoint:     "https://zhi.example",
		APIKey:       "key-zhi",
		PrimaryModel: "z-ai/glm-5.3-free",
		Enabled:      true,
		Priority:     3,
		LastStatus:   MonitorStatusOperational,
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

	router := NewDesktopChannelRouter(store)

	// Client requests standard name "glm-5.3-flash"
	candidates, err := router.Candidates(context.Background(), "glm-5.3-flash", DesktopRoutePriority)
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
