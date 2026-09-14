//go:build embed

// Package web serves the compiled desktop UI that is embedded into the
// binary at build time (go build -tags embed). Without the tag the package
// reports that no UI is available so the gateway still starts headless.
package web

import (
	"embed"
	"io/fs"
	"net/http"
	"path"
	"strings"

	"github.com/gin-gonic/gin"
)

//go:embed all:dist
var assets embed.FS

// Available reports whether a built UI is embedded in this binary.
func Available() bool {
	if _, err := fs.Stat(assets, "dist/index.html"); err != nil {
		return false
	}
	return true
}

// Handler serves static files from the embedded build and falls back to
// index.html for any unknown path so client-side routing keeps working.
// Hashed assets under /assets/ are marked immutable; everything else is
// revalidated on each load.
func Handler() gin.HandlerFunc {
	root, err := fs.Sub(assets, "dist")
	if err != nil {
		return func(c *gin.Context) { c.Next() }
	}
	fileServer := http.FileServer(http.FS(root))
	return func(c *gin.Context) {
		if c.Request.Method != http.MethodGet && c.Request.Method != http.MethodHead {
			c.Next()
			return
		}
		p := path.Clean("/" + c.Request.URL.Path)
		if strings.HasPrefix(p, "/api/") || strings.HasPrefix(p, "/v1/") || p == "/health" {
			c.Next()
			return
		}
		if f, err := root.Open(strings.TrimPrefix(p, "/")); err == nil {
			info, statErr := f.Stat()
			_ = f.Close()
			if statErr == nil && !info.IsDir() {
				if strings.HasPrefix(p, "/assets/") {
					c.Header("Cache-Control", "public, max-age=31536000, immutable")
				} else {
					c.Header("Cache-Control", "no-cache")
				}
				fileServer.ServeHTTP(c.Writer, c.Request)
				c.Abort()
				return
			}
		}
		// SPA fallback
		c.Header("Cache-Control", "no-cache")
		c.Request.URL.Path = "/"
		fileServer.ServeHTTP(c.Writer, c.Request)
		c.Abort()
	}
}
