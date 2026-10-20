use super::*;
use axum::{routing::post, Json};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn serve(app: axum::Router) -> (String, Server) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    (format!("http://{address}"), server)
}

fn seed(store: &Store, name: &str, endpoint: &str, provider: &str, mode: &str) -> Channel {
    let mut channel = Channel {
        name: name.into(),
        endpoint: endpoint.into(),
        provider: provider.into(),
        api_mode: mode.into(),
        api_key: "dummy-credential".into(),
        primary_model: "model".into(),
        enabled: true,
        last_status: "operational".into(),
        ..Default::default()
    };
    store.save(&mut channel).unwrap();
    channel
}

fn gateway(store: &Arc<Store>) -> Gateway {
    Gateway::new(store.clone(), Arc::new(Router::new(store.clone())))
}

fn ordered() -> RequestContext {
    RequestContext {
        strategy: crate::router::ROUTE_MANUAL_PRIORITY.into(),
        allowed_channels: Some(vec!["broken".into(), "backup".into()]),
        ..Default::default()
    }
}

fn request(stream: bool) -> Vec<u8> {
    serde_json::to_vec(&json!({"model":"model","stream":stream,"max_tokens":16,
        "messages":[{"role":"user","content":"ping"}],"input":"ping"}))
    .unwrap()
}

#[tokio::test]
async fn old_primary_outcome_keeps_model_health_but_not_replacement_headline() {
    let store = Store::open_in_memory().unwrap();
    let mut channel = seed(
        &store,
        "source",
        "https://upstream.test",
        "openai",
        "chat_completions",
    );
    channel.extra_models = vec!["replacement".into()];
    store.save(&mut channel).unwrap();
    let gateway = gateway(&store);
    let lease = gateway
        .key_pool
        .acquire_for_channel(&channel, "model", &HashSet::new(), None)
        .unwrap()
        .unwrap();
    let log = Arc::new(Mutex::new(RequestLog {
        upstream_model: "model".into(),
        ..Default::default()
    }));
    let mut logging = LoggingStream::new(
        single_chunk(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_vec()),
        Arc::new(Mutex::new(StreamTokenTracker::default())), log, store.clone(), Some(channel.clone()),
        Instant::now() - Duration::from_millis(750), true);
    logging.key_lease = Some(lease);
    logging.router = Some(gateway.router.clone());
    channel.primary_model = "replacement".into();
    channel.extra_models = vec!["model".into()];
    store.save(&mut channel).unwrap();
    while let Some(chunk) = logging.next().await {
        chunk.unwrap();
    }
    let saved = store.get(channel.id).unwrap();
    assert_eq!(saved.primary_model, "replacement");
    assert_eq!(saved.last_status, "operational");
    assert_eq!(saved.last_latency_ms, None);
    assert!(saved
        .model_health
        .iter()
        .any(|health| health.model == "model" && health.status == "operational"));
    assert!(!saved
        .model_health
        .iter()
        .any(|health| health.model == "replacement"));
}

#[tokio::test]
async fn multiline_sse_events_preserve_text_usage_and_translated_output() {
    let wire = r#"data: {"choices":[{
data: "delta":{"content":"actual answer"}}]}

data: {"choices":[{
data: "delta":{},"finish_reason":"stop"}]}

data: {"choices":[],
data: "usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}}

data: [DONE]

"#;
    for proto in [GatewayProtocol::Chat, GatewayProtocol::Anthropic] {
        let body = wire.to_string();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let body = body.clone();
                async move {
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        );
        let (endpoint, _server) = serve(app).await;
        let store = Store::open_in_memory().unwrap();
        let channel = seed(&store, "broken", &endpoint, "openai", "chat_completions");
        let response = gateway(&store)
            .forward(proto, &HeaderMap::new(), request(true), &ordered())
            .await;
        let status = response.status();
        let output = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        let output = String::from_utf8_lossy(&output);
        if proto == GatewayProtocol::Chat {
            assert!(output.contains("actual answer"));
        } else {
            assert!(output.contains("actual answer"));
            assert!(output.contains("message_stop"));
        }
        let log = store.list_logs(1).unwrap().remove(0);
        assert_eq!(
            (
                log.status_code,
                log.prompt_tokens,
                log.completion_tokens,
                log.total_tokens
            ),
            (200, 100, 20, 120)
        );
        assert_eq!(store.list_keys(channel.id).unwrap()[0].successes, 1);
    }
}

#[tokio::test]
async fn failover_never_combines_a_new_credential_with_an_old_endpoint() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let received = Arc::new(Mutex::new(Vec::new()));
    let app = axum::Router::new()
        .route(
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
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                }
            }),
        )
        .fallback({
            let received = received.clone();
            move |uri: axum::http::Uri, headers: HeaderMap| {
                let received = received.clone();
                async move {
                    received.lock().unwrap().push((
                        uri.path().to_string(),
                        headers["authorization"].to_str().unwrap().to_string(),
                    ));
                    Json(json!({"choices":[{"message":{"content":"ok"}}]}))
                }
            }
        });
    let (endpoint, _server) = serve(app).await;
    let store = Store::open_in_memory().unwrap();
    seed(&store, "broken", &endpoint, "openai", "chat_completions");
    let mut backup = seed(
        &store,
        "backup",
        &format!("{endpoint}/old"),
        "openai",
        "chat_completions",
    );
    let gateway = Arc::new(gateway(&store));
    let call = tokio::spawn({
        let gateway = gateway.clone();
        async move {
            gateway
                .forward(
                    GatewayProtocol::Chat,
                    &HeaderMap::new(),
                    request(false),
                    &ordered(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    backup.endpoint = format!("{endpoint}/new");
    store.save(&mut backup).unwrap();
    let key = store.list_keys(backup.id).unwrap().remove(0);
    store
        .patch_key(
            backup.id,
            key.id,
            &crate::store_keys::KeyPatch {
                secret: Some("new-secret".into()),
                ..Default::default()
            },
        )
        .unwrap();
    release.notify_one();
    let response = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .unwrap()
        .unwrap();
    assert!(!response.status().is_success());
    assert!(received.lock().unwrap().is_empty());
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(false),
            &ordered(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(
        *received.lock().unwrap(),
        vec![(
            "/new/v1/chat/completions".into(),
            "Bearer new-secret".into()
        )]
    );
}

#[tokio::test]
async fn translated_and_native_json_responses_share_the_same_buffer_limit() {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let mut value = json!({"choices":[{"message":{"content":"ok"}}]});
            value["padding"] = json!("x".repeat(MAX_REQUEST_BODY + 1));
            let body = value.to_string();
            let chunks: Vec<_> = body
                .as_bytes()
                .chunks(32 << 10)
                .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
                .collect();
            Response::builder()
                .header("content-type", "application/json")
                .body(Body::from_stream(stream::iter(chunks)))
                .unwrap()
        }),
    );
    let (endpoint, _server) = serve(app).await;
    for proto in [GatewayProtocol::Chat, GatewayProtocol::Anthropic] {
        let store = Store::open_in_memory().unwrap();
        let channel = seed(&store, "broken", &endpoint, "openai", "chat_completions");
        let response = gateway(&store)
            .forward(proto, &HeaderMap::new(), request(false), &ordered())
            .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{proto:?}");
        assert_eq!(store.list_keys(channel.id).unwrap()[0].successes, 0);
        assert_eq!(store.list_logs(1).unwrap()[0].status_code, 502);
    }
}

#[tokio::test]
async fn oversized_sse_lines_after_output_fail_native_and_translated_streams() {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let mut chunks = vec![Ok::<_, std::io::Error>(Bytes::from_static(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            ))];
            chunks.extend((0..33).map(|_| Ok(Bytes::from(vec![b'x'; 32 << 10]))));
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(stream::iter(chunks)))
                .unwrap()
        }),
    );
    let (endpoint, _server) = serve(app).await;
    for proto in [GatewayProtocol::Chat, GatewayProtocol::Anthropic] {
        let store = Store::open_in_memory().unwrap();
        let channel = seed(&store, "broken", &endpoint, "openai", "chat_completions");
        let response = gateway(&store)
            .forward(proto, &HeaderMap::new(), request(true), &ordered())
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(axum::body::to_bytes(response.into_body(), 2 << 20)
            .await
            .is_err());
        assert_eq!(store.list_keys(channel.id).unwrap()[0].successes, 0);
        assert_eq!(store.list_logs(1).unwrap()[0].status_code, 502);
    }
}

#[tokio::test]
async fn translated_missing_usage_keeps_the_prompt_estimate() {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(|| async { Json(json!({"choices":[{"message":{"content":"ok"}}]})) }),
    );
    let (endpoint, _server) = serve(app).await;
    let store = Store::open_in_memory().unwrap();
    seed(&store, "broken", &endpoint, "openai", "chat_completions");
    let response = gateway(&store)
        .forward(
            GatewayProtocol::Anthropic,
            &HeaderMap::new(),
            request(false),
            &ordered(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(store.list_logs(1).unwrap()[0].prompt_tokens > 0);
}

#[tokio::test]
async fn chat_streams_need_all_requested_choices_or_an_authoritative_done() {
    for (second_seen, second_finished, done) in [
        (true, false, false),
        (false, false, false),
        (true, true, false),
        (true, false, true),
    ] {
        let mut wire = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"one\"},\"finish_reason\":\"stop\"}]}\n\n".to_string();
        if second_seen {
            wire += &format!("data: {{\"choices\":[{{\"index\":1,\"delta\":{{\"content\":\"two\"}},\"finish_reason\":{}}}]}}\n\n", if second_finished { "\"stop\"" } else { "null" });
        }
        if done {
            wire += "data: [DONE]\n\n";
        }
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let wire = wire.clone();
                async move {
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from(wire))
                        .unwrap()
                }
            }),
        );
        let (endpoint, _server) = serve(app).await;
        let store = Store::open_in_memory().unwrap();
        let channel = seed(&store, "broken", &endpoint, "openai", "chat_completions");
        let gateway = gateway(&store);
        let mut body: serde_json::Value = serde_json::from_slice(&request(true)).unwrap();
        body["n"] = json!(2);
        let response = gateway
            .forward(
                GatewayProtocol::Chat,
                &HeaderMap::new(),
                serde_json::to_vec(&body).unwrap(),
                &ordered(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let succeeded = second_finished || done;
        assert_eq!(
            store.list_logs(1).unwrap()[0].status_code,
            if succeeded { 200 } else { 502 }
        );
        assert_eq!(
            store.list_keys(channel.id).unwrap()[0].successes,
            i64::from(succeeded)
        );
    }
}

#[tokio::test]
async fn exact_zero_nonstream_usage_is_not_replaced_by_prompt_estimates() {
    for (proto, provider, mode, path, reply) in [
        (
            GatewayProtocol::Chat,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
            json!({"choices":[{"message":{"content":""}}],"usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}}),
        ),
        (
            GatewayProtocol::Responses,
            "openai",
            "responses",
            "/v1/responses",
            json!({"status":"completed","output":[],"usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}}),
        ),
        (
            GatewayProtocol::Anthropic,
            "anthropic",
            "chat_completions",
            "/v1/messages",
            json!({"type":"message","content":[],"usage":{"input_tokens":0,"output_tokens":0}}),
        ),
        (
            GatewayProtocol::Anthropic,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
            json!({"choices":[{"message":{"content":""}}],"usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}}),
        ),
    ] {
        let app = axum::Router::new().route(
            path,
            post(move || {
                let reply = reply.clone();
                async move { Json(reply) }
            }),
        );
        let (endpoint, _server) = serve(app).await;
        let store = Store::open_in_memory().unwrap();
        let channel = seed(&store, "broken", &endpoint, provider, mode);
        let response = gateway(&store)
            .forward(proto, &HeaderMap::new(), request(false), &ordered())
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let log = store.list_logs(1).unwrap().remove(0);
        assert_eq!(
            (log.prompt_tokens, log.completion_tokens, log.total_tokens),
            (0, 0, 0),
            "{proto:?}/{provider}"
        );
        let key = store.list_keys(channel.id).unwrap().remove(0);
        assert_eq!(
            (key.prompt_tokens, key.completion_tokens, key.successes),
            (0, 0, 1)
        );
    }
}

#[tokio::test]
async fn redirects_cannot_send_native_credentials_to_another_origin() {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let (sink, _sink_server) = serve(axum::Router::new().route(
        "/capture",
        post(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async { Json(json!({"type":"message","content":[]})) }
        }),
    ))
    .await;
    let app = axum::Router::new().route(
        "/v1/messages",
        post(move || {
            let target = format!("{sink}/capture");
            async move {
                Response::builder()
                    .status(307)
                    .header("location", target)
                    .body(Body::empty())
                    .unwrap()
            }
        }),
    );
    let (endpoint, _server) = serve(app).await;
    let store = Store::open_in_memory().unwrap();
    seed(&store, "broken", &endpoint, "anthropic", "chat_completions");
    let response = gateway(&store)
        .forward(
            GatewayProtocol::Anthropic,
            &HeaderMap::new(),
            request(false),
            &ordered(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_anthropic_requests_never_attempt_or_mutate_channels() {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let (endpoint, _server) = serve(axum::Router::new().route(
        "/v1/chat/completions",
        post(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async { Json(json!({"choices":[{"message":{"content":"ok"}}]})) }
        }),
    ))
    .await;
    let store = Store::open_in_memory().unwrap();
    let channel = seed(&store, "broken", &endpoint, "openai", "chat_completions");
    let gateway = gateway(&store);
    for messages in [
        json!([]),
        json!([{"role":"invalid","content":"hi"}]),
        json!("wrong type"),
    ] {
        let response = gateway
            .forward(
                GatewayProtocol::Anthropic,
                &HeaderMap::new(),
                serde_json::to_vec(&json!({"model":"model","max_tokens":16,"messages":messages}))
                    .unwrap(),
                &ordered(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(store.list_keys(channel.id).unwrap()[0].attempts, 0);
    assert!(store.get(channel.id).unwrap().model_health.is_empty());
    assert!(gateway
        .key_pool
        .retry_after_model(channel.id, "model")
        .is_zero());
    let response = gateway
        .forward(
            GatewayProtocol::Anthropic,
            &HeaderMap::new(),
            request(false),
            &ordered(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
}

#[tokio::test]
async fn only_semantic_output_commits_and_upstream_truncation_cools_the_model() {
    for role_only in [false, true] {
        for transport_error in [false, true] {
            let hits = Arc::new(AtomicUsize::new(0));
            let seen = hits.clone();
            let app = axum::Router::new().route("/v1/chat/completions", post(move || {
                seen.fetch_add(1, Ordering::SeqCst);
                async move {
                    let first = if role_only {
                        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n"
                    } else { "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n" };
                    let chunks = stream::once(async move { Ok::<_, std::io::Error>(Bytes::from_static(first.as_bytes())) })
                        .chain(stream::once(async move {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            if transport_error { Err(std::io::Error::other("cut")) } else { Ok(Bytes::new()) }
                        }));
                    Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(chunks)).unwrap()
                }
            })).route("/backup/v1/chat/completions", post(|| async {
                Response::builder().header("content-type", "text/event-stream")
                    .body(Body::from("data: {\"choices\":[{\"delta\":{\"content\":\"backup\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")).unwrap()
            }));
            let (endpoint, _server) = serve(app).await;
            let store = Store::open_in_memory().unwrap();
            let broken = seed(&store, "broken", &endpoint, "openai", "chat_completions");
            let backup = seed(
                &store,
                "backup",
                &format!("{endpoint}/backup"),
                "openai",
                "chat_completions",
            );
            let gateway = gateway(&store);
            for attempt in 0..2 {
                let response = gateway
                    .forward(
                        GatewayProtocol::Chat,
                        &HeaderMap::new(),
                        request(true),
                        &ordered(),
                    )
                    .await;
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    response.headers()["x-heigate-channel"],
                    if role_only || attempt == 1 {
                        "backup"
                    } else {
                        "broken"
                    }
                );
                let body = axum::body::to_bytes(response.into_body(), 1 << 20).await;
                if !role_only && attempt == 0 && transport_error {
                    assert!(body.is_err());
                } else {
                    assert!(body.is_ok());
                }
            }
            assert_eq!(hits.load(Ordering::SeqCst), 1);
            assert_eq!(store.list_keys(broken.id).unwrap()[0].successes, 0);
            assert_eq!(
                store.list_keys(backup.id).unwrap()[0].successes,
                if role_only { 2 } else { 1 }
            );
            assert!(!gateway
                .key_pool
                .retry_after_model(broken.id, "model")
                .is_zero());
            let saved = store.get(broken.id).unwrap();
            assert_eq!(saved.last_status, "error");
            assert_eq!(saved.model_health[0].status, "error");
            if !role_only {
                assert!(store
                    .list_logs(5)
                    .unwrap()
                    .iter()
                    .any(|log| log.status_code == 502));
            }
        }
    }
}

#[tokio::test]
async fn client_cancellation_preserves_health_and_never_engages_cooldown() {
    let (endpoint, _server) = serve(axum::Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let chunks = stream::once(async {
                Ok::<_, std::io::Error>(Bytes::from_static(
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
                ))
            })
            .chain(stream::pending());
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(chunks))
                .unwrap()
        }),
    ))
    .await;
    let store = Store::open_in_memory().unwrap();
    let channel = seed(&store, "broken", &endpoint, "openai", "chat_completions");
    let gateway = gateway(&store);
    drop(
        gateway
            .forward(
                GatewayProtocol::Chat,
                &HeaderMap::new(),
                request(true),
                &ordered(),
            )
            .await,
    );
    assert_eq!(store.get(channel.id).unwrap().last_status, "operational");
    assert!(store.get(channel.id).unwrap().model_health.is_empty());
    assert!(gateway
        .key_pool
        .retry_after_model(channel.id, "model")
        .is_zero());
    assert_eq!(store.list_keys(channel.id).unwrap()[0].successes, 0);
}

#[tokio::test]
async fn client_errors_do_not_poison_models_and_success_restores_health_without_probe_timings() {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(|Json(body): Json<serde_json::Value>| async move {
            if body["metadata"]["invalid"] == true {
                return json_error(StatusCode::BAD_REQUEST, "bad client parameter");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            Response::new(Body::from(
                json!({"choices":[{"message":{"content":"ok"}}]}).to_string(),
            ))
        }),
    );
    let (endpoint, _server) = serve(app).await;
    let store = Store::open_in_memory().unwrap();
    let channel = seed(&store, "broken", &endpoint, "openai", "chat_completions");
    let gateway = gateway(&store);
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            serde_json::to_vec(&json!({"model":"model","metadata":{"invalid":true}})).unwrap(),
            &ordered(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(store.get(channel.id).unwrap().model_health.is_empty());
    assert_eq!(store.get(channel.id).unwrap().last_status, "operational");
    assert!(gateway
        .key_pool
        .retry_after_model(channel.id, "model")
        .is_zero());
    store
        .set_model_health(
            channel.id,
            &crate::store::ModelHealth {
                model: "model".into(),
                status: "error".into(),
                latency_ms: Some(999),
                ttft_ms: Some(888),
                error: "previous failure".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(false),
            &ordered(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let saved = store.get(channel.id).unwrap();
    assert_eq!(saved.model_health[0].status, "operational");
    assert!(saved.model_health[0].error.is_empty());
    assert_eq!(saved.model_health[0].ttft_ms, Some(888));
    assert_eq!(saved.model_health[0].latency_ms, Some(999));
    let logs = store.list_logs(5).unwrap();
    assert!(logs.iter().all(|log| log.ttft_ms == 0));
    assert!(
        logs.iter()
            .find(|log| log.status_code == 200)
            .unwrap()
            .latency_ms
            >= 20
    );
}

#[tokio::test]
async fn empty_json_is_not_an_inference_in_any_protocol_including_translation() {
    for (proto, provider, mode, path, valid) in [
        (
            GatewayProtocol::Chat,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
            json!({"choices":[{"message":{"content":""},"finish_reason":"length"}]}),
        ),
        (
            GatewayProtocol::Anthropic,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
            json!({"choices":[{"message":{"content":null},"finish_reason":"content_filter"}]}),
        ),
        (
            GatewayProtocol::Anthropic,
            "anthropic",
            "chat_completions",
            "/v1/messages",
            json!({"type":"message","content":[],"stop_reason":"max_tokens"}),
        ),
        (
            GatewayProtocol::Responses,
            "openai",
            "responses",
            "/v1/responses",
            json!({"status":"incomplete","output":[],"incomplete_details":{"reason":"max_output_tokens"}}),
        ),
    ] {
        let app = axum::Router::new()
            .route(path, post(|| async { Json(json!({})) }))
            .route(
                &format!("/backup{path}"),
                post(move || {
                    let body = valid.clone();
                    async move { Json(body) }
                }),
            );
        let (endpoint, _server) = serve(app).await;
        let store = Store::open_in_memory().unwrap();
        let broken = seed(&store, "broken", &endpoint, provider, mode);
        let backup = seed(
            &store,
            "backup",
            &format!("{endpoint}/backup"),
            provider,
            mode,
        );
        let response = gateway(&store)
            .forward(proto, &HeaderMap::new(), request(false), &ordered())
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{proto:?}/{provider}");
        assert_eq!(response.headers()["x-heigate-channel"], "backup");
        axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(store.list_keys(broken.id).unwrap()[0].successes, 0);
        assert_eq!(store.list_keys(backup.id).unwrap()[0].successes, 1);
    }
}

#[tokio::test]
async fn stale_stream_results_cannot_penalize_new_configuration_or_recover_a_newer_failure() {
    for changed_config in [false, true] {
        let store = Store::open_in_memory().unwrap();
        let channel = seed(
            &store,
            "source",
            "https://old.test",
            "openai",
            "chat_completions",
        );
        let gateway = gateway(&store);
        let lease = gateway
            .key_pool
            .acquire(channel.id, "model", &std::collections::HashSet::new(), None)
            .unwrap()
            .unwrap();
        let started_at = chrono::Utc::now() - chrono::Duration::seconds(2);
        let log = Arc::new(Mutex::new(RequestLog {
            upstream_model: "model".into(),
            ..Default::default()
        }));
        let wire = if changed_config {
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n"
        } else {
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\n"
        };
        let mut logging = LoggingStream::new(
            single_chunk(wire.as_bytes().to_vec()),
            Arc::new(Mutex::new(StreamTokenTracker::default())),
            log,
            store.clone(),
            Some(channel.clone()),
            Instant::now(),
            true,
        );
        logging.key_lease = Some(lease);
        logging.router = Some(gateway.router.clone());
        logging.outcome_started_at = started_at;
        if changed_config {
            store
                .update_runtime(channel.id, |fresh| {
                    fresh.endpoint = "https://new.test".into()
                })
                .unwrap();
        } else {
            gateway.router.apply_live_outcome(
                &channel,
                "model",
                Some(&FailureInfo::new(FailureClass::Network)),
                "new failure",
                20,
                chrono::Utc::now(),
            );
        }
        while let Some(chunk) = logging.next().await {
            chunk.unwrap();
        }
        let saved = store.get(channel.id).unwrap();
        if changed_config {
            assert_eq!(saved.last_status, "unknown");
            assert!(saved.model_health.is_empty());
            assert!(gateway
                .key_pool
                .retry_after_model(channel.id, "model")
                .is_zero());
        } else {
            assert_eq!(saved.last_status, "error");
            assert_eq!(saved.model_health[0].status, "error");
            assert_eq!(saved.model_health[0].error, "new failure");
        }
    }
}

#[tokio::test]
async fn half_open_claim_lasts_until_the_client_stream_finishes_or_is_cancelled() {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let chunks = stream::once(async {
                Ok::<_, std::io::Error>(Bytes::from_static(
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
                ))
            })
            .chain(stream::pending());
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(chunks))
                .unwrap()
        }),
    );
    let (endpoint, _server) = serve(app).await;
    let store = Store::open_in_memory().unwrap();
    let channel = seed(&store, "broken", &endpoint, "openai", "chat_completions");
    store
        .update_runtime(channel.id, |fresh| {
            fresh.last_status = "error".into();
            fresh.circuit_state = "half_open".into();
        })
        .unwrap();
    let gateway = gateway(&store);
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(true),
            &ordered(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!gateway.router.claim_half_open_public(channel.id));
    drop(response);
    assert!(gateway.router.claim_half_open_public(channel.id));
    gateway.router.release_half_open_public(channel.id);
}

#[tokio::test]
async fn stream_prefix_limit_without_semantic_output_remains_a_failure() {
    let role = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n";
    let wire = role.repeat(MAX_STREAM_PREFIX_BYTES / role.len() + 1);
    let (_, _, error) = read_initial_prefix(single_chunk(wire.into_bytes()), true).await;
    assert!(error.unwrap().to_string().contains("prefix limit"));
    for frame in [
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"search\"}}]}}]}\n\n",
        "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"name\":\"search\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"name\":\"search\"}}\n\n",
    ] {
        let (_, _, error) = read_initial_prefix(single_chunk(frame.as_bytes().to_vec()), true).await;
        assert!(error.is_none(), "{frame}");
    }
}

#[tokio::test]
async fn initial_sse_output_and_remaining_bytes_are_independent_of_transport_chunks() {
    for newline in ["\n", "\r\n"] {
        let output = format!(
            "data: {}{newline}{newline}",
            json!({"choices":[{"delta":{"content":"x".repeat(70 << 10)}}]})
        );
        let prefix = format!(
            ": keepalive{newline}{newline}data: {{\"choices\":[{{\"delta\":{{\"role\":\"assistant\"}}}}]}}{newline}{newline}{output}"
        );
        let wire = format!(
            "{prefix}data: {{\"choices\":[{{\"delta\":{{\"content\":\"tail\"}},\"finish_reason\":\"stop\"}}]}}{newline}{newline}data: [DONE]{newline}{newline}"
        );
        for chunk_size in [usize::MAX, 1, 17, 64 << 10] {
            let chunks: Vec<_> = wire
                .as_bytes()
                .chunks(chunk_size)
                .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
                .collect();
            let (mut read, mut rest, error) =
                read_initial_prefix(Box::pin(stream::iter(chunks)), true).await;
            assert!(error.is_none(), "{newline:?}/{chunk_size}: {error:?}");
            assert_eq!(read, prefix.as_bytes());
            while let Some(chunk) = rest.next().await {
                read.extend_from_slice(&chunk.unwrap());
            }
            assert_eq!(read, wire.as_bytes(), "{newline:?}/{chunk_size}");
        }
    }
}

#[tokio::test]
async fn prefix_limits_still_reject_excessive_metadata_and_oversized_output_lines() {
    let metadata = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n".repeat(2000);
    let oversized = format!(
        "data: {}\n\n",
        json!({"choices":[{"delta":{"content":"x".repeat(crate::protocol::stream::MAX_SSE_LINE_BYTES)}}]})
    );
    for wire in [metadata, oversized] {
        for chunk_size in [usize::MAX, 32 << 10] {
            let chunks: Vec<_> = wire
                .as_bytes()
                .chunks(chunk_size)
                .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
                .collect();
            let (prefix, _, error) =
                read_initial_prefix(Box::pin(stream::iter(chunks)), true).await;
            assert!(error.unwrap().to_string().contains("limit"));
            assert!(
                prefix.len()
                    <= MAX_STREAM_PREFIX_BYTES + crate::protocol::stream::MAX_SSE_LINE_BYTES
            );
        }
    }
}

#[tokio::test]
async fn translated_sse_accepts_large_chunks_containing_only_bounded_lines() {
    let output = format!(
        "data: {}\n\n",
        json!({"choices":[{"delta":{"content":"x".repeat(512)}}]})
    );
    let wire = format!(
        "{}data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"length\"}}],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n",
        output.repeat(2200)
    );
    assert!(wire.len() > crate::protocol::stream::MAX_SSE_LINE_BYTES);
    for chunk_size in [usize::MAX, 4093] {
        let chunks: Vec<_> = wire
            .as_bytes()
            .chunks(chunk_size)
            .map(|chunk| Ok::<_, reqwest::Error>(Bytes::copy_from_slice(chunk)))
            .collect();
        let mut translated = Box::pin(translated_stream(
            Box::pin(stream::iter(chunks)),
            "model".into(),
        ));
        let mut tracker = StreamTokenTracker::default();
        tracker.translated_from_chat = true;
        while let Some(chunk) = translated.next().await {
            tracker.feed(&chunk.unwrap());
        }
        assert!(tracker.has_authoritative_end());
        assert!(tracker.stream_error.is_empty());
        assert_eq!(tracker.finish_reason, "max_tokens");
        assert_eq!(
            (
                tracker.prompt_tokens,
                tracker.completion_tokens,
                tracker.total_tokens
            ),
            (100, 20, 120)
        );
    }
}

#[tokio::test]
async fn stale_requests_keep_log_ttft_but_cannot_update_live_model_performance() {
    for change in [
        "none",
        "key",
        "removed model",
        "upstream",
        "disabled",
        "model epoch",
        "newer health",
        "newer sample",
    ] {
        let store = Store::open_in_memory().unwrap();
        let mut channel = seed(
            &store,
            "broken",
            "https://old.test",
            "openai",
            "chat_completions",
        );
        let gateway = gateway(&store);
        let lease = gateway
            .key_pool
            .acquire(channel.id, "model", &HashSet::new(), None)
            .unwrap()
            .unwrap();
        let now = chrono::Utc::now();
        let started_at = now - chrono::Duration::seconds(10);
        let original = channel.clone();
        match change {
            "key" => {
                store
                    .patch_key(
                        channel.id,
                        lease.key.id,
                        &crate::store_keys::KeyPatch {
                            secret: Some("replacement-secret".into()),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            "removed model" => {
                channel.primary_model = "replacement".into();
                store.save(&mut channel).unwrap();
                channel.primary_model = "model".into();
                store.save(&mut channel).unwrap();
            }
            "upstream" => {
                channel.endpoint = "https://new.test".into();
                store.save(&mut channel).unwrap();
            }
            "disabled" => {
                channel.enabled = false;
                store.save(&mut channel).unwrap();
            }
            "model epoch" => {
                lease
                    .failure(&FailureInfo::new(FailureClass::RateLimit), "model")
                    .unwrap();
            }
            "newer health" => {
                store
                    .record_model_outcome(&channel, "model", false, "newer failure", now)
                    .unwrap();
            }
            "newer sample" => {
                store
                    .record_model_ttft(&channel, "model", 777, now)
                    .unwrap();
            }
            _ => {}
        }
        let wire = b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let mut ttft = StreamTtftTracker::default();
        ttft.feed(wire, 100);
        let log = Arc::new(Mutex::new(RequestLog {
            upstream_model: "model".into(),
            ..Default::default()
        }));
        let mut logging = LoggingStream::new(
            single_chunk(wire.to_vec()),
            Arc::new(Mutex::new(StreamTokenTracker::default())),
            log.clone(),
            store.clone(),
            Some(original),
            Instant::now(),
            true,
        );
        logging.key_lease = Some(lease);
        logging.router = Some(gateway.router.clone());
        logging.outcome_started_at = started_at;
        logging.ttft = Some(Arc::new(Mutex::new(TtftMeasurement {
            started: Instant::now(),
            started_at,
            tracker: ttft,
        })));
        while let Some(chunk) = logging.next().await {
            chunk.unwrap();
        }
        assert_eq!(log.lock().unwrap().status_code, 200, "{change}");
        assert!(log.lock().unwrap().ttft_ms >= 100, "{change}");
        let saved = store.get(channel.id).unwrap();
        match change {
            "none" => {
                assert_eq!(saved.model_performance[0].last_ttft_ms, 100);
                assert_eq!(saved.model_performance[0].samples, 1);
            }
            "newer sample" => {
                assert_eq!(saved.model_performance[0].last_ttft_ms, 777);
                assert_eq!(saved.model_performance[0].samples, 1);
            }
            _ => assert!(saved.model_performance.is_empty(), "{change}"),
        }
        if change == "newer health" {
            assert_eq!(saved.model_health[0].status, "error");
            assert_eq!(saved.model_health[0].error, "newer failure");
        }
    }
}

#[tokio::test]
async fn nonstream_logs_preserve_finish_reasons_for_native_and_translated_responses() {
    for (proto, provider, mode, path, reply, expected) in [
        (
            GatewayProtocol::Chat,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
            json!({"choices":[{"message":{"content":"ok"},"finish_reason":"length"}]}),
            "length",
        ),
        (
            GatewayProtocol::Chat,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
            json!({"choices":[{"message":{"content":null},"finish_reason":"content_filter"}]}),
            "content_filter",
        ),
        (
            GatewayProtocol::Chat,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
            json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"call","type":"function","function":{"name":"search","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
            "tool_calls",
        ),
        (
            GatewayProtocol::Anthropic,
            "anthropic",
            "chat_completions",
            "/v1/messages",
            json!({"type":"message","content":[],"stop_reason":"max_tokens"}),
            "max_tokens",
        ),
        (
            GatewayProtocol::Anthropic,
            "anthropic",
            "chat_completions",
            "/v1/messages",
            json!({"type":"message","content":[],"stop_reason":"tool_use"}),
            "tool_use",
        ),
        (
            GatewayProtocol::Responses,
            "openai",
            "responses",
            "/v1/responses",
            json!({"status":"incomplete","output":[],"incomplete_details":{"reason":"max_output_tokens"}}),
            "length",
        ),
        (
            GatewayProtocol::Responses,
            "openai",
            "responses",
            "/v1/responses",
            json!({"status":"incomplete","output":[],"incomplete_details":{"reason":"content_filter"}}),
            "content_filter",
        ),
        (
            GatewayProtocol::Anthropic,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
            json!({"choices":[{"message":{"content":"ok"},"finish_reason":"length"}]}),
            "max_tokens",
        ),
    ] {
        let app = axum::Router::new().route(
            path,
            post(move || {
                let reply = reply.clone();
                async move { Json(reply) }
            }),
        );
        let (endpoint, _server) = serve(app).await;
        let store = Store::open_in_memory().unwrap();
        let channel = seed(&store, "broken", &endpoint, provider, mode);
        let response = gateway(&store)
            .forward(proto, &HeaderMap::new(), request(false), &ordered())
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{proto:?}/{expected}");
        axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let log = store.list_logs(1).unwrap().remove(0);
        assert_eq!(log.status_code, 200);
        assert_eq!(log.finish_reason, expected, "{proto:?}/{provider}");
        assert_eq!(store.list_keys(channel.id).unwrap()[0].successes, 1);
    }
}

#[tokio::test]
async fn a_concurrent_model_failure_does_not_suppress_credential_auth_invalidation() {
    let auth_entered = Arc::new(tokio::sync::Notify::new());
    let release_auth = Arc::new(tokio::sync::Notify::new());
    let entered = auth_entered.clone();
    let release = release_auth.clone();
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<serde_json::Value>| {
            let entered = entered.clone();
            let release = release.clone();
            async move {
                if body["metadata"]["auth"] == true {
                    entered.notify_one();
                    release.notified().await;
                    json_error(StatusCode::UNAUTHORIZED, "invalid credential")
                } else {
                    json_error(StatusCode::NOT_FOUND, "model unavailable")
                }
            }
        }),
    );
    let (endpoint, _server) = serve(app).await;
    let store = Store::open_in_memory().unwrap();
    let channel = seed(&store, "broken", &endpoint, "openai", "chat_completions");
    let gateway = Arc::new(gateway(&store));
    let auth_gateway = gateway.clone();
    let auth_task = tokio::spawn(async move {
        auth_gateway
            .forward(
                GatewayProtocol::Chat,
                &HeaderMap::new(),
                serde_json::to_vec(&json!({"model":"model","metadata":{"auth":true}})).unwrap(),
                &ordered(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), auth_entered.notified())
        .await
        .unwrap();
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            request(false),
            &ordered(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!gateway
        .key_pool
        .retry_after_model(channel.id, "model")
        .is_zero());
    assert!(!store.list_keys(channel.id).unwrap()[0].auth_invalid);
    release_auth.notify_one();
    let response = tokio::time::timeout(Duration::from_secs(2), auth_task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(store.list_keys(channel.id).unwrap()[0].auth_invalid);
}
