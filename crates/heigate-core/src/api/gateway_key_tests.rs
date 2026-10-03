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
        calls.lock().unwrap().push((key.clone(),body));
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
    assert!(gateway.store.list_keys(id).unwrap()[0]
        .cooldown_until
        .is_some());
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
    assert!(gateway.store.list_keys(other.id).unwrap()[0]
        .cooldown_until
        .is_some());
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
    assert!(keys[0].cooldown_until.unwrap() > Utc::now() + chrono::Duration::seconds(290));
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
    assert_eq!(gateway.store.list_keys(id).unwrap()[0].rate_limit_count, 1);
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
