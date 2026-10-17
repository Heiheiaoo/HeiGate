use super::*;
use crate::store_keys::KeyInput;
use axum::{extract::State, routing::post, Json};
use chrono::Utc;
type Calls = Arc<Mutex<Vec<(String, serde_json::Value)>>>;
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn setup(mode: &'static str) -> (Gateway, i64, Calls, Server) {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app=axum::Router::new().route("/v1/chat/completions",post(move |State(calls):State<Calls>,headers:HeaderMap,Json(body):Json<serde_json::Value>|async move {
        let key=headers.get("authorization").unwrap().to_str().unwrap().to_string();
        let requested_model = body["model"].as_str().unwrap_or("").to_string();
        calls.lock().unwrap().push((key.clone(),body));
        if requested_model == "gpt-4o" && matches!(mode, "model429" | "model404" | "model500" | "model400" | "mixed") {
            let status = match mode { "model404" => 404, "model500" | "mixed" => 500, "model400" => 400, _ => 429 };
            return json_error(StatusCode::from_u16(status).unwrap(), "primary model unavailable");
        }
        if mode == "mixed" { return json_error(StatusCode::TOO_MANY_REQUESTS, "backup rate limited"); }
        if mode=="all429" || (mode=="first429" && key.ends_with("key-a")) {
            let mut response=json_error(StatusCode::TOO_MANY_REQUESTS,&format!("rate limit {key}"));
            response.headers_mut().insert("retry-after",HeaderValue::from_static("1"));
            return response;
        }
        if mode=="stream_error" {
            let chunks=stream::once(async {Ok::<_,std::io::Error>(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n"))}).chain(stream::once(async {
                tokio::time::sleep(Duration::from_millis(25)).await;
                Ok::<_,std::io::Error>(Bytes::from_static(b"data: {\"error\":{\"message\":\"limited\",\"code\":429}}\n\n"))
            }));
            let mut response=Response::new(Body::from_stream(chunks));
            response.headers_mut().insert("content-type",HeaderValue::from_static("text/event-stream"));
            return response;
        }
        Json(json!({"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}})).into_response()
    })).with_state(calls.clone());
    let server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    let store = Store::open_in_memory().unwrap();
    let mut channel = Channel {
        name: "Supplier".into(),
        provider: "openai".into(),
        api_mode: "chat_completions".into(),
        endpoint: format!("http://{address}"),
        api_key: "key-a".into(),
        primary_model: "gpt-4o".into(),
        enabled: true,
        ..Default::default()
    };
    store.save(&mut channel).unwrap();
    store
        .add_key(
            channel.id,
            &KeyInput {
                name: "B".into(),
                secret: "key-b".into(),
            },
        )
        .unwrap();
    let router = Arc::new(Router::new(store.clone()));
    (Gateway::new(store, router), channel.id, calls, server)
}

fn request() -> Vec<u8> {
    serde_json::to_vec(&json!({"model":"auto","messages":[{"role":"user","content":"remember 42"},{"role":"assistant","tool_calls":[{"id":"call-1","type":"function","function":{"name":"test","arguments":"{}"}}]},{"role":"tool","tool_call_id":"call-1","content":"42"}]})).unwrap()
}

/// Models of `key_id` that are still cooling down, as
/// `(model, unavailable_until, rate_limit_count)`.
fn cooling_models(gateway: &Gateway, key_id: i64) -> Vec<(String, chrono::DateTime<Utc>, i64)> {
    let conn = gateway.store.lock();
    let mut stmt = conn
        .prepare(
            "SELECT model,unavailable_until,rate_limit_count FROM channel_key_models \
             WHERE key_id=? AND unavailable_until>? ORDER BY model",
        )
        .unwrap();
    stmt.query_map(rusqlite::params![key_id, Utc::now().to_rfc3339()], |row| {
        let until: String = row.get(1)?;
        Ok((
            row.get(0)?,
            chrono::DateTime::parse_from_rfc3339(&until)
                .unwrap()
                .with_timezone(&Utc),
            row.get(2)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

use axum::response::IntoResponse;

#[tokio::test]
async fn cancelled_and_expired_streams_are_not_successes() {
    for expired in [false, true] {
        let store = Store::open_in_memory().unwrap();
        let log = Arc::new(Mutex::new(RequestLog::default()));
        let mut logging = LoggingStream::new(
            Box::pin(stream::pending()),
            Arc::new(Mutex::new(StreamTokenTracker::new(0))),
            log.clone(),
            store,
            None,
            Instant::now(),
            false,
        );
        if expired {
            logging.deadline = Some(Box::pin(tokio::time::sleep_until(
                tokio::time::Instant::now(),
            )));
            assert!(logging.next().await.unwrap().is_err());
            assert!(logging.next().await.is_none());
        }
        drop(logging);
        assert_eq!(log.lock().unwrap().status_code, 502);
        assert!(!log.lock().unwrap().error_message.is_empty());
    }
}

#[tokio::test]
async fn exhausted_healthy_pool_can_fail_over_to_degraded_supplier() {
    let (gateway, id, calls, _server) = setup("ok").await;
    let mut backup = gateway.store.get(id).unwrap();
    backup.id = 0;
    backup.name = "Degraded backup".into();
    backup.last_status = "degraded".into();
    gateway.store.save(&mut backup).unwrap();
    for key in gateway.store.list_keys(id).unwrap() {
        gateway
            .store
            .patch_key(
                id,
                key.id,
                &crate::store_keys::KeyPatch {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(),
            &RequestContext::default(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    axum::body::to_bytes(response.into_body(), 10000)
        .await
        .unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(
        gateway.store.list_logs(10).unwrap()[0].channel_name,
        "Degraded backup"
    );
}

#[tokio::test]
async fn stream_error_after_output_never_replays() {
    let (gateway, id, calls, _server) = setup("stream_error").await;
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(),
            &RequestContext::default(),
        )
        .await;
    let bytes = axum::body::to_bytes(response.into_body(), 10000)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("hello"));
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(!cooling_models(&gateway, gateway.store.list_keys(id).unwrap()[0].id).is_empty());
    assert_eq!(gateway.store.list_logs(10).unwrap()[0].status_code, 502);
}

#[tokio::test]
async fn global_attempt_budget_and_supplier_failover() {
    let (gateway, id, calls, _server) = setup("all429").await;
    let mut other = gateway.store.get(id).unwrap();
    other.id = 0;
    other.name = "Supplier B".into();
    other.api_key = "key-c".into();
    gateway.store.save(&mut other).unwrap();
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(),
            &RequestContext::default(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(calls.lock().unwrap().len(), 3);
    assert!(!cooling_models(&gateway, gateway.store.list_keys(other.id).unwrap()[0].id).is_empty());
}

#[tokio::test]
async fn disabled_keys_cannot_be_probed() {
    let (gateway, id, calls, _server) = setup("ok").await;
    let key = gateway.store.list_keys(id).unwrap().remove(0);
    gateway
        .store
        .patch_key(
            id,
            key.id,
            &crate::store_keys::KeyPatch {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let runner =
        crate::runner::ProbeRunner::with_key_pool(gateway.store.clone(), gateway.key_pool.clone());
    assert!(runner.probe_key_now(id, key.id).await.is_err());
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn auto_retries_other_key_and_preserves_history() {
    let (gateway, id, calls, _server) = setup("first429").await;
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(),
            &RequestContext::default(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    axum::body::to_bytes(response.into_body(), 10000)
        .await
        .unwrap();
    let keys = gateway.store.list_keys(id).unwrap();
    let cooling = cooling_models(&gateway, keys[0].id);
    assert_eq!(cooling.len(), 1);
    assert!(cooling[0].1 > Utc::now() + chrono::Duration::seconds(290));
    assert!(keys[0].cooldown_until.is_none());
    assert_eq!(keys[1].successes, 1);
    assert_eq!(keys[1].prompt_tokens, 10);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1["messages"], calls[1].1["messages"]);
    assert_eq!(calls[1].1["messages"][2]["tool_call_id"], "call-1");
    let logs = gateway.store.list_logs(10).unwrap();
    assert!(!serde_json::to_string(&logs).unwrap().contains("key-a"));
    assert!(logs[0].trace_json.contains("key_id"));
}

#[tokio::test]
async fn all_cooling_keys_are_not_called_again() {
    let (gateway, id, calls, _server) = setup("all429").await;
    for _ in 0..2 {
        let response = gateway
            .forward(
                GatewayProtocol::Chat,
                &HeaderMap::new(),
                request(),
                &RequestContext::default(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(
            response
                .headers()
                .get("retry-after")
                .unwrap()
                .to_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                >= 298
        );
    }
    assert_eq!(calls.lock().unwrap().len(), 2);
    let cooling = cooling_models(&gateway, gateway.store.list_keys(id).unwrap()[0].id);
    assert_eq!(cooling.len(), 1);
    assert_eq!(cooling[0].2, 1);
}

#[tokio::test]
async fn ordinary_requests_rotate_keys() {
    let (gateway, _, calls, _server) = setup("ok").await;
    for _ in 0..4 {
        let response = gateway
            .forward(
                GatewayProtocol::Chat,
                &HeaderMap::new(),
                request(),
                &RequestContext::default(),
            )
            .await;
        axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
    }
    let keys: Vec<_> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|(key, _)| key.clone())
        .collect();
    assert_eq!(
        keys,
        [
            "Bearer key-a",
            "Bearer key-b",
            "Bearer key-a",
            "Bearer key-b"
        ]
    );
}

#[tokio::test]
async fn model_error_does_not_affect_other_models_of_same_supplier() {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route(
            "/v1/chat/completions",
            post(move |State(calls): State<Calls>, headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                let key = headers.get("authorization").unwrap().to_str().unwrap().to_string();
                let model = body["model"].as_str().unwrap_or("").to_string();
                calls.lock().unwrap().push((key.clone(), body));
                if model == "broken-model" {
                    return json_error(StatusCode::INTERNAL_SERVER_ERROR, "upstream model crashed");
                }
                Json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
                })).into_response()
            }),
        )
        .with_state(calls.clone());
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));

    let store = Store::open_in_memory().unwrap();
    let mut channel = Channel {
        name: "MultiModelSupplier".into(),
        provider: "openai".into(),
        api_mode: "chat_completions".into(),
        endpoint: format!("http://{address}"),
        api_key: "key-1".into(),
        primary_model: "broken-model".into(),
        extra_models: vec!["healthy-model".into()],
        enabled: true,
        ..Default::default()
    };
    store.save(&mut channel).unwrap();
    let router = Arc::new(Router::new(store.clone()));
    let gateway = Gateway::new(store.clone(), router);

    // 1. Request broken-model: should fail
    let req_broken = serde_json::to_vec(&json!({
        "model": "broken-model",
        "messages": [{"role": "user", "content": "hi"}]
    }))
    .unwrap();
    let res_broken = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            req_broken,
            &RequestContext::default(),
        )
        .await;
    assert!(res_broken.status().is_server_error());

    // 2. Request healthy-model on the SAME supplier: MUST SUCCEED and NOT be blocked by broken-model cooldown
    let req_healthy = serde_json::to_vec(&json!({
        "model": "healthy-model",
        "messages": [{"role": "user", "content": "hi"}]
    }))
    .unwrap();
    let res_healthy = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            req_healthy,
            &RequestContext::default(),
        )
        .await;
    assert_eq!(res_healthy.status(), StatusCode::OK);

    // 3. Probing the cooling key/model is never blocked by temporary retreat
    let runner =
        crate::runner::ProbeRunner::with_key_pool(gateway.store.clone(), gateway.key_pool.clone());
    let probe_results = runner
        .probe_model_now(channel.id, "healthy-model")
        .await
        .unwrap();
    assert_eq!(probe_results.len(), 1);
    assert!(!probe_results[0].message.contains("没有可用于探活的 Key"));
}

async fn forward_auto_for_strategy(gateway: &Gateway, strategy: &str, models: Option<Vec<String>>) {
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(),
            &RequestContext {
                strategy: strategy.into(),
                allowed_models: models,
                ..Default::default()
            },
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
}

fn add_routing_backup(gateway: &Gateway, id: i64) -> i64 {
    let mut backup = gateway.store.get(id).unwrap();
    backup.id = 0;
    backup.name = "Backup".into();
    backup.api_key = "backup-key".into();
    backup.last_status = "operational".into();
    backup.last_latency_ms = Some(500);
    backup.injection_state = "clean".into();
    gateway.store.save(&mut backup).unwrap();
    gateway
        .store
        .set_model_health(
            backup.id,
            &crate::store::ModelHealth {
                model: "gpt-4o".into(),
                status: "operational".into(),
                latency_ms: Some(500),
                ..Default::default()
            },
        )
        .unwrap();
    backup.id
}

#[tokio::test]
async fn auto_keeps_detected_injection_as_fallback_under_every_strategy() {
    for strategy in ["smart_quality", "latency", "round_robin", "manual_priority"] {
        let (gateway, id, calls, _server) = setup("ok").await;
        let mut fast = gateway.store.get(id).unwrap();
        fast.last_status = "operational".into();
        fast.last_latency_ms = Some(10);
        fast.injection_state = "detected".into();
        gateway.store.save(&mut fast).unwrap();
        add_routing_backup(&gateway, id);
        for _ in 0..3 {
            forward_auto_for_strategy(&gateway, strategy, None).await;
        }
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .all(|(key, _)| key == "Bearer backup-key"),
            "strategy: {strategy}"
        );
    }
}

#[tokio::test]
async fn auto_round_robin_rotates_suppliers_despite_different_changing_latencies() {
    let (gateway, id, calls, _server) = setup("ok").await;
    let mut fast = gateway.store.get(id).unwrap();
    fast.last_status = "operational".into();
    fast.last_latency_ms = Some(10);
    gateway.store.save(&mut fast).unwrap();
    add_routing_backup(&gateway, id);
    for _ in 0..4 {
        forward_auto_for_strategy(&gateway, "round_robin", None).await;
    }
    let backup_turns: Vec<_> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|(key, _)| key == "Bearer backup-key")
        .collect();
    assert_eq!(backup_turns, [false, true, false, true]);
}

#[tokio::test]
async fn auto_manual_priority_selects_the_first_model_even_when_another_is_faster() {
    let (gateway, id, calls, _server) = setup("ok").await;
    let mut channel = gateway.store.get(id).unwrap();
    channel.last_status = "operational".into();
    channel.last_latency_ms = Some(500);
    channel.extra_models = vec!["gpt-4.1".into()];
    gateway.store.save(&mut channel).unwrap();
    gateway
        .store
        .set_model_health(
            id,
            &crate::store::ModelHealth {
                model: "gpt-4o".into(),
                status: "operational".into(),
                latency_ms: Some(500),
                ..Default::default()
            },
        )
        .unwrap();
    gateway
        .store
        .set_model_health(
            id,
            &crate::store::ModelHealth {
                model: "gpt-4.1".into(),
                status: "operational".into(),
                latency_ms: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    for (strategy, expected) in [("manual_priority", "gpt-4o"), ("latency", "gpt-4.1")] {
        forward_auto_for_strategy(
            &gateway,
            strategy,
            Some(vec!["gpt-4o".into(), "gpt-4.1".into()]),
        )
        .await;
        assert_eq!(calls.lock().unwrap().last().unwrap().1["model"], expected);
    }
}

#[tokio::test]
async fn auto_retains_half_open_recovery_priority() {
    let (gateway, id, calls, _server) = setup("ok").await;
    let mut healthy = gateway.store.get(id).unwrap();
    healthy.last_status = "operational".into();
    healthy.last_latency_ms = Some(10);
    gateway.store.save(&mut healthy).unwrap();
    let backup_id = add_routing_backup(&gateway, id);
    gateway
        .store
        .update_runtime(backup_id, |c| {
            c.circuit_state = CIRCUIT_HALF_OPEN.into();
        })
        .unwrap();
    forward_auto_for_strategy(&gateway, "smart_quality", None).await;
    assert_eq!(calls.lock().unwrap()[0].0, "Bearer backup-key");
}

fn add_backup_model(gateway: &Gateway, id: i64) {
    let mut channel = gateway.store.get(id).unwrap();
    channel.extra_models = vec!["gpt-4.1".into()];
    gateway.store.save(&mut channel).unwrap();
}

#[tokio::test]
async fn auto_retries_same_supplier_backup_model_within_shared_budgets() {
    for mode in ["model429", "model404", "model500"] {
        let (gateway, id, calls, _server) = setup(mode).await;
        add_backup_model(&gateway, id);
        gateway
            .store
            .add_key(
                id,
                &KeyInput {
                    name: "third".into(),
                    secret: "key-c".into(),
                },
            )
            .unwrap();
        forward_auto_for_strategy(
            &gateway,
            "manual_priority",
            Some(vec!["gpt-4o".into(), "gpt-4.1".into()]),
        )
        .await;
        let calls = calls.lock().unwrap();
        assert!(calls.len() <= 3);
        assert_eq!(calls.last().unwrap().1["model"], "gpt-4.1");
        assert!(
            calls
                .iter()
                .map(|(key, _)| key)
                .collect::<std::collections::HashSet<_>>()
                .len()
                <= 2
        );
        let original: serde_json::Value = serde_json::from_slice(&request()).unwrap();
        assert!(calls
            .iter()
            .all(|(_, body)| body["messages"] == original["messages"]));
    }
}

#[tokio::test]
async fn backup_models_do_not_expand_attempt_or_key_budgets() {
    let (gateway, id, calls, _server) = setup("mixed").await;
    add_backup_model(&gateway, id);
    gateway
        .store
        .add_key(
            id,
            &KeyInput {
                name: "third".into(),
                secret: "key-c".into(),
            },
        )
        .unwrap();
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(),
            &RequestContext {
                strategy: "manual_priority".into(),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(
        calls
            .iter()
            .map(|(key, _)| key)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2
    );
    assert_eq!(calls[0].1["model"], "gpt-4o");
    assert!(calls[1..]
        .iter()
        .all(|(_, body)| body["model"] == "gpt-4.1"));
}

#[tokio::test]
async fn auto_skips_a_cooling_model_but_respects_the_model_allowlist() {
    let (gateway, id, calls, _server) = setup("ok").await;
    add_backup_model(&gateway, id);
    for key in gateway.store.list_keys(id).unwrap() {
        gateway
            .key_pool
            .acquire(id, "gpt-4o", &Default::default(), Some(key.id))
            .unwrap()
            .unwrap()
            .failure(&FailureInfo::new(FailureClass::RateLimit), "gpt-4o")
            .unwrap();
    }
    let ctx = RequestContext {
        strategy: "manual_priority".into(),
        allowed_models: Some(vec!["gpt-4o".into()]),
        ..Default::default()
    };
    let response = gateway
        .forward(GatewayProtocol::Chat, &HeaderMap::new(), request(), &ctx)
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(calls.lock().unwrap().is_empty());
    forward_auto_for_strategy(
        &gateway,
        "manual_priority",
        Some(vec!["gpt-4o".into(), "gpt-4.1".into()]),
    )
    .await;
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(calls.lock().unwrap()[0].1["model"], "gpt-4.1");
}

#[tokio::test]
async fn backup_models_are_not_tried_for_client_errors_or_after_output() {
    for mode in ["model400", "stream_error"] {
        let (gateway, id, calls, _server) = setup(mode).await;
        add_backup_model(&gateway, id);
        let response = gateway
            .forward(
                GatewayProtocol::Chat,
                &HeaderMap::new(),
                request(),
                &RequestContext {
                    strategy: "manual_priority".into(),
                    ..Default::default()
                },
            )
            .await;
        let _ = axum::body::to_bytes(response.into_body(), 1 << 20).await;
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
}
