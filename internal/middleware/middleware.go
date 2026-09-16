// Package middleware contains the small set of HTTP guards HeiGate needs as
// a local-only desktop service: panic recovery, conservative security
// headers, and a local-origin policy that keeps web pages running in the
// user's browser from talking to the management API.
package middleware

import (
	"crypto/subtle"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"runtime/debug"
	"strings"

	"github.com/gin-gonic/gin"
)

// RequestHeader must be present on every state-changing management request.
// Browsers cannot attach a custom header to a cross-origin request without a
// CORS preflight, and HeiGate never answers preflights, so a malicious page
// cannot forge such a request.
const RequestHeader = "X-HeiGate-Request"

// Recovery turns panics into 500 responses instead of crashing the process.
func Recovery() gin.HandlerFunc {
	return func(c *gin.Context) {
		defer func() {
			if r := recover(); r != nil {
				slog.Error("panic recovered", "path", c.Request.URL.Path, "error", r, "stack", string(debug.Stack()))
				if !c.Writer.Written() {
					c.AbortWithStatusJSON(http.StatusInternalServerError, gin.H{"error": "internal server error"})
				} else {
					c.Abort()
				}
			}
		}()
		c.Next()
	}
}

// SecurityHeaders sets a strict content security policy suited to the
// embedded single-page UI. Nothing is loaded from the network.
func SecurityHeaders() gin.HandlerFunc {
	const csp = "default-src 'self'; " +
		"script-src 'self'; " +
		"style-src 'self' 'unsafe-inline'; " +
		"img-src 'self' data: blob:; " +
		"font-src 'self' data:; " +
		"connect-src 'self'; " +
		"worker-src 'self' blob:; " +
		"frame-ancestors 'none'; " +
		"base-uri 'self'; " +
		"form-action 'self'"
	return func(c *gin.Context) {
		h := c.Writer.Header()
		h.Set("Content-Security-Policy", csp)
		h.Set("X-Content-Type-Options", "nosniff")
		h.Set("X-Frame-Options", "DENY")
		h.Set("Referrer-Policy", "no-referrer")
		h.Set("Cross-Origin-Opener-Policy", "same-origin")
		h.Set("Cross-Origin-Resource-Policy", "same-origin")
		c.Next()
	}
}

// LoopbackOnly rejects connections that did not originate on this machine
// and requests whose Host header does not name a loopback address. The
// second check defeats DNS rebinding: a page on attacker.example that
// resolves to 127.0.0.1 still arrives with Host: attacker.example.
func LoopbackOnly() gin.HandlerFunc {
	return func(c *gin.Context) {
		if !rejectNonLocal(c) {
			c.Next()
		}
	}
}

// rejectNonLocal aborts the request when it is not local; it reports true
// when the request was aborted.
func rejectNonLocal(c *gin.Context) bool {
	if !isLoopbackAddr(c.Request.RemoteAddr) {
		c.AbortWithStatusJSON(http.StatusForbidden, gin.H{"error": "HeiGate only accepts local connections"})
		return true
	}
	if !isLoopbackHost(c.Request.Host) {
		c.AbortWithStatusJSON(http.StatusForbidden, gin.H{"error": "unexpected Host header"})
		return true
	}
	return false
}

// SameOriginOnly is the browser-facing guard for the management API. It
// combines LoopbackOnly with an Origin / Sec-Fetch-Site check and requires
// RequestHeader on every non-idempotent request.
func SameOriginOnly() gin.HandlerFunc {
	return func(c *gin.Context) {
		if rejectNonLocal(c) {
			return
		}
		if origin := c.GetHeader("Origin"); origin != "" && origin != "null" {
			u, err := url.Parse(origin)
			if err != nil || !isLoopbackHost(u.Host) {
				c.AbortWithStatusJSON(http.StatusForbidden, gin.H{"error": "cross-origin requests are not allowed"})
				return
			}
		}
		switch strings.ToLower(c.GetHeader("Sec-Fetch-Site")) {
		case "cross-site", "same-site":
			c.AbortWithStatusJSON(http.StatusForbidden, gin.H{"error": "cross-origin requests are not allowed"})
			return
		}
		switch c.Request.Method {
		case http.MethodGet, http.MethodHead, http.MethodOptions:
		default:
			if strings.TrimSpace(c.GetHeader(RequestHeader)) == "" {
				c.AbortWithStatusJSON(http.StatusForbidden, gin.H{"error": "missing " + RequestHeader + " header"})
				return
			}
		}
		c.Next()
	}
}

// BearerAuth accepts the gateway key via "Authorization: Bearer <key>" or
// "x-api-key: <key>", matching what Anthropic and OpenAI SDKs send.
func BearerAuth(key string) gin.HandlerFunc {
	key = strings.TrimSpace(key)
	return func(c *gin.Context) {
		if key == "" {
			c.AbortWithStatusJSON(http.StatusInternalServerError, gin.H{"error": "gateway key is not configured"})
			return
		}
		provided := strings.TrimSpace(c.GetHeader("x-api-key"))
		if provided == "" {
			auth := strings.TrimSpace(c.GetHeader("Authorization"))
			if len(auth) > 7 && strings.EqualFold(auth[:7], "Bearer ") {
				provided = strings.TrimSpace(auth[7:])
			}
		}
		if provided == "" || subtle.ConstantTimeCompare([]byte(provided), []byte(key)) != 1 {
			c.Header("WWW-Authenticate", `Bearer realm="HeiGate"`)
			c.AbortWithStatusJSON(http.StatusUnauthorized, gin.H{
				"type":  "error",
				"error": gin.H{"type": "authentication_error", "message": "invalid HeiGate gateway API key"},
			})
			return
		}
		c.Next()
	}
}

func isLoopbackAddr(remoteAddr string) bool {
	host, _, err := net.SplitHostPort(strings.TrimSpace(remoteAddr))
	if err != nil {
		host = strings.TrimSpace(remoteAddr)
	}
	ip := net.ParseIP(strings.Trim(host, "[]"))
	return ip != nil && ip.IsLoopback()
}

func isLoopbackHost(hostport string) bool {
	hostport = strings.TrimSpace(hostport)
	if hostport == "" {
		return false
	}
	host := hostport
	if h, _, err := net.SplitHostPort(hostport); err == nil {
		host = h
	}
	host = strings.Trim(strings.ToLower(host), "[]")
	if host == "localhost" || strings.HasSuffix(host, ".localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}
