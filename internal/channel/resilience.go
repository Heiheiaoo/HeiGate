package channel

import (
	"errors"
	"net"
	"strings"
	"time"
)

type FailureClass string

const (
	FailureNetwork   FailureClass = "network"
	FailureRateLimit FailureClass = "rate_limit"
	FailureAuth      FailureClass = "auth"
	FailureModel     FailureClass = "model"
	FailureProvider  FailureClass = "provider"
	FailureClient    FailureClass = "client"
)

const (
	CircuitClosed   = "closed"
	CircuitOpen     = "open"
	CircuitHalfOpen = "half_open"
)

type FailureInfo struct {
	Class      FailureClass
	StatusCode int
	RetryAfter time.Duration
}

type failureInfoProvider interface {
	FailureInfo() FailureInfo
}

func FailureFromError(err error) FailureInfo {
	if err == nil {
		return FailureInfo{}
	}
	var provider failureInfoProvider
	if errors.As(err, &provider) {
		return provider.FailureInfo()
	}
	if errors.Is(err, net.ErrClosed) {
		return FailureInfo{Class: FailureNetwork}
	}
	return FailureInfo{Class: FailureNetwork}
}

func FailureCooldown(info FailureInfo, failures int) time.Duration {
	if failures < 1 {
		failures = 1
	}
	if info.RetryAfter > 0 {
		if info.RetryAfter > time.Hour {
			return time.Hour
		}
		return info.RetryAfter
	}
	switch info.Class {
	case FailureAuth:
		return 10 * time.Minute
	case FailureModel:
		return 2 * time.Minute
	case FailureRateLimit:
		return 30 * time.Second
	case FailureProvider, FailureNetwork:
		cooldown := 15 * time.Second
		for step := 1; step < failures && cooldown < 5*time.Minute; step++ {
			cooldown *= 2
		}
		if cooldown > 5*time.Minute {
			return 5 * time.Minute
		}
		return cooldown
	default:
		return 30 * time.Second
	}
}

// FailureIsRetryable reports whether resending the same request could
// plausibly succeed. A deterministic client-side rejection (bad request, bad
// key, unknown model) fails identically on a second identical request, so
// retrying only burns upstream quota and adds latency.
func FailureIsRetryable(info FailureInfo) bool {
	switch info.Class {
	case FailureAuth, FailureModel, FailureClient:
		return false
	}
	return true
}

// FailureIsChannelFault reports whether a failure should count against the
// channel's health. A client-class failure is the request's fault, not the
// channel's: counting it would let one malformed request bench a perfectly
// healthy channel and take it out of the routing pool.
func FailureIsChannelFault(info FailureInfo) bool {
	return info.Class != FailureClient
}

func FailureIsCircuitEligible(info FailureInfo) bool {
	return info.Class == FailureProvider || info.Class == FailureNetwork || info.StatusCode == 408 || info.StatusCode >= 500
}

func ClassifyStatus(status int) FailureInfo {
	info := FailureInfo{StatusCode: status}
	switch {
	case status == 401 || status == 403:
		info.Class = FailureAuth
	case status == 404:
		info.Class = FailureModel
	case status == 408 || status == 429:
		info.Class = FailureRateLimit
	case status >= 500:
		info.Class = FailureProvider
	default:
		info.Class = FailureClient
	}
	return info
}

func FailureLabel(info FailureInfo) string {
	if info.Class != "" {
		return string(info.Class)
	}
	return strings.ToLower(string(FailureNetwork))
}
