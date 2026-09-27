package api

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"os"
	"runtime"
	"strings"
)

// LoadOrCreateGatewayKey keeps the local gateway credential outside the
// database and restricts the file to the current user.
func LoadOrCreateGatewayKey(path string) (string, error) {
	if data, err := os.ReadFile(path); err == nil {
		key := strings.TrimSpace(string(data))
		if key != "" {
			if runtime.GOOS != "windows" {
				if info, statErr := os.Stat(path); statErr != nil {
					return "", fmt.Errorf("stat desktop gateway key: %w", statErr)
				} else if info.Mode().Perm() != 0o600 {
					if chmodErr := os.Chmod(path, 0o600); chmodErr != nil {
						return "", fmt.Errorf("restrict desktop gateway key: %w", chmodErr)
					}
				}
			}
			return key, nil
		}
	} else if !os.IsNotExist(err) {
		return "", fmt.Errorf("read desktop gateway key: %w", err)
	}
	buf := make([]byte, 32)
	if _, err := rand.Read(buf); err != nil {
		return "", fmt.Errorf("generate desktop gateway key: %w", err)
	}
	key := "hg_" + hex.EncodeToString(buf)
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
