// Package datadir resolves where HeiGate keeps its local state (SQLite
// database, gateway key). The location can be overridden with DATA_DIR;
// otherwise it follows the platform convention for application support
// files.
package datadir

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// EnvVar is the environment variable that overrides the data directory.
const EnvVar = "DATA_DIR"

// AppName is the folder name used under the platform's application support
// directory.
const AppName = "HeiGate"

// Path returns the data directory. It never returns an empty string; when
// the home directory cannot be determined it falls back to a folder next to
// the current working directory.
func Path() string {
	if dir := strings.TrimSpace(os.Getenv(EnvVar)); dir != "" {
		return filepath.Clean(dir)
	}
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return filepath.Join(".", "heigate-data")
	}
	switch runtime.GOOS {
	case "darwin":
		return filepath.Join(home, "Library", "Application Support", AppName)
	case "windows":
		if appData := os.Getenv("APPDATA"); appData != "" {
			return filepath.Join(appData, AppName)
		}
		return filepath.Join(home, "AppData", "Roaming", AppName)
	default:
		if xdg := os.Getenv("XDG_DATA_HOME"); xdg != "" {
			return filepath.Join(xdg, strings.ToLower(AppName))
		}
		return filepath.Join(home, ".local", "share", strings.ToLower(AppName))
	}
}

// Ensure creates the data directory (0700) and returns it.
func Ensure() (string, error) {
	dir := Path()
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", err
	}
	return dir, nil
}

// DatabasePath is the SQLite file inside the data directory.
func DatabasePath() string {
	return filepath.Join(Path(), "heigate.sqlite")
}

// GatewayKeyPath is the file holding the local gateway API key.
func GatewayKeyPath() string {
	return filepath.Join(Path(), "gateway.key")
}
