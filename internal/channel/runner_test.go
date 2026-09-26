package channel

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/Heiheiaoo/heigate/internal/probe"
	"github.com/stretchr/testify/require"
	_ "modernc.org/sqlite"
)

var sumPattern = regexp.MustCompile(`(\d+) \+ (\d+)`)

func newTestStore(t *testing.T, name string) *Store {
	t.Helper()
	db, err := sql.Open("sqlite", "file:"+name+"?mode=memory&cache=shared")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = db.Close() })
	store, err := NewStore(db)
	if err != nil {
		t.Fatal(err)
	}
	return store
}

// A channel whose probes keep failing must end up with an open circuit and a
// cooldown, the same bookkeeping the transport-failure and forward paths do.
// Without it the channel stays a routing candidate and is retried forever.
func TestProbeNowOpensCircuitAfterRepeatedFailures(t *testing.T) {
	// 404 is not retried, so each ProbeNow is exactly one upstream call.
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		w.WriteHeader(http.StatusNotFound)
		_, _ = io.WriteString(w, `{"error":"model not found"}`)
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_runner_circuit_test")
	ch := &Channel{
		Name: "failing", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "sk-test", PrimaryModel: "m", Enabled: true, IntervalSeconds: 0,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	runner := NewProbeRunner(store)
	defer runner.Stop()

	for i := 1; i <= 3; i++ {
		if _, err := runner.ProbeNow(context.Background(), ch.ID); err != nil {
			t.Fatalf("probe %d: %v", i, err)
		}
		loaded, err := store.Get(context.Background(), ch.ID)
		if err != nil {
			t.Fatal(err)
		}
		if loaded.FailureCount != i {
			t.Fatalf("probe %d: failure count = %d, want %d", i, loaded.FailureCount, i)
		}
		if loaded.CooldownUntil == nil {
			t.Fatalf("probe %d: expected a cooldown to be set", i)
		}
		wantState := CircuitClosed
		if i >= 3 {
			wantState = CircuitOpen
		}
		if loaded.CircuitState != wantState {
			t.Fatalf("probe %d: circuit = %q, want %q", i, loaded.CircuitState, wantState)
		}
	}
}

// A recovered channel clears the failure bookkeeping so routing can use it again.
func TestProbeNowSuccessClearsCircuit(t *testing.T) {
	var failing bool = true
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		if failing {
			w.WriteHeader(http.StatusNotFound)
			_, _ = io.WriteString(w, `{"error":"model not found"}`)
			return
		}
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+fmt.Sprintf(`{"choices":[{"delta":{"content":%q}}]}`, sumFromProbeBody(t, body))+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_runner_recovery_test")
	ch := &Channel{
		Name: "recovering", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "sk-test", PrimaryModel: "m", Enabled: true, IntervalSeconds: 0,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	runner := NewProbeRunner(store)
	defer runner.Stop()

	for i := 0; i < 3; i++ {
		_, _ = runner.ProbeNow(context.Background(), ch.ID)
	}
	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.CircuitState != CircuitOpen {
		t.Fatalf("setup: circuit = %q, want open", loaded.CircuitState)
	}

	failing = false
	if _, err := runner.ProbeNow(context.Background(), ch.ID); err != nil {
		t.Fatal(err)
	}
	loaded, err = store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.LastStatus != probe.StatusOperational {
		t.Fatalf("status = %q, want operational", loaded.LastStatus)
	}
	if loaded.FailureCount != 0 {
		t.Fatalf("failure count = %d, want 0", loaded.FailureCount)
	}
	if loaded.CircuitState != CircuitClosed {
		t.Fatalf("circuit = %q, want closed", loaded.CircuitState)
	}
	if loaded.CooldownUntil != nil {
		t.Fatal("expected cooldown to be cleared")
	}
}

// sumFromProbeBody extracts "a + b" from a probe request body and returns the
// sum, so a stub upstream can answer the challenge correctly.
func sumFromProbeBody(t *testing.T, body []byte) string {
	t.Helper()
	var req struct {
		Messages []struct {
			Content string `json:"content"`
		} `json:"messages"`
	}
	if err := json.Unmarshal(body, &req); err != nil || len(req.Messages) == 0 {
		t.Fatalf("unexpected probe body: %s", body)
	}
	m := sumPattern.FindStringSubmatch(req.Messages[0].Content)
	if len(m) != 3 {
		t.Fatalf("no addition found in probe prompt: %q", req.Messages[0].Content)
	}
	a, _ := strconv.Atoi(m[1])
	b, _ := strconv.Atoi(m[2])
	return strconv.Itoa(a + b)
}

// A probe holds its channel snapshot for tens of seconds. If the user edits the
// channel (e.g. removes models) while a probe is in flight, the probe result
// must not write the stale snapshot back and resurrect the removed models.
func TestProbeNowDoesNotClobberConcurrentEdit(t *testing.T) {
	store := newTestStore(t, "desktop_probe_race_test")
	ch := &Channel{
		Name: "racy", Provider: probe.ProviderOpenAI,
		Endpoint: "placeholder", APIKey: "sk-test",
		PrimaryModel: "old-model", ExtraModels: []string{"old-extra"},
		Enabled: true, IntervalSeconds: 0,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		// The user edits the channel while the probe request is in flight.
		edited, err := store.Get(context.Background(), ch.ID)
		if err != nil {
			t.Errorf("edit: %v", err)
			return
		}
		edited.PrimaryModel = "new-model"
		edited.ExtraModels = []string{"new-extra"}
		if err := store.Save(context.Background(), edited); err != nil {
			t.Errorf("edit save: %v", err)
			return
		}
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+fmt.Sprintf(`{"choices":[{"delta":{"content":%q}}]}`, sumFromProbeBody(t, body))+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	// Point the channel at the stub server only now, so the probe sees it but
	// the "user edit" above happens against the already-saved row.
	ch.Endpoint = server.URL
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	runner := NewProbeRunner(store)
	defer runner.Stop()

	if _, err := runner.ProbeNow(context.Background(), ch.ID); err != nil {
		t.Fatal(err)
	}

	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.PrimaryModel != "new-model" || len(loaded.ExtraModels) != 1 || loaded.ExtraModels[0] != "new-extra" {
		t.Fatalf("probe resurrected stale models: primary=%q extra=%v", loaded.PrimaryModel, loaded.ExtraModels)
	}
	if loaded.LastStatus != "unknown" {
		t.Fatalf("status = %q, want unknown: an old primary's result must not be assigned to the newly selected model", loaded.LastStatus)
	}
}

func TestProbeNowChecksOnlyPrimaryUnlessDeepScanRequested(t *testing.T) {
	var requested []string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		body, _ := io.ReadAll(r.Body)
		var req struct {
			Model string `json:"model"`
		}
		_ = json.Unmarshal(body, &req)
		requested = append(requested, req.Model)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+fmt.Sprintf(`{"choices":[{"delta":{"content":%q}}]}`, sumFromProbeBody(t, body))+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_primary_only_probe_test")
	ch := &Channel{
		Name: "lean", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "k", PrimaryModel: "primary", ExtraModels: []string{"extra-a", "extra-b"},
		Enabled: true, IntervalSeconds: 300,
	}
	require.NoError(t, store.Save(context.Background(), ch))
	runner := NewProbeRunner(store)
	defer runner.Stop()

	_, err := runner.ProbeNow(context.Background(), ch.ID)
	require.NoError(t, err)
	require.Equal(t, []string{"primary"}, requested)

	requested = nil
	_, err = runner.ProbeAllModelsNow(context.Background(), ch.ID)
	require.NoError(t, err)
	require.Equal(t, []string{"primary", "extra-a", "extra-b"}, requested)
}

func TestProbeModelNowExtraDoesNotChangeChannelHealth(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		w.WriteHeader(http.StatusNotFound)
		_, _ = io.WriteString(w, `{"error":{"message":"model not found"}}`)
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_extra_model_probe_test")
	ch := &Channel{
		Name: "extra", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "k", PrimaryModel: "primary", ExtraModels: []string{"broken-extra"},
		Enabled: true, IntervalSeconds: 300, LastStatus: probe.StatusOperational,
	}
	require.NoError(t, store.Save(context.Background(), ch))
	runner := NewProbeRunner(store)
	defer runner.Stop()

	results, err := runner.ProbeModelNow(context.Background(), ch.ID, "broken-extra")
	require.NoError(t, err)
	require.Len(t, results, 1)
	require.Equal(t, probe.StatusError, results[0].Status)

	loaded, err := store.Get(context.Background(), ch.ID)
	require.NoError(t, err)
	require.Equal(t, probe.StatusOperational, loaded.LastStatus)
	require.Zero(t, loaded.FailureCount)
	require.Nil(t, loaded.CooldownUntil)
}

func TestProbeMethodsShareOneChannelInFlightSlot(t *testing.T) {
	started := make(chan struct{})
	release := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		body, _ := io.ReadAll(r.Body)
		close(started)
		<-release
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+fmt.Sprintf(`{"choices":[{"delta":{"content":%q}}]}`, sumFromProbeBody(t, body))+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_probe_inflight_test")
	ch := &Channel{
		Name: "locked", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "k", PrimaryModel: "primary", ExtraModels: []string{"extra"},
		Enabled: true, IntervalSeconds: 300,
	}
	require.NoError(t, store.Save(context.Background(), ch))
	runner := NewProbeRunner(store)
	defer runner.Stop()

	done := make(chan error, 1)
	go func() {
		_, err := runner.ProbeNow(context.Background(), ch.ID)
		done <- err
	}()
	<-started

	results, err := runner.ProbeModelNow(context.Background(), ch.ID, "extra")
	require.ErrorIs(t, err, ErrProbeBusy)
	require.Empty(t, results)

	close(release)
	require.NoError(t, <-done)
}

// Only a fully failed probe clears the streak. A degraded channel is still
// serving, so it must not lose backoff progress a clean probe earned.
func TestProbeStreakSurvivesDegradedButNotFailure(t *testing.T) {
	var slow bool
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		body, _ := io.ReadAll(r.Body)
		if slow {
			time.Sleep(80 * time.Millisecond)
		}
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+fmt.Sprintf(`{"choices":[{"delta":{"content":%q}}]}`, sumFromProbeBody(t, body))+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	old := probe.DegradedThreshold
	probe.DegradedThreshold = 50 * time.Millisecond
	defer func() { probe.DegradedThreshold = old }()

	store := newTestStore(t, "desktop_probe_streak_degraded_test")
	ch := &Channel{
		Name: "flappy", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "k", PrimaryModel: "m", Enabled: true, IntervalSeconds: 300,
		FailureCount: 53, CircuitState: CircuitOpen,
	}
	cooldown := time.Now().Add(time.Hour)
	ch.CooldownUntil = &cooldown
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}
	runner := NewProbeRunner(store)
	defer runner.Stop()

	// Two clean probes earn a streak of 2.
	for i := 0; i < 2; i++ {
		if _, err := runner.ProbeNow(context.Background(), ch.ID); err != nil {
			t.Fatal(err)
		}
	}
	// A slow (degraded) probe leaves it alone.
	slow = true
	if _, err := runner.ProbeNow(context.Background(), ch.ID); err != nil {
		t.Fatal(err)
	}
	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.LastStatus != probe.StatusDegraded {
		t.Fatalf("setup: status = %q, want degraded", loaded.LastStatus)
	}
	if loaded.ProbeStreak != 2 {
		t.Fatalf("degraded probe changed the streak to %d, want it held at 2", loaded.ProbeStreak)
	}
	if loaded.FailureCount != 0 || loaded.CircuitState != CircuitClosed || loaded.CooldownUntil != nil {
		t.Fatalf("successful but slow probe left stale circuit state: failures=%d circuit=%q cooldown=%v", loaded.FailureCount, loaded.CircuitState, loaded.CooldownUntil)
	}
}

// The streak is what drives backoff, so a failing probe must clear it.
func TestProbeStreakResetsOnFailure(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		w.WriteHeader(http.StatusNotFound)
		_, _ = io.WriteString(w, `{"error":"model not found"}`)
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_probe_streak_test")
	ch := &Channel{
		Name: "streaky", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "k", PrimaryModel: "m", Enabled: true, IntervalSeconds: 300,
		ProbeStreak: 12,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	runner := NewProbeRunner(store)
	defer runner.Stop()
	if _, err := runner.ProbeNow(context.Background(), ch.ID); err != nil {
		t.Fatal(err)
	}

	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.ProbeStreak != 0 {
		t.Fatalf("failing probe left streak at %d, want 0", loaded.ProbeStreak)
	}
}

// A clean probe run still records successful health samples.
func TestProbeStreakRisesAndStretchesInterval(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+fmt.Sprintf(`{"choices":[{"delta":{"content":%q}}]}`, sumFromProbeBody(t, body))+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_probe_streak_rise_test")
	ch := &Channel{
		Name: "steady", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "k", PrimaryModel: "m", Enabled: true, IntervalSeconds: 300,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	runner := NewProbeRunner(store)
	defer runner.Stop()

	for i := 1; i <= 5; i++ {
		if _, err := runner.ProbeNow(context.Background(), ch.ID); err != nil {
			t.Fatal(err)
		}
	}
	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.ProbeStreak != 5 {
		t.Fatalf("streak = %d, want 5", loaded.ProbeStreak)
	}
}

// The channel's status describes its primary model. A broken extra model used
// to drag the whole channel to degraded, which cost it its place in the
// healthiest routing bucket and blocked adaptive backoff.
func TestPrimaryModelDrivesChannelStatus(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		var req struct {
			Model string `json:"model"`
		}
		body, _ := io.ReadAll(r.Body)
		_ = json.Unmarshal(body, &req)
		if req.Model != "primary" {
			w.WriteHeader(http.StatusNotFound)
			_, _ = io.WriteString(w, `{"error":{"message":"model not found"}}`)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+fmt.Sprintf(`{"choices":[{"delta":{"content":%q}}]}`, sumFromProbeBody(t, body))+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_primary_status_test")
	ch := &Channel{
		Name: "mixed", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "k", PrimaryModel: "primary", ExtraModels: []string{"stale-model"},
		Enabled: true, IntervalSeconds: 300,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	runner := NewProbeRunner(store)
	defer runner.Stop()
	if _, err := runner.ProbeAllModelsNow(context.Background(), ch.ID); err != nil {
		t.Fatal(err)
	}

	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.LastStatus != probe.StatusOperational {
		t.Fatalf("status = %q, want operational: a working primary must not be dragged down by a stale extra model", loaded.LastStatus)
	}
	if loaded.ProbeStreak != 1 {
		t.Fatalf("streak = %d, want 1: a working primary must earn adaptive backoff", loaded.ProbeStreak)
	}
	if !strings.Contains(loaded.LastError, "stale-model") {
		t.Fatalf("the broken extra model must still be reported, got %q", loaded.LastError)
	}
}

// The primary is the default routing target, so a dead primary benches the
// whole channel as error even when a backup model still answers: a request that
// lands on the primary would fail, and latency is cleared to keep it out of the
// healthy routing bucket.
func TestDeadPrimaryMarksChannelError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		body, _ := io.ReadAll(r.Body)
		var req struct {
			Model string `json:"model"`
		}
		_ = json.Unmarshal(body, &req)
		if req.Model == "primary" {
			w.WriteHeader(http.StatusNotFound)
			_, _ = io.WriteString(w, `{"error":{"message":"model not found"}}`)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: "+fmt.Sprintf(`{"choices":[{"delta":{"content":%q}}]}`, sumFromProbeBody(t, body))+"\n\n")
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_dead_primary_test")
	ch := &Channel{
		Name: "backup", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "k", PrimaryModel: "primary", ExtraModels: []string{"backup-model"},
		Enabled: true, IntervalSeconds: 300,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	runner := NewProbeRunner(store)
	defer runner.Stop()
	if _, err := runner.ProbeAllModelsNow(context.Background(), ch.ID); err != nil {
		t.Fatal(err)
	}

	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.LastStatus != probe.StatusError {
		t.Fatalf("status = %q, want error when primary model fails", loaded.LastStatus)
	}
	if loaded.LastLatencyMs != nil {
		t.Fatalf("latency = %v, want nil when primary model fails", loaded.LastLatencyMs)
	}
}

// With nothing usable the channel is failed and benched, as before.
func TestAllModelsDeadIsError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodHead {
			return
		}
		w.WriteHeader(http.StatusNotFound)
		_, _ = io.WriteString(w, `{"error":{"message":"model not found"}}`)
	}))
	defer server.Close()

	store := newTestStore(t, "desktop_all_dead_test")
	ch := &Channel{
		Name: "dead", Provider: probe.ProviderOpenAI, Endpoint: server.URL,
		APIKey: "k", PrimaryModel: "primary", ExtraModels: []string{"extra"},
		Enabled: true, IntervalSeconds: 300,
	}
	if err := store.Save(context.Background(), ch); err != nil {
		t.Fatal(err)
	}

	runner := NewProbeRunner(store)
	defer runner.Stop()
	if _, err := runner.ProbeAllModelsNow(context.Background(), ch.ID); err != nil {
		t.Fatal(err)
	}

	loaded, err := store.Get(context.Background(), ch.ID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.LastStatus != probe.StatusError {
		t.Fatalf("status = %q, want error when nothing serves", loaded.LastStatus)
	}
	if loaded.CooldownUntil == nil {
		t.Fatal("a channel with nothing usable should be benched")
	}
}
