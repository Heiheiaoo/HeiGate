use super::*;
use axum::{routing::post, Json};

struct TestServer(tokio::task::JoinHandle<()>);

impl Drop for TestServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn live_ttft_counts_only_successful_streams_on_the_actual_model() {
    for (proto, provider, mode, path) in [
        (
            GatewayProtocol::Chat,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
        ),
        (
            GatewayProtocol::Responses,
            "openai",
            "responses",
            "/v1/responses",
        ),
        (
            GatewayProtocol::Anthropic,
            "anthropic",
            "chat_completions",
            "/v1/messages",
        ),
        (
            GatewayProtocol::Anthropic,
            "openai",
            "chat_completions",
            "/v1/chat/completions",
        ),
    ] {
        let app = axum::Router::new().route(path, post(move |Json(body): Json<serde_json::Value>| async move {
            assert_eq!(body["model"], "actual-model");
            let stream_requested = body["stream"].as_bool().unwrap_or(false);
            if !stream_requested {
                let response = match path {
                    "/v1/messages" => json!({"type":"message","content":[]}),
                    "/v1/responses" => json!({"status":"completed","output":[]}),
                    _ => json!({"choices":[{"message":{"role":"assistant","content":""},"finish_reason":"stop"}]}),
                };
                return Response::new(Body::from(response.to_string()));
            }
            let mode = body["metadata"]["mode"].as_str().or_else(|| body["messages"][0]["content"].as_str()).unwrap_or("ok");
            let fail = mode == "fail";
            let cancel = mode == "cancel";
            let empty = mode == "empty";
            let (role, text, done) = match path {
                "/v1/messages" => (
                    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"pong\"}}\n\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                ),
                "/v1/responses" => (
                    "data: {\"type\":\"response.created\"}\n\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"pong\"}\n\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}}\n\n",
                ),
                _ => (
                    "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"pong\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                ),
            };
            let chunks = stream::once(async move { Ok::<_, std::io::Error>(Bytes::from_static(role.as_bytes())) })
                .chain(stream::once(async move {
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    Ok::<_, std::io::Error>(Bytes::from_static(if empty { b"" } else { text.as_bytes() }))
                }))
                .chain(stream::once(async move {
                    tokio::time::sleep(Duration::from_millis(if cancel { 5000 } else { 10 })).await;
                    if fail { Err(std::io::Error::other("stream cut")) }
                    else { Ok(Bytes::from_static(done.as_bytes())) }
                }));
            Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(chunks)).unwrap()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = TestServer(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        let store = Store::open_in_memory().unwrap();
        let mut channel = Channel {
            name: "source".into(),
            provider: provider.into(),
            api_mode: mode.into(),
            endpoint: format!("http://{address}"),
            api_key: "test".into(),
            primary_model: "primary".into(),
            extra_models: vec!["actual-model".into()],
            enabled: true,
            ..Default::default()
        };
        store.save(&mut channel).unwrap();
        store
            .set_model_health(
                channel.id,
                &crate::store::ModelHealth {
                    model: "actual-model".into(),
                    status: "operational".into(),
                    ttft_ms: Some(999),
                    ..Default::default()
                },
            )
            .unwrap();
        let gateway = Gateway::new(store.clone(), Arc::new(Router::new(store.clone())));
        for (mode, streaming, expected_samples) in [
            ("ok", true, 1),
            ("fail", true, 1),
            ("cancel", true, 1),
            ("ok", false, 1),
            ("empty", true, 1),
            ("ok", true, 2),
        ] {
            // Each scenario is independent of the failed stream's model cooldown.
            store
                .lock()
                .execute("UPDATE channel_key_models SET unavailable_until=''", [])
                .unwrap();
            let response = gateway.forward(proto, &HeaderMap::new(), serde_json::to_vec(&json!({
                "model":"actual-model", "stream":streaming, "max_tokens":16,
                "messages":[{"role":"user","content":mode}], "input":"ping", "metadata":{"mode":mode}
            })).unwrap(), &RequestContext::default()).await;
            if mode == "cancel" {
                drop(response);
            } else {
                let _ = axum::body::to_bytes(response.into_body(), 1 << 20).await;
            }
            let saved = store.get(channel.id).unwrap();
            assert_eq!(
                saved.model_performance.len(),
                1,
                "{proto:?}, {provider}, {mode}"
            );
            let performance = &saved.model_performance[0];
            assert_eq!(performance.model, "actual-model");
            assert_eq!(
                performance.samples, expected_samples,
                "{proto:?}, {provider}, {mode}"
            );
            assert!(
                performance.ttft_ms >= 35.0,
                "role/start must not count: {}",
                performance.ttft_ms
            );
            assert_eq!(
                saved
                    .model_health
                    .iter()
                    .find(|h| h.model == "actual-model")
                    .unwrap()
                    .ttft_ms,
                Some(999)
            );
        }
    }
}

#[tokio::test]
async fn live_ttft_excludes_previous_failed_attempt_time() {
    let app = axum::Router::new()
        .route("/v1/chat/completions", post(|| async {
            tokio::time::sleep(Duration::from_millis(120)).await;
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "failure")
        }))
        .route("/backup/v1/chat/completions", post(|| async {
            let chunks = stream::once(async {
                tokio::time::sleep(Duration::from_millis(25)).await;
                Ok::<_, std::io::Error>(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"pong\"}}]}\n\ndata: [DONE]\n\n"))
            });
            Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(chunks)).unwrap()
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let _server = TestServer(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    let store = Store::open_in_memory().unwrap();
    let mut first = Channel {
        name: "first".into(),
        provider: "openai".into(),
        endpoint: format!("http://{address}"),
        api_key: "test".into(),
        primary_model: "model".into(),
        enabled: true,
        ..Default::default()
    };
    store.save(&mut first).unwrap();
    let mut backup = first.clone();
    backup.id = 0;
    backup.name = "backup".into();
    backup.endpoint.push_str("/backup");
    store.save(&mut backup).unwrap();
    let gateway = Gateway::new(store.clone(), Arc::new(Router::new(store.clone())));
    let response = gateway
        .forward(
            GatewayProtocol::Chat,
            &HeaderMap::new(),
            serde_json::to_vec(&json!({"model":"model","stream":true})).unwrap(),
            &RequestContext {
                strategy: crate::router::ROUTE_MANUAL_PRIORITY.into(),
                allowed_channels: Some(vec!["first".into(), "backup".into()]),
                ..Default::default()
            },
        )
        .await;
    axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(store.get(first.id).unwrap().model_performance.is_empty());
    let saved = store.get(backup.id).unwrap();
    let performance = &saved.model_performance[0];
    let log = &store.list_logs(1).unwrap()[0];
    assert!(log.ttft_ms - performance.last_ttft_ms >= 120);
    assert!(
        log.latency_ms - performance.last_ttft_ms >= 120,
        "failed attempts leaked into TTFT: {log:?}"
    );
}

#[tokio::test]
async fn invalid_json_at_clean_eof_fails_over_and_never_counts_success() {
    for body in [
        "{\"choices\":[",
        "<html>bad gateway</html>",
        "null",
        "{} trailing",
    ] {
        for with_backup in [false, true] {
            let backup_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let calls = backup_calls.clone();
            let app = axum::Router::new()
                .route("/v1/chat/completions", post(move || async move {
                    Response::builder().header("content-type", "application/json").body(Body::from(body)).unwrap()
                }))
                .route("/healthy/v1/chat/completions", post(move || {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async { Json(json!({"choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}})) }
                }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let _server = TestServer(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }));
            let store = Store::open_in_memory().unwrap();
            let mut broken = Channel {
                name: "Broken".into(),
                provider: "openai".into(),
                api_mode: "chat_completions".into(),
                endpoint: format!("http://{address}"),
                api_key: "test".into(),
                primary_model: "model".into(),
                enabled: true,
                ..Default::default()
            };
            store.save(&mut broken).unwrap();
            let mut backup = broken.clone();
            backup.id = 0;
            backup.name = "Backup".into();
            backup.endpoint = format!("http://{address}/healthy");
            if with_backup {
                store.save(&mut backup).unwrap();
            }
            let gateway = Gateway::new(store.clone(), Arc::new(Router::new(store.clone())));
            let response = gateway
                .forward(
                    GatewayProtocol::Chat,
                    &HeaderMap::new(),
                    serde_json::to_vec(
                        &json!({"model":"model","messages":[{"role":"user","content":"hi"}]}),
                    )
                    .unwrap(),
                    &RequestContext {
                        strategy: crate::router::ROUTE_MANUAL_PRIORITY.into(),
                        allowed_channels: Some(vec!["Broken".into(), "Backup".into()]),
                        ..Default::default()
                    },
                )
                .await;
            assert_eq!(
                response.status(),
                if with_backup {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_GATEWAY
                }
            );
            axum::body::to_bytes(response.into_body(), 10000)
                .await
                .unwrap();
            assert_eq!(store.list_keys(broken.id).unwrap()[0].successes, 0);
            let log = &store.list_logs(10).unwrap()[0];
            let trace: Vec<TraceStep> = serde_json::from_str(&log.trace_json).unwrap();
            assert!(!trace[0].succeeded);
            assert!(trace[0].error.contains("invalid upstream JSON response"));
            assert_eq!(
                backup_calls.load(std::sync::atomic::Ordering::SeqCst),
                usize::from(with_backup)
            );
            if with_backup {
                assert!(log.is_failover);
                assert_eq!(store.list_keys(backup.id).unwrap()[0].successes, 1);
            } else {
                assert_eq!(log.status_code, 502);
            }
        }
    }
}

#[tokio::test]
async fn translated_in_band_errors_preserve_classification_before_and_after_output() {
    for status in [429, 401, 404] {
        for after_output in [false, true] {
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let captured = calls.clone();
            let app = axum::Router::new().route("/v1/chat/completions", post(move || {
                captured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    let first = if after_output { "data: {\"id\":\"chat-1\",\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n" } else { "" };
                    let chunks = stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(first)) })
                        .chain(stream::once(async move {
                            tokio::time::sleep(Duration::from_millis(25)).await;
                            Ok::<_, std::io::Error>(Bytes::from(format!("data: {}\n\n", json!({"error":{"code":status,"message":"upstream unavailable"}}))))
                        }));
                    Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(chunks)).unwrap()
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let _server = TestServer(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }));
            let store = Store::open_in_memory().unwrap();
            let mut channel = Channel {
                name: "Supplier".into(),
                provider: "openai".into(),
                api_mode: "chat_completions".into(),
                endpoint: format!("http://{address}"),
                api_key: "test".into(),
                primary_model: "model".into(),
                enabled: true,
                ..Default::default()
            };
            store.save(&mut channel).unwrap();
            let gateway = Gateway::new(store.clone(), Arc::new(Router::new(store.clone())));
            let request = serde_json::to_vec(&json!({"model":"model","max_tokens":20,"messages":[{"role":"user","content":"hi"}],"stream":true})).unwrap();
            let response = gateway
                .forward(
                    GatewayProtocol::Anthropic,
                    &HeaderMap::new(),
                    request.clone(),
                    &RequestContext::default(),
                )
                .await;
            assert_eq!(
                response.status().as_u16(),
                if after_output { 200 } else { status }
            );
            let chunks = response
                .into_body()
                .into_data_stream()
                .collect::<Vec<_>>()
                .await;
            if after_output {
                assert!(chunks.iter().any(Result::is_err));
                let wire = chunks
                    .into_iter()
                    .filter_map(Result::ok)
                    .map(|b| String::from_utf8(b.to_vec()).unwrap())
                    .collect::<String>();
                assert!(wire.contains("hello"));
                assert!(!wire.contains("message_stop"));
                let error = wire
                    .lines()
                    .filter_map(|l| l.strip_prefix("data: "))
                    .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
                    .find(|v| v["type"] == "error")
                    .unwrap();
                assert_eq!(error["error"]["code"], status);
            }
            let key = &store.list_keys(channel.id).unwrap()[0];
            assert_eq!(key.successes, 0);
            if status == 401 {
                assert!(key.auth_invalid);
            } else {
                assert!(!gateway
                    .key_pool
                    .retry_after_model(channel.id, "model")
                    .is_zero());
            }
            if status == 429 {
                assert_eq!(key.rate_limits, 1);
            }
            let next = gateway
                .forward(
                    GatewayProtocol::Anthropic,
                    &HeaderMap::new(),
                    request,
                    &RequestContext::default(),
                )
                .await;
            assert_ne!(next.status(), StatusCode::OK);
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn native_anthropic_zero_usage_is_preserved_in_logs_and_key_totals() {
    let app = axum::Router::new().route("/v1/messages", post(|| async {
        Response::builder().header("content-type", "text/event-stream").body(Body::from(
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":0,\"output_tokens\":1}}}\n\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":20}}\n\ndata: {\"type\":\"message_stop\"}\n\n"
        )).unwrap()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let _server = TestServer(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let store = Store::open_in_memory().unwrap();
    let mut channel = Channel {
        name: "Native".into(),
        provider: "anthropic".into(),
        api_mode: "messages".into(),
        endpoint: format!("http://{address}"),
        api_key: "test".into(),
        primary_model: "model".into(),
        enabled: true,
        ..Default::default()
    };
    store.save(&mut channel).unwrap();
    let gateway = Gateway::new(store.clone(), Arc::new(Router::new(store.clone())));
    let response = gateway.forward(GatewayProtocol::Anthropic, &HeaderMap::new(), serde_json::to_vec(&json!({"model":"model","max_tokens":20,"messages":[{"role":"user","content":"nonempty prompt"}],"stream":true})).unwrap(), &RequestContext::default()).await;
    assert_eq!(response.status(), StatusCode::OK);
    axum::body::to_bytes(response.into_body(), 10000)
        .await
        .unwrap();
    let log = &store.list_logs(10).unwrap()[0];
    assert_eq!(
        (log.prompt_tokens, log.completion_tokens, log.total_tokens),
        (0, 20, 20)
    );
    let key = &store.list_keys(channel.id).unwrap()[0];
    assert_eq!(
        (key.successes, key.prompt_tokens, key.completion_tokens),
        (1, 0, 20)
    );
}

#[tokio::test]
async fn translated_usage_and_responses_limits_are_logged_without_losing_tokens() {
    for case in [
        "translated",
        "translated_zero",
        "translated_no_usage",
        "limited",
        "limited_empty",
        "filtered",
        "truncated",
        "failed",
    ] {
        let translated = case.starts_with("translated");
        let frame = if translated {
            let mut frame = format!(
                "data: {}\n\ndata: {}\n\n",
                json!({"id":"chat-1","choices":[{"delta":{"content":"hello"}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
            );
            if case != "translated_no_usage" {
                let (input, output) = if case == "translated_zero" {
                    (0, 0)
                } else {
                    (100, 20)
                };
                frame.push_str(&format!("data: {}\n\n", json!({"choices":[],"usage":{"prompt_tokens":input,"completion_tokens":output,"total_tokens":input+output}})));
            }
            frame.push_str("data: [DONE]\n\n");
            frame
        } else {
            let mut frame = if case == "limited_empty" {
                String::new()
            } else {
                format!(
                    "data: {}\n\n",
                    json!({"type":"response.output_text.delta","delta":"hello"})
                )
            };
            if case != "truncated" {
                let response = if case == "failed" {
                    json!({"status":"failed","error":{"code":"server_error","message":"failed"}})
                } else {
                    json!({"status":"incomplete","error":null,"incomplete_details":{"reason":if case == "filtered" { "content_filter" } else { "max_output_tokens" }},"usage":{"input_tokens":100,"output_tokens":20,"total_tokens":120}})
                };
                frame.push_str(&format!("data: {}\n\n", json!({"type":if case == "failed" {"response.failed"} else {"response.incomplete"},"response":response})));
            }
            frame
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let path = if translated {
            "/v1/chat/completions"
        } else {
            "/v1/responses"
        };
        let app = axum::Router::new().route(
            path,
            post(move || {
                let frame = frame.clone();
                async move {
                    let split = frame.find("\n\n").unwrap() + 2;
                    let first = Bytes::copy_from_slice(&frame.as_bytes()[..split]);
                    let tail = Bytes::copy_from_slice(&frame.as_bytes()[split..]);
                    let chunks = stream::once(async move { Ok::<_, std::io::Error>(first) }).chain(
                        stream::once(async move {
                            tokio::time::sleep(Duration::from_millis(25)).await;
                            Ok::<_, std::io::Error>(tail)
                        }),
                    );
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(chunks))
                        .unwrap()
                }
            }),
        );
        let _server = TestServer(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        let store = Store::open_in_memory().unwrap();
        let mut channel = Channel {
            name: "Supplier".into(),
            provider: "openai".into(),
            api_mode: if translated {
                "chat_completions"
            } else {
                "responses"
            }
            .into(),
            endpoint: format!("http://{address}"),
            api_key: "test".into(),
            primary_model: "model".into(),
            enabled: true,
            ..Default::default()
        };
        store.save(&mut channel).unwrap();
        let gateway = Gateway::new(store.clone(), Arc::new(Router::new(store.clone())));
        let response = gateway.forward(
            if translated { GatewayProtocol::Anthropic } else { GatewayProtocol::Responses },
            &HeaderMap::new(),
            serde_json::to_vec(&json!({"model":"model","max_tokens":20,"messages":[{"role":"user","content":"hi"}],"input":"hi","stream":true})).unwrap(),
            &RequestContext::default(),
        ).await;
        assert_eq!(response.status(), StatusCode::OK, "{case}");
        let body = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let log = &store.list_logs(10).unwrap()[0];
        let key = &store.list_keys(channel.id).unwrap()[0];
        let failed = matches!(case, "failed" | "truncated");
        assert_eq!(log.status_code, if failed { 502 } else { 200 }, "{case}");
        assert_eq!(key.successes, if failed { 0 } else { 1 }, "{case}");
        if !failed && case != "translated_no_usage" {
            let expected = if case == "translated_zero" {
                (0, 0, 0)
            } else {
                (100, 20, 120)
            };
            assert_eq!(
                (log.prompt_tokens, log.completion_tokens, log.total_tokens),
                expected,
                "{case}"
            );
            assert_eq!(
                (key.prompt_tokens, key.completion_tokens),
                (expected.0, expected.1),
                "{case}"
            );
            if translated {
                let body = String::from_utf8(body.to_vec()).unwrap();
                let final_event: serde_json::Value = body
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                    .find(|event| event["type"] == "message_delta")
                    .unwrap();
                assert_eq!(final_event["usage"]["input_tokens"], expected.0);
                assert_eq!(final_event["usage"]["output_tokens"], expected.1);
            }
        }
        if case == "translated_no_usage" {
            assert!(log.prompt_tokens > 0);
            assert!(log.completion_tokens > 0);
            assert_eq!(log.total_tokens, log.prompt_tokens + log.completion_tokens);
        }
        if matches!(case, "limited" | "limited_empty") {
            assert_eq!(log.finish_reason, "length");
            assert!(log.error_message.is_empty());
        }
        if case == "filtered" {
            assert_eq!(log.finish_reason, "content_filter");
        }
    }
}

#[tokio::test]
async fn non_streaming_body_is_buffered_and_read_errors_are_preserved() {
    for prefix_size in [16, 8192] {
        let prefix = Bytes::from(vec![b'x'; prefix_size]);
        let source: ByteStream = Box::pin(stream::iter(vec![
            Ok(prefix.clone()),
            Err(std::io::Error::other("interrupted body")),
        ]));
        let (read, _, error) = read_initial_prefix(source, false).await;
        assert_eq!(read, prefix);
        assert_eq!(error.unwrap().to_string(), "interrupted body");
    }
    let source: ByteStream = Box::pin(stream::iter(vec![
        Ok(Bytes::from(vec![b'x'; 8192])),
        Ok(Bytes::from_static(b"tail")),
    ]));
    let (read, mut rest, error) = read_initial_prefix(source, false).await;
    assert!(error.is_none());
    assert_eq!(read.len(), 8196);
    assert!(read.ends_with(b"tail"));
    assert!(rest.next().await.is_none());

    let source: ByteStream = Box::pin(stream::iter(vec![
        Ok(Bytes::from(vec![b'x'; MAX_REQUEST_BODY])),
        Ok(Bytes::from_static(b"x")),
    ]));
    let (read, _, error) = read_initial_prefix(source, false).await;
    assert_eq!(read.len(), MAX_REQUEST_BODY);
    assert_eq!(
        error.unwrap().to_string(),
        "upstream response body too large"
    );
}

#[tokio::test]
async fn interrupted_json_fails_over_without_counting_success() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for prefix_size in [16, 8192] {
        for with_backup in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let _broken_server = TestServer(tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = socket.read(&mut buffer).await.unwrap();
                        if n == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..n]);
                    }
                    let prefix = format!("{{\"content\":\"{}", "x".repeat(prefix_size));
                    let headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        prefix.len() + 100
                    );
                    socket.write_all(headers.as_bytes()).await.unwrap();
                    socket.write_all(prefix.as_bytes()).await.unwrap();
                    socket.shutdown().await.unwrap();
                }
            }));
            let store = Store::open_in_memory().unwrap();
            let mut broken = Channel {
                name: "Broken".into(),
                provider: "openai".into(),
                api_mode: "chat_completions".into(),
                endpoint: format!("http://{address}"),
                api_key: "test-broken".into(),
                primary_model: "model".into(),
                enabled: true,
                ..Default::default()
            };
            store.save(&mut broken).unwrap();
            let backup_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let backup_address = backup_listener.local_addr().unwrap();
            let backup_app = axum::Router::new().route("/v1/chat/completions", post(|| async {
                Json(json!({"choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}))
            }));
            let _backup_server = TestServer(tokio::spawn(async move {
                axum::serve(backup_listener, backup_app).await.unwrap();
            }));
            let mut backup = broken.clone();
            backup.id = 0;
            backup.name = "Backup".into();
            backup.endpoint = format!("http://{backup_address}");
            backup.api_key = "test-backup".into();
            if with_backup {
                store.save(&mut backup).unwrap();
            }
            let gateway = Gateway::new(store.clone(), Arc::new(Router::new(store.clone())));
            let response = gateway
                .forward(
                    GatewayProtocol::Chat,
                    &HeaderMap::new(),
                    serde_json::to_vec(
                        &json!({"model":"model","messages":[{"role":"user","content":"hi"}]}),
                    )
                    .unwrap(),
                    &RequestContext {
                        strategy: crate::router::ROUTE_MANUAL_PRIORITY.into(),
                        allowed_channels: Some(vec!["Broken".into(), "Backup".into()]),
                        ..Default::default()
                    },
                )
                .await;
            assert_eq!(
                response.status(),
                if with_backup {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_GATEWAY
                }
            );
            let body = axum::body::to_bytes(response.into_body(), 10000)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let broken_key = &store.list_keys(broken.id).unwrap()[0];
            assert_eq!(broken_key.attempts, 1);
            assert_eq!(broken_key.successes, 0);
            let log = &store.list_logs(10).unwrap()[0];
            let trace: Vec<TraceStep> = serde_json::from_str(&log.trace_json).unwrap();
            assert!(!trace[0].succeeded);
            assert!(!trace[0].error.is_empty());
            if with_backup {
                assert_eq!(body["choices"][0]["message"]["content"], "ok");
                assert!(log.is_failover);
                assert_eq!(log.channel_name, "Backup");
                assert_eq!(store.list_keys(backup.id).unwrap()[0].successes, 1);
                assert_eq!(trace.len(), 2);
            } else {
                assert_eq!(log.status_code, 502);
                assert_eq!(trace.len(), 1);
            }
        }
    }
}

#[tokio::test]
async fn anthropic_requests_do_not_call_or_cool_down_responses_channels() {
    for with_chat in [false, true] {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let responses_seen = seen.clone();
        let chat_seen = seen.clone();
        let app = axum::Router::new()
            .route("/v1/responses", post(move || {
                let seen = responses_seen.clone();
                async move {
                    seen.lock().unwrap().push("responses".into());
                    Json(json!({"id":"resp-1","status":"completed","output":[],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}))
                }
            }))
            .route("/v1/chat/completions", post(move || {
                let seen = chat_seen.clone();
                async move {
                    seen.lock().unwrap().push("chat".into());
                    Json(json!({"id":"chat-1","model":"model","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}))
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = TestServer(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        let store = Store::open_in_memory().unwrap();
        let mut responses = Channel {
            name: "Responses".into(),
            provider: "openai".into(),
            api_mode: "responses".into(),
            endpoint: format!("http://{address}"),
            api_key: "test-responses".into(),
            primary_model: "model".into(),
            enabled: true,
            ..Default::default()
        };
        store.save(&mut responses).unwrap();
        let mut chat = responses.clone();
        chat.id = 0;
        chat.name = "Chat".into();
        chat.api_mode = "chat_completions".into();
        if with_chat {
            store.save(&mut chat).unwrap();
        }
        let gateway = Gateway::new(store.clone(), Arc::new(Router::new(store.clone())));
        let ctx = RequestContext {
            strategy: crate::router::ROUTE_MANUAL_PRIORITY.into(),
            allowed_channels: Some(vec!["Responses".into(), "Chat".into()]),
            ..Default::default()
        };
        let response = gateway.forward(
            GatewayProtocol::Anthropic,
            &HeaderMap::new(),
            serde_json::to_vec(&json!({"model":"model","max_tokens":20,"messages":[{"role":"user","content":"hi"}]})).unwrap(),
            &ctx,
        ).await;
        assert_eq!(
            response.status(),
            if with_chat {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            }
        );
        axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            if with_chat {
                vec!["chat".to_string()]
            } else {
                vec![]
            }
        );
        let key = &store.list_keys(responses.id).unwrap()[0];
        assert_eq!(key.attempts, 0);
        assert!(gateway
            .key_pool
            .retry_after_model(responses.id, "model")
            .is_zero());
        let response = gateway
            .forward(
                GatewayProtocol::Responses,
                &HeaderMap::new(),
                serde_json::to_vec(&json!({"model":"model","input":"hi"})).unwrap(),
                &ctx,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().last().unwrap(), "responses");
        assert_eq!(store.list_keys(responses.id).unwrap()[0].successes, 1);
    }
}

#[tokio::test]
async fn translated_stream_preserves_roleless_deltas_and_split_utf8() {
    let frames = [
        json!({"id":"chat-1","choices":[{"delta":{"role":"assistant"}}]}),
        json!({"choices":[{"delta":{"content":"你好🌍"}}]}),
        json!({"choices":[{"delta":{"tool_calls":[
            {"index":0,"id":"a","function":{"name":"lookup","arguments":"{\"q\":"}},
            {"index":1,"id":"b","function":{"name":"search","arguments":"{\"q\":"}}
        ]}}]}),
        json!({"choices":[{"delta":{"tool_calls":[
            {"index":1,"function":{"arguments":"\"上海\"}"}},
            {"index":0,"function":{"arguments":"\"北京\"}"}}
        ]}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}),
    ];
    let mut wire = frames
        .iter()
        .map(|v| format!("data: {v}\n\n"))
        .collect::<String>();
    wire.push_str("data: [DONE]\n\n");
    // Every byte is a separate transport chunk, including multibyte characters.
    let chunks = wire
        .bytes()
        .map(|b| Ok::<_, reqwest::Error>(Bytes::from(vec![b])))
        .collect::<Vec<_>>();
    let translated = translated_stream(Box::pin(stream::iter(chunks)), "model".into());
    let output = translated
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
        .collect::<String>();
    assert!(output.contains("你好🌍"));
    assert!(!output.contains('�'));
    let mut open = None;
    let mut tools = std::collections::BTreeMap::new();
    let mut arguments = std::collections::BTreeMap::<i64, String>::new();
    let mut stops = 0;
    for line in output.lines().filter_map(|l| l.strip_prefix("data: ")) {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        let index = event["index"].as_i64();
        match event["type"].as_str().unwrap() {
            "content_block_start" => {
                assert!(open.is_none(), "blocks must not overlap");
                open = index;
                if event["content_block"]["type"] == "tool_use" {
                    tools.insert(
                        index.unwrap(),
                        event["content_block"]["id"].as_str().unwrap().to_string(),
                    );
                }
            }
            "content_block_delta" => {
                assert_eq!(open, index, "delta must target the open block");
                if let Some(args) = event["delta"]["partial_json"].as_str() {
                    arguments.entry(index.unwrap()).or_default().push_str(args);
                }
            }
            "content_block_stop" => {
                assert_eq!(open.take(), index);
            }
            "message_delta" => {
                assert!(open.is_none());
                assert_eq!(event["delta"]["stop_reason"], "tool_use");
                assert_eq!(event["usage"]["output_tokens"], 20);
            }
            "message_stop" => stops += 1,
            _ => {}
        }
    }
    assert_eq!(stops, 1);
    assert_eq!(tools.len(), 2);
    for (index, id) in tools {
        let parsed: serde_json::Value = serde_json::from_str(&arguments[&index]).unwrap();
        assert_eq!(parsed["q"], if id == "a" { "北京" } else { "上海" });
    }
}

#[tokio::test]
async fn translated_stream_does_not_complete_or_emit_buffered_tools_after_error() {
    let raw = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\"}}]}}]}\n\ndata: {\"error\":{\"message\":\"failed\"}}\n\n";
    let result = translated_stream(
        Box::pin(stream::iter(vec![Ok(Bytes::from(raw))])),
        "model".into(),
    )
    .collect::<Vec<_>>()
    .await;
    assert!(result.iter().any(Result::is_err));
    let output = result
        .into_iter()
        .filter_map(Result::ok)
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
        .collect::<String>();
    assert!(!output.contains("content_block_start"));
    assert!(!output.contains("message_stop"));
}

#[tokio::test]
async fn translated_stream_emits_complete_events_before_same_chunk_size_error() {
    let mut raw =
        b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\n"
            .to_vec();
    raw.extend(vec![b'x'; crate::protocol::stream::MAX_SSE_LINE_BYTES + 1]);

    let result = translated_stream(
        Box::pin(stream::iter(vec![Ok(Bytes::from(raw))])),
        "model".into(),
    )
    .collect::<Vec<_>>()
    .await;

    assert!(result.last().is_some_and(Result::is_err));
    let output = result
        .into_iter()
        .filter_map(Result::ok)
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
        .collect::<String>();
    assert!(output.contains("hello"));
    assert!(output.contains("message_start"));
    assert!(!output.contains("message_delta"));
    assert!(!output.contains("message_stop"));
}

#[tokio::test]
async fn chat_and_responses_request_options_and_usage_follow_their_protocols() {
    for proto in [GatewayProtocol::Chat, GatewayProtocol::Responses] {
        let seen = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let capture = seen.clone();
        let path = if proto == GatewayProtocol::Responses {
            "/v1/responses"
        } else {
            "/v1/chat/completions"
        };
        let app = axum::Router::new().route(path, post(move |Json(body): Json<serde_json::Value>| {
            let capture = capture.clone();
            async move {
                capture.lock().unwrap().push(body);
                let frame = if proto == GatewayProtocol::Responses {
                    format!("data: {}\n\ndata: {}\n\n",
                        json!({"type":"response.output_text.delta","delta":"hello"}),
                        json!({"type":"response.completed","response":{"usage":{"input_tokens":100,"output_tokens":20,"total_tokens":120}}}))
                } else {
                    format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":{"content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}}))
                };
                Response::builder().header("content-type", "text/event-stream").body(Body::from(frame)).unwrap()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Store::open_in_memory().unwrap();
        let mut channel = Channel {
            name: "Supplier".into(),
            provider: "openai".into(),
            api_mode: if proto == GatewayProtocol::Responses {
                "responses"
            } else {
                "chat_completions"
            }
            .into(),
            endpoint: format!("http://{address}"),
            api_key: "test".into(),
            primary_model: "model".into(),
            enabled: true,
            ..Default::default()
        };
        store.save(&mut channel).unwrap();
        let gateway = Gateway::new(store.clone(), Arc::new(Router::new(store.clone())));
        let request = json!({"model":"model","input":"hi","messages":[{"role":"user","content":"hi"}],"stream":true});
        let response = gateway
            .forward(
                proto,
                &HeaderMap::new(),
                serde_json::to_vec(&request).unwrap(),
                &RequestContext::default(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let sent = seen.lock().unwrap();
        assert_eq!(sent.len(), 1);
        if proto == GatewayProtocol::Responses {
            assert!(sent[0].get("stream_options").is_none());
        } else {
            assert_eq!(sent[0]["stream_options"]["include_usage"], true);
        }
        let log = &store.list_logs(10).unwrap()[0];
        assert_eq!(
            (log.prompt_tokens, log.completion_tokens, log.total_tokens),
            (100, 20, 120)
        );
        assert_eq!(log.status_code, 200);
        let key = &store.list_keys(channel.id).unwrap()[0];
        assert_eq!((key.prompt_tokens, key.completion_tokens), (100, 20));
        server.abort();
    }
}
