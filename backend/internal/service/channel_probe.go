package service

import (
	"context"
	"fmt"
	"net/url"
	"strings"
	"sync"
)

// ChannelProbeConfig is the protocol-neutral input required by the desktop
// gateway to run the same challenge probe used by HeiGate's channel monitor.
// Endpoint must be the provider origin (for example https://api.example.com),
// while the adapter appends the protocol-specific request path.
type ChannelProbeConfig struct {
	Provider     string
	APIMode      string
	Endpoint     string
	APIKey       string
	PrimaryModel string
	ExtraModels  []string
	ExtraHeaders map[string]string
}

func validateDesktopEndpoint(ep string) error {
	ep = strings.TrimSpace(ep)
	if ep == "" {
		return ErrChannelMonitorInvalidEndpoint
	}
	if !strings.HasPrefix(ep, "http://") && !strings.HasPrefix(ep, "https://") {
		ep = "https://" + ep
	}
	u, err := url.Parse(ep)
	if err != nil || u.Host == "" {
		return ErrChannelMonitorInvalidEndpoint
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return ErrChannelMonitorEndpointScheme
	}
	return nil
}

// ValidateDesktopChannelConfig validates the provider-neutral desktop channel
// fields before they are persisted. It allows http/https and localhost for desktop use.
func ValidateDesktopChannelConfig(cfg ChannelProbeConfig) error {
	cfg.Provider = strings.TrimSpace(cfg.Provider)
	cfg.APIMode = defaultAPIMode(cfg.APIMode)
	cfg.Endpoint = strings.TrimSpace(cfg.Endpoint)
	cfg.APIKey = strings.TrimSpace(cfg.APIKey)
	cfg.PrimaryModel = strings.TrimSpace(cfg.PrimaryModel)
	if err := validateProvider(cfg.Provider); err != nil {
		return err
	}
	if err := validateAPIMode(cfg.Provider, cfg.APIMode); err != nil {
		return err
	}
	if err := validateDesktopEndpoint(cfg.Endpoint); err != nil {
		return err
	}
	if cfg.PrimaryModel == "" {
		return ErrChannelMonitorMissingPrimaryModel
	}
	return nil
}

// ValidateDesktopChannelInterval keeps the desktop scheduler on the same
// bounds as the built-in channel monitor, but allows 0 for manual-only probing.
func ValidateDesktopChannelInterval(seconds int) error {
	if seconds <= 0 {
		return nil
	}
	return validateInterval(seconds)
}

// ProbeChannel executes one synchronous probe for a desktop channel. It does
// not persist history or mutate channel state; callers decide how to store the
// results and whether a failed probe should remove a channel from routing.
//
// The probe itself is intentionally the existing HeiGate challenge flow:
// each configured model receives a short arithmetic request, the response is
// validated, and slow successful responses are marked degraded.
func ProbeChannel(ctx context.Context, cfg ChannelProbeConfig) ([]*CheckResult, error) {
	if ctx == nil {
		ctx = context.Background()
	}
	cfg.Provider = strings.TrimSpace(cfg.Provider)
	cfg.APIMode = defaultAPIMode(cfg.APIMode)
	cfg.Endpoint = strings.TrimSpace(cfg.Endpoint)
	cfg.APIKey = strings.TrimSpace(cfg.APIKey)
	cfg.PrimaryModel = strings.TrimSpace(cfg.PrimaryModel)

	if err := ValidateDesktopChannelConfig(cfg); err != nil {
		return nil, err
	}

	models := normalizeModels(append([]string{cfg.PrimaryModel}, cfg.ExtraModels...))
	if len(models) == 0 {
		return nil, fmt.Errorf("no probe models configured")
	}
	pingMs := pingEndpointOrigin(ctx, cfg.Endpoint)
	opts := &CheckOptions{
		APIMode:      cfg.APIMode,
		ExtraHeaders: cfg.ExtraHeaders,
	}
	results := make([]*CheckResult, len(models))
	var wg sync.WaitGroup
	for i, model := range models {
		wg.Add(1)
		go func(idx int, m string) {
			defer wg.Done()
			res := runCheckForModel(ctx, cfg.Provider, cfg.Endpoint, cfg.APIKey, m, opts)
			res.PingLatencyMs = pingMs
			results[idx] = res
		}(i, model)
	}
	wg.Wait()
	return results, nil
}
