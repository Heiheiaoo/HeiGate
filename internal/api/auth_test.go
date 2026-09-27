package api

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/stretchr/testify/require"
)

func TestLoadOrCreateGatewayKeyRestrictsExistingFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "gateway.key")
	require.NoError(t, os.WriteFile(path, []byte("hg_existing\n"), 0o644))
	key, err := LoadOrCreateGatewayKey(path)
	require.NoError(t, err)
	require.Equal(t, "hg_existing", key)
	if runtime.GOOS != "windows" {
		info, err := os.Stat(path)
		require.NoError(t, err)
		require.Equal(t, os.FileMode(0o600), info.Mode().Perm())
	}
}

func TestLoadOrCreateGatewayKeyCreatesUniqueKey(t *testing.T) {
	path := filepath.Join(t.TempDir(), "gateway.key")
	first, err := LoadOrCreateGatewayKey(path)
	require.NoError(t, err)
	second, err := LoadOrCreateGatewayKey(path)
	require.NoError(t, err)
	require.Equal(t, first, second)
	require.Len(t, first, len("hg_")+64)
}
