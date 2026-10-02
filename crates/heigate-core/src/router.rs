//! Resolves a model to enabled local channels and runs the fault-tolerant
//! retry loop. Ported from the Go `internal/channel/router.go`.
//!
//! It owns no wire-protocol code; callers supply the forward closure.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};

use chrono::Utc;

use crate::canonical::channel_supports_model_or_canonical;
use crate::probe::{STATUS_DEGRADED, STATUS_ERROR, STATUS_OPERATIONAL};
use crate::resilience::{
    failure_cooldown, failure_is_channel_fault, failure_is_circuit_eligible, failure_label,
    FailureInfo, FailureInfoProvider, CIRCUIT_CLOSED, CIRCUIT_HALF_OPEN, CIRCUIT_OPEN,
};
use crate::store::{Channel, Store};

pub const ROUTE_SMART_QUALITY: &str = "smart_quality";
pub const ROUTE_LATENCY: &str = "latency";
pub const ROUTE_ROUND_ROBIN: &str = "round_robin";

#[derive(Default)]
struct RouterState {
    turn: HashMap<String, usize>,
    half_open: HashSet<i64>,
}

/// Resolves a model to enabled local channels and retries across them.
pub struct Router {
    store: Arc<Store>,
    state: Mutex<RouterState>,
}

impl Router {
    pub fn new(store: Arc<Store>) -> Self {
        Router {
            store,
            state: Mutex::new(RouterState::default()),
        }
    }

    /// Returns enabled channels that advertise the requested model, ordered by
    /// the routing strategy. Failed channels re-enter the pool as single-request
    /// recovery probes once their cooldown expires.
    pub fn candidates(&self, model: &str, strategy: &str) -> Result<Vec<Channel>, String> {
        self.candidates_matching(model, strategy, None)
    }

    /// Applies an optional protocol predicate before health and priority
    /// selection — filtering after selection would hide the only usable route
    /// for a protocol.
    pub fn candidates_matching(
        &self,
        model: &str,
        strategy: &str,
        matches: Option<&(dyn Fn(&Channel) -> bool + Send + Sync)>,
    ) -> Result<Vec<Channel>, String> {
        let channels = self.store.list().map_err(|err| err.to_string())?;
        let model = model.trim();
        let now = Utc::now();

        let mut healthy: Vec<Channel> = Vec::new();
        let mut degraded: Vec<Channel> = Vec::new();
        let mut failed: Vec<Channel> = Vec::new();
        let mut cooldown: Vec<Channel> = Vec::new();

        for mut channel in channels {
            if !channel.enabled {
                continue;
            }
            if !model.is_empty() && !supports_model(&channel, model) {
                continue;
            }
            if let Some(matches) = matches {
                if !matches(&channel) {
                    continue;
                }
            }
            if let Some(until) = channel.cooldown_until {
                if now < until {
                    cooldown.push(channel);
                    continue;
                }
            }
            if channel.circuit_state == CIRCUIT_OPEN
                || (channel.failure_count > 0 && channel.last_status == STATUS_ERROR)
            {
                // Lazy half-open recovery: after the cooldown expires, let one
                // real request test the channel.
                channel.circuit_state = CIRCUIT_HALF_OPEN.to_string();
                let channel_id = channel.id;
                let _ = self.store.update_runtime(channel_id, move |fresh| {
                    let cooldown_expired = fresh
                        .cooldown_until
                        .map(|until| now >= until)
                        .unwrap_or(true);
                    if cooldown_expired
                        && (fresh.circuit_state == CIRCUIT_OPEN
                            || (fresh.failure_count > 0 && fresh.last_status == STATUS_ERROR))
                    {
                        fresh.circuit_state = CIRCUIT_HALF_OPEN.to_string();
                    }
                });
            }
            if channel.circuit_state == CIRCUIT_HALF_OPEN {
                healthy.push(channel);
                continue;
            }
            match channel.last_status.as_str() {
                STATUS_OPERATIONAL | "healthy" | "" | "unknown" => healthy.push(channel),
                STATUS_DEGRADED => degraded.push(channel),
                _ => failed.push(channel),
            }
        }

        if healthy.is_empty() {
            healthy = degraded;
        }
        if healthy.is_empty() {
            healthy = failed;
        }
        // A cooling channel is never a last-resort candidate.
        if healthy.is_empty() {
            return Err(format!("no enabled channel supports model {model:?}"));
        }

        self.sort_candidates(&mut healthy, model, strategy);
        Ok(healthy)
    }

    /// Tries each candidate in order, applying the same failover bookkeeping as
    /// [`Router::route`] to a caller-supplied candidate list.
    pub async fn route_candidates<F, Fut, E>(
        &self,
        model: &str,
        candidates: &mut [Channel],
        mut forward: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&mut Channel) -> Fut,
        Fut: Future<Output = Result<(), E>>,
        E: FailureInfoProvider + std::fmt::Display,
    {
        if candidates.is_empty() {
            return Err(format!("no enabled channel supports model {model:?}"));
        }

        let mut last_err: Option<String> = None;
        let mut claimed_half_open: Vec<i64> = Vec::new();

        // Indexed: the winner is cloned out and returned by position.
        #[allow(clippy::needless_range_loop)]
        for index in 0..candidates.len() {
            // A half-open circuit is a single recovery probe, not a concurrency
            // bucket. Claim the persisted half-open channel before forwarding.
            let is_half_open = candidates[index].circuit_state == CIRCUIT_HALF_OPEN;
            if is_half_open {
                if !self.claim_half_open(candidates[index].id) {
                    continue;
                }
                claimed_half_open.push(candidates[index].id);
            }

            let started = std::time::Instant::now();
            let outcome = forward(&mut candidates[index]).await;
            let latency = started.elapsed().as_millis() as i64;

            match outcome {
                Ok(()) => {
                    self.apply_channel_success(&mut candidates[index], latency);
                    self.release_half_open(claimed_half_open);
                    return Ok(index);
                }
                Err(err) => {
                    last_err = Some(err.to_string());
                    let info: FailureInfo = err.failure_info();
                    // A client-class failure is the request's fault; only return
                    // it without benching the channel.
                    if failure_is_channel_fault(&info) {
                        self.apply_channel_failure(
                            &mut candidates[index],
                            &info,
                            &err.to_string(),
                            latency,
                        );
                    }
                }
            }
        }

        self.release_half_open(claimed_half_open);

        let last_err = last_err
            .unwrap_or_else(|| "all candidate channels are currently recovering".to_string());
        Err(format!(
            "all desktop channels failed for model {model:?}: {last_err}"
        ))
    }

    /// Clears a channel's transient failure state and records a fresh latency.
    /// Shared with the gateway's forwarding loop so the two cannot diverge.
    pub fn apply_channel_success(&self, channel: &mut Channel, latency: i64) {
        channel.last_status = STATUS_OPERATIONAL.to_string();
        channel.last_error = String::new();
        channel.failure_count = 0;
        channel.circuit_state = CIRCUIT_CLOSED.to_string();
        channel.cooldown_until = None;
        channel.last_latency_ms = Some(latency);
        // Keep the forwarder's measured TTFT if it supplied one; never
        // substitute total request duration for it.
        let ttft = channel.last_ttft_ms.filter(|value| *value > 0);
        let now = Utc::now();
        channel.last_checked_at = Some(now);
        let channel_id = channel.id;
        let _ = self.store.update_runtime(channel_id, move |fresh| {
            fresh.last_status = STATUS_OPERATIONAL.to_string();
            fresh.last_error = String::new();
            fresh.failure_count = 0;
            fresh.circuit_state = CIRCUIT_CLOSED.to_string();
            fresh.cooldown_until = None;
            fresh.last_latency_ms = Some(latency);
            if let Some(ttft) = ttft {
                fresh.last_ttft_ms = Some(ttft);
            }
            fresh.last_checked_at = Some(now);
        });
    }

    /// Persists a channel fault: increments the failure count, sets the cooldown
    /// and opens the circuit once eligible.
    pub fn apply_channel_failure(
        &self,
        channel: &mut Channel,
        info: &FailureInfo,
        message: &str,
        latency: i64,
    ) {
        let label = failure_label(info);
        channel.failure_count += 1;
        channel.last_status = STATUS_ERROR.to_string();
        channel.last_error = format!("{label}: {message}");
        let until = Utc::now() + failure_cooldown(info, channel.failure_count);
        channel.cooldown_until = Some(until);
        if failure_is_circuit_eligible(info) && channel.failure_count >= 3 {
            channel.circuit_state = CIRCUIT_OPEN.to_string();
        } else if channel.circuit_state.is_empty() {
            channel.circuit_state = CIRCUIT_CLOSED.to_string();
        }
        channel.last_latency_ms = Some(latency);
        channel.last_ttft_ms = None;
        let now = Utc::now();
        channel.last_checked_at = Some(now);

        let channel_id = channel.id;
        let info = *info;
        let message = message.to_string();
        let _ = self.store.update_runtime(channel_id, move |fresh| {
            fresh.failure_count += 1;
            fresh.last_status = STATUS_ERROR.to_string();
            fresh.last_error = format!("{}: {}", failure_label(&info), message);
            let until = Utc::now() + failure_cooldown(&info, fresh.failure_count);
            fresh.cooldown_until = Some(until);
            if failure_is_circuit_eligible(&info) && fresh.failure_count >= 3 {
                fresh.circuit_state = CIRCUIT_OPEN.to_string();
            } else if fresh.circuit_state.is_empty() {
                fresh.circuit_state = CIRCUIT_CLOSED.to_string();
            }
            fresh.last_latency_ms = Some(latency);
            fresh.last_ttft_ms = None;
            fresh.last_checked_at = Some(now);
        });
    }

    /// Claims the single in-flight slot for a half-open channel.
    pub fn claim_half_open_public(&self, id: i64) -> bool {
        self.claim_half_open(id)
    }

    /// Releases a previously claimed half-open slot.
    pub fn release_half_open_public(&self, id: i64) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.half_open.remove(&id);
    }

    fn claim_half_open(&self, id: i64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.half_open.insert(id)
    }

    fn release_half_open(&self, ids: Vec<i64>) {
        if ids.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        for id in ids {
            state.half_open.remove(&id);
        }
    }

    fn sort_candidates(&self, channels: &mut [Channel], model: &str, strategy: &str) {
        channels.sort_by(|a, b| {
            // Recovery probes go first under every strategy; RouteCandidates
            // still enforces one in-flight half-open call.
            let a_half = a.circuit_state == CIRCUIT_HALF_OPEN;
            let b_half = b.circuit_state == CIRCUIT_HALF_OPEN;
            if a_half != b_half {
                return if a_half {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
            match strategy {
                ROUTE_LATENCY | ROUTE_ROUND_ROBIN => {
                    responsiveness_of(a).cmp(&responsiveness_of(b))
                }
                _ => quality_score(b).cmp(&quality_score(a)),
            }
        });

        if strategy == ROUTE_ROUND_ROBIN && channels.len() > 1 {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let len = channels.len();
            let start = *state.turn.get(model).unwrap_or(&0) % len;
            *state.turn.entry(model.to_string()).or_insert(0) += 1;
            drop(state);
            channels.rotate_left(start);
        }
    }
}

fn quality_score(channel: &Channel) -> i64 {
    let mut score: i64 = match channel.last_status.as_str() {
        STATUS_OPERATIONAL | "healthy" => 10000,
        STATUS_DEGRADED => 5000,
        "unknown" | "" => 3000,
        _ => 0,
    };

    if channel.circuit_state == CIRCUIT_HALF_OPEN {
        score += 20000;
    } else if channel.circuit_state == CIRCUIT_OPEN {
        score -= 8000;
    }

    score -= channel.failure_count * 1500;

    if channel.last_error.contains("429") || channel.last_error.contains("Too Many") {
        score -= 3000;
    }

    let lat = responsiveness_of(channel);
    if lat < 1_000_000 {
        score -= lat;
    }

    score
}

fn supports_model(channel: &Channel, model: &str) -> bool {
    if model.is_empty() {
        return false;
    }
    channel_supports_model_or_canonical(channel, model)
}

fn latency_of(channel: &Channel) -> i64 {
    match channel.last_latency_ms {
        Some(value) if value > 0 => value,
        _ => i64::MAX,
    }
}

/// The routing sort key: time to first token when measured, total latency
/// otherwise.
fn responsiveness_of(channel: &Channel) -> i64 {
    match channel.last_ttft_ms {
        Some(value) if value > 0 => value,
        _ => latency_of(channel),
    }
}

#[cfg(test)]
mod tests {
    //! Ports the behaviour of `internal/channel/router_test.go`.

    use super::*;
    use crate::store::Store;

    fn seed(store: &Store, name: &str, model: &str, status: &str) -> Channel {
        let mut channel = Channel {
            name: name.to_string(),
            provider: "openai".to_string(),
            api_mode: "chat_completions".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: "sk".to_string(),
            primary_model: model.to_string(),
            enabled: true,
            ..Default::default()
        };
        store.save(&mut channel).unwrap();
        if !status.is_empty() {
            store
                .update_runtime(channel.id, |fresh| {
                    fresh.last_status = status.to_string();
                })
                .unwrap();
        }
        store.get(channel.id).unwrap()
    }

    #[test]
    fn candidates_filter_by_model_and_enabled() {
        let store = Store::open_in_memory().unwrap();
        seed(&store, "a", "gpt-4o", STATUS_OPERATIONAL);
        seed(&store, "b", "claude-3", STATUS_OPERATIONAL);
        let router = Router::new(store.clone());

        let got = router.candidates("gpt-4o", ROUTE_SMART_QUALITY).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "a");
    }

    #[test]
    fn no_supporting_channel_is_an_error() {
        let store = Store::open_in_memory().unwrap();
        seed(&store, "a", "gpt-4o", STATUS_OPERATIONAL);
        let router = Router::new(store);
        assert!(router
            .candidates("unknown-model", ROUTE_SMART_QUALITY)
            .is_err());
    }

    #[tokio::test]
    async fn route_candidates_records_success_and_clears_failure() {
        let store = Store::open_in_memory().unwrap();
        let mut channel = seed(&store, "a", "gpt-4o", STATUS_ERROR);
        channel.failure_count = 2;
        store.save(&mut channel).unwrap();

        let router = Router::new(store.clone());
        let mut candidates = router.candidates("gpt-4o", ROUTE_SMART_QUALITY).unwrap();
        assert_eq!(candidates.len(), 1);

        let index = router
            .route_candidates("gpt-4o", &mut candidates, |_ch| async {
                Ok::<(), ForwardTestError>(())
            })
            .await
            .unwrap();
        assert_eq!(index, 0);

        let stored = store.get(channel.id).unwrap();
        assert_eq!(stored.last_status, STATUS_OPERATIONAL);
        assert_eq!(stored.failure_count, 0);
        assert_eq!(stored.circuit_state, CIRCUIT_CLOSED);
        assert!(stored.cooldown_until.is_none());
    }

    #[tokio::test]
    async fn route_candidates_fails_over_and_cools_the_channel() {
        let store = Store::open_in_memory().unwrap();
        let first = seed(&store, "a", "gpt-4o", STATUS_OPERATIONAL);
        let second = seed(&store, "b", "gpt-4o", STATUS_OPERATIONAL);
        let router = Router::new(store.clone());

        let mut candidates = router.candidates("gpt-4o", ROUTE_SMART_QUALITY).unwrap();
        assert_eq!(candidates.len(), 2);

        // First candidate fails with a 500 (provider), second succeeds.
        let first_name = candidates[0].name.clone();
        let index = router
            .route_candidates("gpt-4o", &mut candidates, |ch| {
                let name = ch.name.clone();
                let first_name = first_name.clone();
                async move {
                    if name == first_name {
                        Err(ForwardTestError::provider())
                    } else {
                        Ok(())
                    }
                }
            })
            .await
            .unwrap();
        assert_eq!(candidates[index].name, second.name);

        let failed = store.get(first.id).unwrap();
        assert_eq!(failed.last_status, STATUS_ERROR);
        assert_eq!(failed.failure_count, 1);
        assert!(failed.cooldown_until.is_some());
        assert!(failed.last_error.starts_with("provider:"));
    }

    #[test]
    fn half_open_claim_is_exclusive() {
        let store = Store::open_in_memory().unwrap();
        let router = Router::new(store);
        assert!(router.claim_half_open(1));
        assert!(!router.claim_half_open(1));
        router.release_half_open(vec![1]);
        assert!(router.claim_half_open(1));
    }

    #[test]
    fn cooldown_channels_become_half_open_probes() {
        let store = Store::open_in_memory().unwrap();
        let mut channel = seed(&store, "a", "gpt-4o", STATUS_ERROR);
        channel.circuit_state = CIRCUIT_OPEN.to_string();
        channel.failure_count = 3;
        channel.cooldown_until = Some(Utc::now() - chrono::Duration::seconds(1));
        store.save(&mut channel).unwrap();

        let router = Router::new(store.clone());
        let candidates = router.candidates("gpt-4o", ROUTE_SMART_QUALITY).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].circuit_state, CIRCUIT_HALF_OPEN);
    }

    /// A minimal forward error mirroring the gateway's failure classification.
    #[derive(Debug)]
    enum ForwardTestError {
        Status(i32),
    }

    impl ForwardTestError {
        fn provider() -> Self {
            ForwardTestError::Status(500)
        }
    }

    impl std::fmt::Display for ForwardTestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                ForwardTestError::Status(status) => write!(f, "upstream status {status}"),
            }
        }
    }

    impl FailureInfoProvider for ForwardTestError {
        fn failure_info(&self) -> FailureInfo {
            match self {
                ForwardTestError::Status(status) => crate::resilience::classify_status(*status),
            }
        }
    }
}
