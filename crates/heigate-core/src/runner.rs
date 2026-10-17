//! On-demand probe scheduling. Ported from the Go `internal/channel/runner.go`.
//!
//! Nothing is scheduled in the background; each channel owns one in-flight slot
//! (shared with injection checks) so explicit diagnostics never overlap and
//! trip provider-side burst limits.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::probe::{self, Config, ProbeResult, STATUS_DEGRADED, STATUS_ERROR, STATUS_OPERATIONAL};
use crate::resilience::{FailureClass, FailureInfo, CIRCUIT_CLOSED};
use crate::store::{Channel, ModelHealth, Store};

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
                .acquire_probe_for_channel(&channel, &model, key_id)
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
            let accepted = self.persist_probe_result(&channel, &result, &lease);
            result.accepted = Some(accepted);
            if success && accepted {
                let _ = lease.success(0, 0, &model);
            }
            results.push(result);
        }
        Ok(results)
    }

    fn persist_probe_result(
        &self,
        channel: &Channel,
        primary: &ProbeResult,
        lease: &crate::key_pool::KeyLease,
    ) -> bool {
        let issue_text = if primary.status != STATUS_OPERATIONAL && !primary.message.is_empty() {
            format!("[{}] {}", primary.model, primary.message)
        } else {
            String::new()
        };
        self.store
            .record_probe_result(channel, &model_health_from_probe(primary), lease, |fresh| {
                fresh.last_checked_at = Some(primary.checked_at);
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
            })
            .unwrap_or(false)
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
    use chrono::Utc;

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
            accepted: None,
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

    #[tokio::test]
    async fn an_older_probe_cannot_clear_a_new_rate_limit_but_a_fresh_probe_can() {
        use axum::routing::post;
        use std::time::Duration;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            post({
                let entered = entered.clone();
                let release = release.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        ([("content-type", "text/event-stream")],
                     "data: {\"choices\":[{\"delta\":{\"content\":\"42\"}}]}\n\ndata: [DONE]\n\n")
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Store::open_in_memory().unwrap();
        let mut channel = seed(&store, "gpt-4o");
        channel.endpoint = format!("http://{address}");
        store.save(&mut channel).unwrap();
        let pool = crate::key_pool::KeyPool::new(store.clone());
        let runner = Arc::new(ProbeRunner::with_key_pool(store.clone(), pool.clone()));
        let probe = tokio::spawn({
            let runner = runner.clone();
            async move { runner.probe_now(channel.id).await.unwrap() }
        });
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        let lease = pool
            .acquire(channel.id, "gpt-4o", &HashSet::new(), None)
            .unwrap()
            .unwrap();
        lease
            .failure(&FailureInfo::new(FailureClass::RateLimit), "gpt-4o")
            .unwrap();
        let live_failure_at = Utc::now();
        let router = crate::router::Router::new(store.clone());
        router.apply_live_outcome(
            &channel,
            "gpt-4o",
            Some(&FailureInfo::new(FailureClass::RateLimit)),
            "new live rate limit",
            10,
            live_failure_at,
        );
        release.notify_one();
        let results = tokio::time::timeout(Duration::from_secs(5), probe)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(results[0].status, STATUS_OPERATIONAL);
        assert_eq!(results[0].accepted, Some(false));
        let saved = store.get(channel.id).unwrap();
        assert_eq!(saved.last_checked_at, Some(live_failure_at));
        assert_eq!(saved.model_health[0].status, STATUS_ERROR);
        assert_eq!(saved.model_health[0].error, "new live rate limit");
        assert!(pool
            .acquire(channel.id, "gpt-4o", &HashSet::new(), None)
            .unwrap()
            .is_none());
        assert!(pool.retry_after_model(channel.id, "gpt-4o") > Duration::from_secs(290));

        // A new user-initiated sample may recover the same cooled model.
        release.notify_one();
        let results = runner.probe_now(channel.id).await.unwrap();
        assert_eq!(results[0].status, STATUS_OPERATIONAL);
        assert_eq!(results[0].accepted, Some(true));
        assert_eq!(
            store.get(channel.id).unwrap().model_health[0].status,
            STATUS_OPERATIONAL
        );
        assert!(pool
            .acquire(channel.id, "gpt-4o", &HashSet::new(), None)
            .unwrap()
            .is_some());
        server.abort();
    }

    #[tokio::test]
    async fn an_old_endpoint_probe_cannot_poison_a_new_upstream() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post({
                let entered = entered.clone();
                let release = release.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Store::open_in_memory().unwrap();
        let mut channel = seed(&store, "gpt-4o");
        channel.endpoint = endpoint;
        store.save(&mut channel).unwrap();
        let runner = Arc::new(ProbeRunner::new(store.clone()));
        let request = tokio::spawn({
            let runner = runner.clone();
            let id = channel.id;
            async move { runner.probe_now(id).await.unwrap() }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        channel.endpoint = "https://new.example.test".into();
        store.save(&mut channel).unwrap();
        release.notify_one();
        let results = tokio::time::timeout(std::time::Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(results[0].status, STATUS_ERROR);
        assert_eq!(results[0].accepted, Some(false));
        let saved = store.get(channel.id).unwrap();
        assert_eq!(saved.last_status, "unknown");
        assert!(saved.model_health.is_empty() && saved.last_checked_at.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn a_probe_reports_rejection_after_its_credential_is_replaced() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    entered.notify_one();
                    release.notified().await;
                    ([("content-type", "text/event-stream")],
                        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n")
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Store::open_in_memory().unwrap();
        let mut channel = seed(&store, "gpt-4o");
        channel.endpoint = endpoint;
        store.save(&mut channel).unwrap();
        let runner = Arc::new(ProbeRunner::new(store.clone()));
        let request = tokio::spawn({
            let runner = runner.clone();
            let id = channel.id;
            async move { runner.probe_now(id).await.unwrap() }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        let key = store.list_keys(channel.id).unwrap().remove(0);
        store
            .patch_key(
                channel.id,
                key.id,
                &crate::store_keys::KeyPatch {
                    secret: Some("replacement-secret".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        release.notify_one();
        let results = tokio::time::timeout(std::time::Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(results[0].status, STATUS_OPERATIONAL);
        assert_eq!(results[0].accepted, Some(false));
        assert_eq!(
            serde_json::to_value(&results[0]).unwrap()["accepted"],
            false
        );
        let saved = store.get(channel.id).unwrap();
        assert_eq!(saved.last_status, "unknown");
        assert!(saved.model_health.is_empty() && saved.last_checked_at.is_none());
        assert_eq!(store.list_keys(channel.id).unwrap()[0].successes, 0);
        server.abort();
    }

    #[tokio::test]
    async fn a_chat_finish_frame_cannot_hide_a_later_error_or_clear_model_cooldown() {
        use futures_util::{stream, StreamExt};
        use std::time::Duration;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    let first = stream::once(async move {
                        entered.notify_one();
                        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n"))
                    });
                    let tail = stream::once(async move {
                        release.notified().await;
                        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"data: {\"error\":{\"code\":429,\"message\":\"late rate limit\"}}\n\n"))
                    });
                    axum::response::Response::builder().header("content-type", "text/event-stream")
                        .body(axum::body::Body::from_stream(first.chain(tail))).unwrap()
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Store::open_in_memory().unwrap();
        let mut channel = seed(&store, "gpt-4o");
        channel.endpoint = endpoint;
        store.save(&mut channel).unwrap();
        let pool = crate::key_pool::KeyPool::new(store.clone());
        let lease = pool
            .acquire(channel.id, "gpt-4o", &HashSet::new(), None)
            .unwrap()
            .unwrap();
        lease
            .failure(&FailureInfo::new(FailureClass::RateLimit), "gpt-4o")
            .unwrap();
        let runner = Arc::new(ProbeRunner::with_key_pool(store.clone(), pool.clone()));
        let mut request = tokio::spawn({
            let runner = runner.clone();
            async move { runner.probe_now(channel.id).await.unwrap() }
        });
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut request)
                .await
                .is_err()
        );
        release.notify_one();
        let results = tokio::time::timeout(Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(results[0].status, STATUS_ERROR);
        assert_eq!(results[0].accepted, Some(true));
        assert!(results[0].message.contains("late rate limit"));
        assert_eq!(store.list_keys(channel.id).unwrap()[0].successes, 0);
        assert!(!pool.retry_after_model(channel.id, "gpt-4o").is_zero());
        server.abort();
    }
}
