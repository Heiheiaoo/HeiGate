package main

import (
	"context"
	"encoding/json"
	"io"
	"net"
	"net/http"
	"testing"
	"time"

	"github.com/gin-gonic/gin"

	"github.com/Heiheiaoo/heigate/internal/api"
	"github.com/Heiheiaoo/heigate/internal/channel"
)

// The macOS shell polls /health and only loads the UI once the payload carries
// mode == "desktop". A gateway that answers 200 without that field looks
// healthy over HTTP but leaves the desktop app stuck on a startup-timeout
// dialog, so the contract is pinned against the real router.
func TestHealthReportsDesktopMode(t *testing.T) {
	gin.SetMode(gin.TestMode)
	t.Setenv("DATA_DIR", t.TempDir())

	store, err := channel.OpenStore(t.TempDir() + "/heigate.sqlite")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = store.Close() }()

	runner := channel.NewProbeRunner(store)
	defer runner.Stop()

	router := newRouter(api.NewHandlerWithGatewayKey(store, runner, "test-gateway-key"), "test-gateway-key")

	addr, shutdown := serveTestRouter(t, router)
	defer shutdown()

	body, status := getJSON(t, "http://"+addr+"/health")
	if status != http.StatusOK {
		t.Fatalf("status = %d, want 200", status)
	}
	if got := body["mode"]; got != "desktop" {
		t.Fatalf("mode = %v, want \"desktop\"; the desktop shell waits for this field", got)
	}
	if got := body["status"]; got != "ok" {
		t.Fatalf("status field = %v, want \"ok\"", got)
	}
}

func getJSON(t *testing.T, url string) (map[string]any, int) {
	t.Helper()
	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Get(url)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatal(err)
	}
	var out map[string]any
	_ = json.Unmarshal(raw, &out)
	return out, resp.StatusCode
}

func serveTestRouter(t *testing.T, router *gin.Engine) (string, func()) {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	server := &http.Server{Handler: router}
	go func() { _ = server.Serve(listener) }()
	return listener.Addr().String(), func() {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		_ = server.Shutdown(ctx)
	}
}
