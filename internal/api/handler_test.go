package api

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/Heiheiaoo/heigate/internal/channel"
	"github.com/Heiheiaoo/heigate/internal/probe"
	"github.com/gin-gonic/gin"
	"github.com/stretchr/testify/require"
)

// writeError maps typed sentinel errors to status codes. A channel-not-found
// (possibly wrapped) must be 404, validation sentinels must be 400, and any
// other error must be 500 — previously the mapping matched on message text, so
// e.g. a driver error mentioning "invalid" was wrongly returned as 400.
func TestWriteErrorMapsSentinelsToStatus(t *testing.T) {
	gin.SetMode(gin.TestMode)
	cases := []struct {
		name string
		err  error
		want int
	}{
		{"channel not found", channel.ErrChannelNotFound, http.StatusNotFound},
		{"wrapped channel not found", fmt.Errorf("desktop channel 7: %w", channel.ErrChannelNotFound), http.StatusNotFound},
		{"invalid provider", probe.ErrInvalidProvider, http.StatusBadRequest},
		{"invalid api mode", probe.ErrInvalidAPIMode, http.StatusBadRequest},
		{"invalid endpoint", probe.ErrInvalidEndpoint, http.StatusBadRequest},
		{"missing primary model", probe.ErrMissingPrimaryModel, http.StatusBadRequest},
		{"plain db error", fmt.Errorf("insert desktop channel: disk I/O error"), http.StatusInternalServerError},
		{"message merely mentioning invalid", fmt.Errorf("invalid sqlite handle"), http.StatusInternalServerError},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			rec := httptest.NewRecorder()
			c, _ := gin.CreateTestContext(rec)
			writeError(c, tc.err)
			require.Equal(t, tc.want, rec.Code)
		})
	}
}
