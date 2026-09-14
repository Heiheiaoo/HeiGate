//go:build !embed

// Package web serves the compiled desktop UI when it is embedded with
// `go build -tags embed`. This file is the headless fallback used for plain
// builds and tests.
package web

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

// Available reports whether a built UI is embedded in this binary.
func Available() bool { return false }

// Handler returns a handler that explains how to get the UI.
func Handler() gin.HandlerFunc {
	return func(c *gin.Context) {
		if c.Request.Method != http.MethodGet && c.Request.Method != http.MethodHead {
			c.Next()
			return
		}
		c.String(http.StatusNotFound, "HeiGate UI is not embedded in this build. Run `make frontend` then rebuild with -tags embed.")
		c.Abort()
	}
}
