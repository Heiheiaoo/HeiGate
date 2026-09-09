package desktopapi

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"net/http"
	"os"
	"strings"

	"github.com/gin-gonic/gin"
)

// LoadOrCreateGatewayKey keeps the local gateway credential outside the
// database and restricts the file to the current user.
func LoadOrCreateGatewayKey(path string) (string, error) {
	if data, err := os.ReadFile(path); err == nil {
		key := strings.TrimSpace(string(data))
		if key != "" {
			return key, nil
		}
	} else if !os.IsNotExist(err) {
		return "", fmt.Errorf("read desktop gateway key: %w", err)
	}
	buf := make([]byte, 32)
	if _, err := rand.Read(buf); err != nil {
		return "", fmt.Errorf("generate desktop gateway key: %w", err)
	}
	key := "s2d_" + hex.EncodeToString(buf)
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		if os.IsExist(err) {
			data, readErr := os.ReadFile(path)
			if readErr != nil {
				return "", fmt.Errorf("read concurrent desktop gateway key: %w", readErr)
			}
			return strings.TrimSpace(string(data)), nil
		}
		return "", fmt.Errorf("create desktop gateway key: %w", err)
	}
	if _, err := file.WriteString(key + "\n"); err != nil {
		_ = file.Close()
		return "", fmt.Errorf("write desktop gateway key: %w", err)
	}
	if err := file.Close(); err != nil {
		return "", fmt.Errorf("close desktop gateway key: %w", err)
	}
	return key, nil
}

func GatewayAuth(key string) gin.HandlerFunc {
	return func(c *gin.Context) {
		if strings.TrimSpace(key) == "" {
			c.AbortWithStatusJSON(http.StatusInternalServerError, gin.H{"error": "desktop gateway key is not configured"})
			return
		}
		provided := strings.TrimSpace(c.GetHeader("x-api-key"))
		if provided == "" {
			provided = strings.TrimSpace(strings.TrimPrefix(c.GetHeader("Authorization"), "Bearer "))
		}
		if provided == "" || provided != key {
			c.AbortWithStatusJSON(http.StatusUnauthorized, gin.H{"error": "invalid desktop gateway API key"})
			return
		}
		c.Next()
	}
}
