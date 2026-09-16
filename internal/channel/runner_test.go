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
	"testing"

	"github.com/Heiheiaoo/heigate/internal/probe"
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
	if loaded.LastStatus != probe.StatusOperational {
		t.Fatalf("status = %q, want operational", loaded.LastStatus)
	}
}
