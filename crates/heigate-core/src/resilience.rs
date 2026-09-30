//! Failure classification and circuit-breaker cooldowns. Ported from the Go
//! `internal/channel/resilience.go`; the classes and durations must not drift.
//!
//! Every failure class must carry a non-zero cooldown: the router promotes any
//! failed channel whose cooldown has expired to a half-open probe at the front
//! of the queue, so a zero cooldown would send every request to a broken
//! upstream first.

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Network,
    RateLimit,
    Auth,
    Model,
    Provider,
    Client,
}

pub const CIRCUIT_CLOSED: &str = "closed";
pub const CIRCUIT_OPEN: &str = "open";
pub const CIRCUIT_HALF_OPEN: &str = "half_open";

/// What is known about a failure. `class` is `None` for a zero value (no
/// error), which the Go code represents as an empty `FailureClass`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FailureInfo {
    pub class: Option<FailureClass>,
    pub status_code: i32,
    pub retry_after: Duration,
}

impl FailureInfo {
    pub fn new(class: FailureClass) -> Self {
        FailureInfo {
            class: Some(class),
            ..Default::default()
        }
    }
}

/// Implemented by errors that already know how they should be classified.
/// Mirrors the Go `failureInfoProvider` interface.
pub trait FailureInfoProvider {
    fn failure_info(&self) -> FailureInfo;
}

/// The cooldown to apply after a failure, honoring a server `Retry-After` and
/// growing with repeated failures.
pub fn failure_cooldown(info: &FailureInfo, failures: i64) -> Duration {
    let failures = if failures < 1 { 1 } else { failures };
    let hour = Duration::from_secs(3600);
    if info.retry_after > Duration::ZERO {
        return if info.retry_after > hour { hour } else { info.retry_after };
    }
    match info.class {
        Some(FailureClass::Auth) => Duration::from_secs(600),
        Some(FailureClass::Model) => Duration::from_secs(120),
        Some(FailureClass::RateLimit)
        | Some(FailureClass::Provider)
        | Some(FailureClass::Network) => {
            let cap = Duration::from_secs(300);
            let mut cooldown = Duration::from_secs(15);
            let mut step = 1;
            while step < failures && cooldown < cap {
                cooldown *= 2;
                step += 1;
            }
            if cooldown > cap {
                cap
            } else {
                cooldown
            }
        }
        _ => Duration::from_secs(30),
    }
}

/// Whether resending the same request could plausibly succeed immediately.
/// Deterministic client-side rejections and active rate limits should not be
/// resent before the caller's cooldown or failover logic gets a chance to run.
pub fn failure_is_retryable(info: &FailureInfo) -> bool {
    !matches!(
        info.class,
        Some(FailureClass::Auth)
            | Some(FailureClass::Model)
            | Some(FailureClass::RateLimit)
            | Some(FailureClass::Client)
    )
}

/// Whether a failure should count against the channel's health. A client-class
/// failure is the request's fault, not the channel's.
pub fn failure_is_channel_fault(info: &FailureInfo) -> bool {
    info.class != Some(FailureClass::Client)
}

pub fn failure_is_circuit_eligible(info: &FailureInfo) -> bool {
    matches!(
        info.class,
        Some(FailureClass::Provider) | Some(FailureClass::Network)
    ) || info.status_code == 408
        || info.status_code >= 500
}

/// Classifies an upstream HTTP status code.
pub fn classify_status(status: i32) -> FailureInfo {
    let class = match status {
        401 | 403 => FailureClass::Auth,
        404 => FailureClass::Model,
        408 => FailureClass::Network,
        429 => FailureClass::RateLimit,
        s if s >= 500 => FailureClass::Provider,
        _ => FailureClass::Client,
    };
    FailureInfo {
        class: Some(class),
        status_code: status,
        retry_after: Duration::ZERO,
    }
}

/// The lowercase label for a failure class; an unset class reads as `network`.
pub fn failure_label(info: &FailureInfo) -> String {
    match info.class {
        Some(class) => class_name(class).to_string(),
        None => "network".to_string(),
    }
}

fn class_name(class: FailureClass) -> &'static str {
    match class {
        FailureClass::Network => "network",
        FailureClass::RateLimit => "rate_limit",
        FailureClass::Auth => "auth",
        FailureClass::Model => "model",
        FailureClass::Provider => "provider",
        FailureClass::Client => "client",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_status_matches_policy() {
        assert_eq!(classify_status(401).class, Some(FailureClass::Auth));
        assert_eq!(classify_status(403).class, Some(FailureClass::Auth));
        assert_eq!(classify_status(404).class, Some(FailureClass::Model));
        assert_eq!(classify_status(408).class, Some(FailureClass::Network));
        assert_eq!(classify_status(429).class, Some(FailureClass::RateLimit));
        assert_eq!(classify_status(500).class, Some(FailureClass::Provider));
        assert_eq!(classify_status(503).class, Some(FailureClass::Provider));
        assert_eq!(classify_status(400).class, Some(FailureClass::Client));
    }

    #[test]
    fn cooldowns_match_the_policy_table() {
        let auth = classify_status(401);
        assert_eq!(failure_cooldown(&auth, 1), Duration::from_secs(600));

        let model = classify_status(404);
        assert_eq!(failure_cooldown(&model, 1), Duration::from_secs(120));

        // Network/rate-limit/provider back off 15s -> 5min, capped.
        let network = classify_status(500);
        assert_eq!(failure_cooldown(&network, 1), Duration::from_secs(15));
        assert_eq!(failure_cooldown(&network, 2), Duration::from_secs(30));
        assert_eq!(failure_cooldown(&network, 3), Duration::from_secs(60));
        assert_eq!(failure_cooldown(&network, 5), Duration::from_secs(240));
        assert_eq!(failure_cooldown(&network, 6), Duration::from_secs(300));
        assert_eq!(failure_cooldown(&network, 50), Duration::from_secs(300));

        // Retry-After wins, capped at one hour.
        let mut info = FailureInfo::new(FailureClass::RateLimit);
        info.retry_after = Duration::from_secs(120);
        assert_eq!(failure_cooldown(&info, 1), Duration::from_secs(120));
        info.retry_after = Duration::from_secs(7200);
        assert_eq!(failure_cooldown(&info, 1), Duration::from_secs(3600));

        // Unknown class falls back to 30s.
        assert_eq!(
            failure_cooldown(&FailureInfo::default(), 1),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn class_predicates() {
        assert!(!failure_is_retryable(&classify_status(401)));
        assert!(!failure_is_retryable(&classify_status(404)));
        assert!(!failure_is_retryable(&classify_status(429)));
        assert!(!failure_is_retryable(&classify_status(400)));
        assert!(failure_is_retryable(&classify_status(500)));

        assert!(!failure_is_channel_fault(&classify_status(400)));
        assert!(failure_is_channel_fault(&classify_status(500)));

        assert!(failure_is_circuit_eligible(&classify_status(500)));
        assert!(failure_is_circuit_eligible(&classify_status(408)));
        assert!(!failure_is_circuit_eligible(&classify_status(400)));

        assert_eq!(failure_label(&FailureInfo::default()), "network");
        assert_eq!(failure_label(&classify_status(429)), "rate_limit");
    }
}
