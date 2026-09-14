package middleware

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/gin-gonic/gin"
	"github.com/stretchr/testify/require"
)

func newRouter(guard gin.HandlerFunc) *gin.Engine {
	gin.SetMode(gin.TestMode)
	r := gin.New()
	g := r.Group("/api", guard)
	g.GET("/ping", func(c *gin.Context) { c.String(http.StatusOK, "pong") })
	g.POST("/write", func(c *gin.Context) { c.String(http.StatusOK, "written") })
	return r
}

func do(r *gin.Engine, method, path, remote, host string, headers map[string]string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(method, path, nil)
	req.RemoteAddr = remote
	req.Host = host
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	r.ServeHTTP(rec, req)
	return rec
}

func TestLoopbackOnly(t *testing.T) {
	r := newRouter(LoopbackOnly())
	require.Equal(t, 200, do(r, "GET", "/api/ping", "127.0.0.1:5000", "127.0.0.1:8080", nil).Code)
	require.Equal(t, 200, do(r, "GET", "/api/ping", "[::1]:5000", "localhost:8080", nil).Code)
	require.Equal(t, 200, do(r, "GET", "/api/ping", "127.0.0.1:5000", "[::1]:8080", nil).Code)
	require.Equal(t, 403, do(r, "GET", "/api/ping", "192.168.1.9:5000", "127.0.0.1:8080", nil).Code, "remote LAN address")
	require.Equal(t, 403, do(r, "GET", "/api/ping", "127.0.0.1:5000", "attacker.example:8080", nil).Code, "DNS rebinding host")
	require.Equal(t, 403, do(r, "GET", "/api/ping", "127.0.0.1:5000", "", nil).Code, "empty host")
}

func TestSameOriginOnly(t *testing.T) {
	r := newRouter(SameOriginOnly())
	local := "127.0.0.1:8080"

	// Plain GET from the embedded UI (no Origin on same-origin navigations) passes.
	require.Equal(t, 200, do(r, "GET", "/api/ping", "127.0.0.1:1", local, nil).Code)
	// Same-origin fetch with Origin header passes.
	require.Equal(t, 200, do(r, "GET", "/api/ping", "127.0.0.1:1", local, map[string]string{"Origin": "http://127.0.0.1:8080", "Sec-Fetch-Site": "same-origin"}).Code)
	// Vite dev server proxy: Origin is localhost on another port; still loopback.
	require.Equal(t, 200, do(r, "GET", "/api/ping", "127.0.0.1:1", local, map[string]string{"Origin": "http://localhost:5173"}).Code)

	// Cross-origin page: rejected by Origin.
	require.Equal(t, 403, do(r, "GET", "/api/ping", "127.0.0.1:1", local, map[string]string{"Origin": "https://evil.example"}).Code)
	// Cross-origin page: rejected by Sec-Fetch-Site even if Origin were stripped.
	require.Equal(t, 403, do(r, "GET", "/api/ping", "127.0.0.1:1", local, map[string]string{"Sec-Fetch-Site": "cross-site"}).Code)

	// Mutations need the custom header ("simple" cross-origin POSTs never carry it).
	require.Equal(t, 403, do(r, "POST", "/api/write", "127.0.0.1:1", local, map[string]string{"Content-Type": "text/plain"}).Code)
	require.Equal(t, 200, do(r, "POST", "/api/write", "127.0.0.1:1", local, map[string]string{RequestHeader: "1"}).Code)
	// Still subject to the loopback rules.
	require.Equal(t, 403, do(r, "POST", "/api/write", "10.0.0.2:1", local, map[string]string{RequestHeader: "1"}).Code)
}

func TestBearerAuth(t *testing.T) {
	r := newRouter(BearerAuth("hg_secret"))
	require.Equal(t, 401, do(r, "GET", "/api/ping", "127.0.0.1:1", "127.0.0.1:8080", nil).Code)
	require.Equal(t, 401, do(r, "GET", "/api/ping", "127.0.0.1:1", "127.0.0.1:8080", map[string]string{"Authorization": "Bearer wrong"}).Code)
	require.Equal(t, 200, do(r, "GET", "/api/ping", "127.0.0.1:1", "127.0.0.1:8080", map[string]string{"Authorization": "Bearer hg_secret"}).Code)
	require.Equal(t, 200, do(r, "GET", "/api/ping", "127.0.0.1:1", "127.0.0.1:8080", map[string]string{"Authorization": "bearer hg_secret"}).Code)
	require.Equal(t, 200, do(r, "GET", "/api/ping", "127.0.0.1:1", "127.0.0.1:8080", map[string]string{"x-api-key": "hg_secret"}).Code)

	empty := newRouter(BearerAuth(""))
	require.Equal(t, 500, do(empty, "GET", "/api/ping", "127.0.0.1:1", "127.0.0.1:8080", map[string]string{"x-api-key": "x"}).Code)
}

func TestRecovery(t *testing.T) {
	gin.SetMode(gin.TestMode)
	r := gin.New()
	r.Use(Recovery())
	r.GET("/boom", func(*gin.Context) { panic("kaboom") })
	rec := httptest.NewRecorder()
	r.ServeHTTP(rec, httptest.NewRequest("GET", "/boom", nil))
	require.Equal(t, 500, rec.Code)
	require.Contains(t, rec.Body.String(), "internal server error")
}

func TestSecurityHeaders(t *testing.T) {
	gin.SetMode(gin.TestMode)
	r := gin.New()
	r.Use(SecurityHeaders())
	r.GET("/", func(c *gin.Context) { c.String(200, "ok") })
	rec := httptest.NewRecorder()
	r.ServeHTTP(rec, httptest.NewRequest("GET", "/", nil))
	require.Contains(t, rec.Header().Get("Content-Security-Policy"), "frame-ancestors 'none'")
	require.Equal(t, "nosniff", rec.Header().Get("X-Content-Type-Options"))
}
