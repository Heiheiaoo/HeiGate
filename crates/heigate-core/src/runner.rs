//! On-demand probe scheduling. Ported from the Go `internal/channel/runner.go`.
//!
//! Nothing is scheduled in the background; each channel owns one in-flight slot
//! (shared with injection checks) so explicit diagnostics never overlap and
//! trip provider-side burst limits.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use chrono::Utc;

use crate::probe::{self, Config, ProbeResult, STATUS_DEGRADED, STATUS_ERROR, STATUS_OPERATIONAL};
use crate::resilience::{FailureClass, FailureInfo, CIRCUIT_CLOSED};
use crate::store::{ModelHealth, Store};

/// Returned when a probe is already in flight for the channel.
pub const ERR_PROBE_BUSY: &str = "probe already in progress";
/// Returned when the channel is disabled.
pub const ERR_CHANNEL_DISABLED: &str = "channel is disabled";

#[derive(Debug)]
pub enum RunnerError {
    Busy,
    Disabled,
    NotFound(i64),
    Store(String),
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunnerError::Busy => write!(f, "{ERR_PROBE_BUSY}"),
            RunnerError::Disabled => write!(f, "{ERR_CHANNEL_DISABLED}"),
            RunnerError::NotFound(id) => write!(f, "desktop channel {id}: not found"),
            RunnerError::Store(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RunnerError {}

/// Runs on-demand health probes for local channels.
pub struct ProbeRunner {
    store: Arc<Store>,
    key_pool: Arc<crate::key_pool::KeyPool>,
    in_flight: Mutex<HashSet<i64>>,
}

impl ProbeRunner {
    pub fn new(store: Arc<Store>) -> Self {
        Self::with_key_pool(store.clone(), crate::key_pool::KeyPool::new(store))
    }

    pub fn with_key_pool(store: Arc<Store>, key_pool: Arc<crate::key_pool::KeyPool>) -> Self {
        ProbeRunner {
            store,
            key_pool,
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    /// Runs the lightweight channel health check: only the primary model is
    /// called, because it drives the channel headline status, routing priority,
    /// circuit breaker, and adaptive schedule.
    pub async fn probe_now(&self, id: i64) -> Result<Vec<ProbeResult>, RunnerError> {
        self.probe(id, "", false, None).await
    }

    /// Checks one explicitly selected model. When it is the channel's current
    /// primary model, the normal health/circuit bookkeeping is persisted.
    pub async fn probe_model_now(
        &self,
        id: i64,
        model: &str,
    ) -> Result<Vec<ProbeResult>, RunnerError> {
        let model = model.trim();
        if model.is_empty() {
            return self.probe_now(id).await;
        }
        self.probe(id, model, false, None).await
    }

    /// Performs an explicit deep scan of every configured model, sequentially.
    pub async fn probe_all_models_now(&self, id: i64) -> Result<Vec<ProbeResult>, RunnerError> {
        self.probe(id, "", true, None).await
    }

    pub async fn probe_key_now(
        &self,
        id: i64,
        key_id: i64,
    ) -> Result<Vec<ProbeResult>, RunnerError> {
        self.probe(id, "", false, Some(key_id)).await
    }

    async fn probe(
        &self,
        id: i64,
        target_model: &str,
        all_models: bool,
        key_id: Option<i64>,
    ) -> Result<Vec<ProbeResult>, RunnerError> {
        if !self.acquire_channel(id) {
            return Err(RunnerError::Busy);
        }
        let _guard = ChannelGuard { runner: self, id };

        let channel = self.store.get(id).map_err(|err| match err {
            crate::store::StoreError::NotFound(id) => RunnerError::NotFound(id),
            other => RunnerError::Store(other.to_string()),
        })?;
        if !channel.enabled {
            return Err(RunnerError::Disabled);
        }

        let probed_primary = channel.primary_model.clone();
        let models = if !target_model.is_empty() {
            vec![target_model.to_string()]
        } else if all_models {
            let mut models = vec![probed_primary.clone()];
            models.extend(channel.extra_models.clone());
            models
        } else {
            vec![probed_primary.clone()]
        };
        let mut results = Vec::new();
        for model in models {
            let lease = self
                .key_pool
                .acquire_for_probe(id, &model, key_id)
                .map_err(|e| RunnerError::Store(e.to_string()))?
                .ok_or_else(|| {
                    RunnerError::Store("没有可用于探活的 Key（未配置或已停用）".into())
                })?;
            let cfg = Config {
                provider: channel.provider.clone(),
                api_mode: channel.api_mode.clone(),
                endpoint: probe::normalize_endpoint(&channel.endpoint),
                api_key: lease.key.secret.clone(),
                primary_model: model.clone(),
                extra_models: Vec::new(),
                extra_headers: Default::default(),
            };
            let mut result = probe::check_model_once(&cfg, &model).await;
            result.key_id = lease.key.id;
            result.key_name = lease.key.name.clone();
            if !lease.key.secret.is_empty() {
                result.message = result.message.replace(&lease.key.secret, "[REDACTED]");
            }
            let success = matches!(result.status.as_str(), STATUS_OPERATIONAL | STATUS_DEGRADED);
            if success {
                let _ = lease.success(0, 0, &model);
                let _ = self.store.clear_key_model_cooldown(lease.key.id, &model);
            }
            // CRITICAL: Probes must NEVER call lease.failure or engage temporary retreat cooldown on the key or model.
            // Upstream diagnostic results are saved directly to model_health and channel headline.
            let _ = self
                .store
                .set_model_health(id, &model_health_from_probe(&result));
            if model == probed_primary {
                self.persist_probe_results(id, &probed_primary, std::slice::from_ref(&result));
            }
            results.push(result);
        }
        Ok(results)
    }

    fn persist_probe_results(&self, id: i64, expected_primary: &str, results: &[ProbeResult]) {
        // Non-operational models are surfaced in the channel's message
        // regardless of which is primary; the headline status and latency come
        // from the primary alone.
        let issues: Vec<String> = results
            .iter()
            .filter(|r| r.status != STATUS_OPERATIONAL && !r.message.is_empty())
            .map(|r| format!("[{}] {}", r.model, r.message))
            .collect();
        let issue_text = issues.join("; ");

        let primary = results
            .iter()
            .find(|r| r.model == expected_primary)
            .or_else(|| results.first());
        let Some(primary) = primary else {
            return;
        };
        let primary = primary.clone();
        let expected = expected_primary.to_string();
        let _ = self.store.update_runtime(id, move |fresh| {
            if fresh.primary_model != expected {
                return;
            }
            fresh.last_checked_at = Some(Utc::now());
            fresh.last_ping_latency_ms = primary.ping_latency_ms;

            match primary.status.as_str() {
                STATUS_OPERATIONAL => {
                    fresh.last_status = STATUS_OPERATIONAL.to_string();
                    fresh.last_latency_ms = primary.latency_ms;
                    fresh.last_ttft_ms = primary.ttft_ms;
                    fresh.last_error = issue_text;
                    fresh.probe_streak += 1;
                    fresh.failure_count = 0;
                    fresh.circuit_state = CIRCUIT_CLOSED.to_string();
                    fresh.cooldown_until = None;
                }
                STATUS_DEGRADED => {
                    fresh.last_status = STATUS_DEGRADED.to_string();
                    fresh.last_latency_ms = primary.latency_ms;
                    fresh.last_ttft_ms = primary.ttft_ms;
                    // The request completed; it is slow, not broken. Clear stale
                    // failure/circuit state.
                    fresh.failure_count = 0;
                    fresh.circuit_state = CIRCUIT_CLOSED.to_string();
                    fresh.cooldown_until = None;
                    fresh.last_error = if !primary.message.is_empty() {
                        format!("[{}] {}", primary.model, primary.message)
                    } else {
                        issue_text
                    };
                }
                _ => {
                    fresh.last_status = STATUS_ERROR.to_string();
                    fresh.last_latency_ms = None;
                    fresh.last_ttft_ms = None;
                    fresh.last_error = if !primary.message.is_empty() {
                        format!("[{}] {}", primary.model, primary.message)
                    } else if !issue_text.is_empty() {
                        issue_text
                    } else {
                        format!("[{}] 探活失败", primary.model)
                    };
                    fresh.failure_count += 1;
                    fresh.probe_streak = 0;
                    // Probes are diagnostic tests and must NOT engage cooldown_until or circuit breaker on the channel.
                }
            }
        });
    }

    /// Reserves the single upstream request slot for a channel. Manual probes
    /// and injection checks share the slot.
    pub fn acquire_channel(&self, id: i64) -> bool {
        let mut in_flight = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
        in_flight.insert(id)
    }

    /// Releases a channel's upstream request slot.
    pub fn release_channel(&self, id: i64) {
        let mut in_flight = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
        in_flight.remove(&id);
    }
}

/// Releases the channel slot when the probe future is dropped, so an aborted
/// request cannot permanently occupy it.
struct ChannelGuard<'a> {
    runner: &'a ProbeRunner,
    id: i64,
}

impl Drop for ChannelGuard<'_> {
    fn drop(&mut self) {
        self.runner.release_channel(self.id);
    }
}

/// Maps an upstream failure message to a failure class, detecting rate limits.
#[allow(dead_code)]
fn probe_failure_info(result: &ProbeResult) -> FailureInfo {
    if result.http_status >= 400 {
        let mut info = crate::resilience::classify_status(result.http_status);
        info.retry_after = std::time::Duration::from_secs(result.retry_after_secs);
        return info;
    }
    let message = result.message.to_lowercase();
    for marker in [
        "429",
        "rate limit",
        "rate_limit",
        "rpm",
        "tpm",
        "quota exceeded",
        "too many requests",
    ] {
        if message.contains(marker) {
            return FailureInfo {
                class: Some(FailureClass::RateLimit),
                status_code: 429,
                retry_after: std::time::Duration::ZERO,
            };
        }
    }
    FailureInfo::new(FailureClass::Provider)
}

/// Builds a stored model-health snapshot from a probe result.
pub fn model_health_from_probe(result: &ProbeResult) -> ModelHealth {
    ModelHealth {
        model: result.model.clone(),
        status: result.status.clone(),
        latency_ms: result.latency_ms,
        ttft_ms: result.ttft_ms,
        error: result.message.clone(),
        checked_at: Some(result.checked_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Channel, Store};

    fn seed(store: &Store, model: &str) -> Channel {
        let mut channel = Channel {
            name: "a".to_string(),
            provider: "openai".to_string(),
            api_mode: "chat_completions".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: "sk".to_string(),
            primary_model: model.to_string(),
            enabled: true,
            ..Default::default()
        };
        store.save(&mut channel).unwrap();
        channel
    }

    #[test]
    fn channel_slot_is_exclusive_and_released() {
        let store = Store::open_in_memory().unwrap();
        let runner = ProbeRunner::new(store);
        assert!(runner.acquire_channel(1));
        assert!(!runner.acquire_channel(1));
        runner.release_channel(1);
        assert!(runner.acquire_channel(1));
    }

    #[test]
    fn rate_limit_messages_are_classified() {
        let mut result = ProbeResult {
            http_status: 0,
            retry_after_secs: 0,
            key_id: 0,
            key_name: String::new(),
            model: "gpt".to_string(),
            status: STATUS_ERROR.to_string(),
            message: "upstream HTTP 429: too many requests".to_string(),
            latency_ms: None,
            ttft_ms: None,
            ping_latency_ms: None,
            checked_at: Utc::now(),
        };
        assert_eq!(
            probe_failure_info(&result).class,
            Some(FailureClass::RateLimit)
        );
        result.message = "connection reset".to_string();
        assert_eq!(
            probe_failure_info(&result).class,
            Some(FailureClass::Provider)
        );
    }

    #[tokio::test]
    async fn probe_on_disabled_channel_is_rejected() {
        let store = Store::open_in_memory().unwrap();
        let mut channel = seed(&store, "gpt-4o");
        channel.enabled = false;
        store.save(&mut channel).unwrap();

        let runner = ProbeRunner::new(store);
        let err = runner.probe_now(channel.id).await.unwrap_err();
        assert!(matches!(err, RunnerError::Disabled));
        // The slot was released even though the probe was rejected.
        assert!(runner.acquire_channel(channel.id));
    }
}
