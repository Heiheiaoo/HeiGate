package service

import (
	"errors"
	"net"
	"strings"
	"time"
)

type DesktopFailureClass string

const (
	DesktopFailureNetwork   DesktopFailureClass = "network"
	DesktopFailureRateLimit DesktopFailureClass = "rate_limit"
	DesktopFailureAuth      DesktopFailureClass = "auth"
	DesktopFailureModel     DesktopFailureClass = "model"
	DesktopFailureProvider  DesktopFailureClass = "provider"
	DesktopFailureClient    DesktopFailureClass = "client"
)

const (
	DesktopCircuitClosed = "closed"
	DesktopCircuitOpen   = "open"
	DesktopCircuitHalf   = "half_open"
)

type DesktopFailureInfo struct {
	Class      DesktopFailureClass
	StatusCode int
	RetryAfter time.Duration
}

type desktopFailureInfoProvider interface {
	FailureInfo() DesktopFailureInfo
}

func DesktopFailureFromError(err error) DesktopFailureInfo {
	if err == nil {
		return DesktopFailureInfo{}
	}
	var provider desktopFailureInfoProvider
	if errors.As(err, &provider) {
		return provider.FailureInfo()
	}
	if errors.Is(err, net.ErrClosed) {
		return DesktopFailureInfo{Class: DesktopFailureNetwork}
	}
	return DesktopFailureInfo{Class: DesktopFailureNetwork}
}

func DesktopFailureCooldown(info DesktopFailureInfo, failures int) time.Duration {
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
	case DesktopFailureAuth:
		return 10 * time.Minute
	case DesktopFailureModel:
		return 2 * time.Minute
	case DesktopFailureRateLimit:
		return 30 * time.Second
	case DesktopFailureProvider, DesktopFailureNetwork:
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

func DesktopFailureIsCircuitEligible(info DesktopFailureInfo) bool {
	return info.Class == DesktopFailureProvider || info.Class == DesktopFailureNetwork || info.StatusCode == 408 || info.StatusCode >= 500
}

func DesktopClassifyStatus(status int) DesktopFailureInfo {
	info := DesktopFailureInfo{StatusCode: status}
	switch {
	case status == 401 || status == 403:
		info.Class = DesktopFailureAuth
	case status == 404:
		info.Class = DesktopFailureModel
	case status == 408 || status == 429:
		info.Class = DesktopFailureRateLimit
	case status >= 500:
		info.Class = DesktopFailureProvider
	default:
		info.Class = DesktopFailureClient
	}
	return info
}

func DesktopFailureLabel(info DesktopFailureInfo) string {
	if info.Class != "" {
		return string(info.Class)
	}
	return strings.ToLower(string(DesktopFailureNetwork))
}
