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
            let mut response=Response::new(Body::from("data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: {\"error\":{\"message\":\"limited\",\"code\":429}}\n\n"));
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
        .add_keys(
            channel.id,
            &[KeyInput {
                name: "B".into(),
                secret: "key-b".into(),
            }],
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
