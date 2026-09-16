// Command heigate runs the HeiGate local AI gateway: a single process that
// serves the desktop UI, a local management API, and Anthropic/OpenAI
// compatible endpoints that route requests to the user's configured
// upstream channels. All state lives in a SQLite file under the data
// directory; nothing is sent anywhere except to the channels the user adds.
package main

import (
	"context"
	_ "embed"
	"errors"
	"flag"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/gin-gonic/gin"

	"github.com/Heiheiaoo/heigate/internal/api"
	"github.com/Heiheiaoo/heigate/internal/channel"
	"github.com/Heiheiaoo/heigate/internal/datadir"
	"github.com/Heiheiaoo/heigate/internal/middleware"
	"github.com/Heiheiaoo/heigate/internal/web"
)

//go:embed VERSION
var embeddedVersion string

// version may be overridden at link time: -ldflags "-X main.version=1.2.3".
var version = ""

const (
	envHost = "HEIGATE_HOST"
	envPort = "HEIGATE_PORT"

	defaultHost = "127.0.0.1"
	defaultPort = 8080

	logPruneInterval = 15 * time.Minute
	shutdownTimeout  = 5 * time.Second
)

func main() {
	if strings.TrimSpace(version) == "" {
		version = strings.TrimSpace(embeddedVersion)
	}
	if version == "" {
		version = "0.0.0-dev"
	}

	var (
		showVersion = flag.Bool("version", false, "print the version and exit")
		host        = flag.String("host", envOr(envHost, defaultHost), "address to listen on (loopback only is strongly recommended)")
		port        = flag.Int("port", envInt(envPort, defaultPort), "TCP port to listen on; 0 picks a free port")
		dataDir     = flag.String("data-dir", "", "override the data directory (default: "+datadir.EnvVar+" or the platform application support folder)")
	)
	flag.Parse()

	if *showVersion {
		fmt.Printf("HeiGate %s\n", version)
		return
	}
	if *dataDir != "" {
		_ = os.Setenv(datadir.EnvVar, *dataDir)
	}
	if err := run(*host, *port); err != nil {
		slog.Error("heigate exited with error", "error", err)
		os.Exit(1)
	}
}

func run(host string, port int) error {
	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo}))
	slog.SetDefault(logger)

	dir, err := datadir.Ensure()
	if err != nil {
		return fmt.Errorf("create data directory: %w", err)
	}

	store, err := channel.OpenStore(datadir.DatabasePath())
	if err != nil {
		return fmt.Errorf("open database: %w", err)
	}
	defer func() { _ = store.Close() }()

	if deleted, err := store.AutoPruneLogs(context.Background()); err == nil && deleted > 0 {
		slog.Info("pruned request logs on startup", "deleted", deleted)
	}

	runCtx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go pruneLoop(runCtx, store)

	runner := channel.NewProbeRunner(store)
	if err := runner.Start(runCtx); err != nil {
		slog.Warn("probe scheduler started in degraded state", "error", err)
	}
	defer runner.Stop()

	gatewayKey, err := api.LoadOrCreateGatewayKey(datadir.GatewayKeyPath())
	if err != nil {
		return fmt.Errorf("initialise gateway key: %w", err)
	}

	listener, err := net.Listen("tcp", net.JoinHostPort(host, strconv.Itoa(port)))
	if err != nil {
		return fmt.Errorf("listen: %w", err)
	}
	boundPort := listener.Addr().(*net.TCPAddr).Port
	baseURL := "http://" + net.JoinHostPort(host, strconv.Itoa(boundPort))

	handler := api.NewHandlerWithGatewayKey(store, runner, gatewayKey)
	handler.SetGatewayBaseURL(baseURL)

	gin.SetMode(gin.ReleaseMode)
	router := newRouter(handler, gatewayKey)

	server := &http.Server{
		Handler:           router,
		ReadHeaderTimeout: 30 * time.Second,
		IdleTimeout:       120 * time.Second,
	}

	errCh := make(chan error, 1)
	go func() {
		errCh <- server.Serve(listener)
	}()

	slog.Info("HeiGate started",
		"version", version,
		"url", baseURL,
		"data_dir", dir,
		"ui_embedded", web.Available(),
	)
	if !web.Available() {
		slog.Warn("no embedded UI in this build; the management UI will not be served")
	}

	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)

	select {
	case sig := <-quit:
		slog.Info("shutting down", "signal", sig.String())
	case err := <-errCh:
		if err != nil && !errors.Is(err, http.ErrServerClosed) {
			return fmt.Errorf("serve: %w", err)
		}
		return nil
	}

	cancel()
	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), shutdownTimeout)
	defer shutdownCancel()
	if err := server.Shutdown(shutdownCtx); err != nil {
		slog.Warn("forced shutdown", "error", err)
	}
	return nil
}

// newRouter wires the HTTP surface: the health endpoint the desktop shell
// polls, the local management API, and the gateway endpoints.
func newRouter(handler *api.Handler, gatewayKey string) *gin.Engine {
	router := gin.New()
	router.Use(middleware.Recovery(), middleware.SecurityHeaders())
	router.GET("/health", func(c *gin.Context) {
		// The macOS shell polls this endpoint and requires mode=="desktop" so a
		// different local service bound to the same port is not mistaken for
		// the gateway. Dropping this field hangs app startup until it times out.
		c.JSON(http.StatusOK, gin.H{"status": "ok", "mode": "desktop", "version": version})
	})
	handler.RegisterRoutes(router.Group("/api", middleware.SameOriginOnly()))
	handler.RegisterGatewayRoutes(router.Group("", middleware.LoopbackOnly(), middleware.BearerAuth(gatewayKey)))
	router.NoRoute(web.Handler())
	return router
}

func pruneLoop(ctx context.Context, store *channel.Store) {
	ticker := time.NewTicker(logPruneInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			if deleted, err := store.AutoPruneLogs(ctx); err == nil && deleted > 0 {
				slog.Info("pruned request logs", "deleted", deleted)
			}
		}
	}
}

func envOr(key, fallback string) string {
	if v := strings.TrimSpace(os.Getenv(key)); v != "" {
		return v
	}
	return fallback
}

func envInt(key string, fallback int) int {
	if v := strings.TrimSpace(os.Getenv(key)); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n >= 0 && n <= 65535 {
			return n
		}
	}
	return fallback
}
