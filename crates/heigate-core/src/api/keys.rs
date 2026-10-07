use super::handlers::SharedState;
use crate::{
    store::StoreError,
    store_keys::{KeyInput, KeyPatch},
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn error(err: StoreError) -> Response {
    let (status, message) = match err {
        StoreError::NotFound(_) => (StatusCode::NOT_FOUND, "渠道或 Key 不存在".to_string()),
        StoreError::Invalid(message) => (StatusCode::BAD_REQUEST, message),
        _ => (
            StatusCode::BAD_REQUEST,
            "操作失败，请检查重复 Key 或配置后重试".to_string(),
        ),
    };
    (status, Json(json!({"error":message}))).into_response()
}

pub async fn list(State(state): State<SharedState>, Path(channel_id): Path<i64>) -> Response {
    if let Err(err) = state.store.get(channel_id) {
        return error(err);
    }
    match state.store.list_keys(channel_id) {
        Ok(keys) => {
            let data: Vec<_> = keys
                .iter()
                .map(|key| {
                    let mut value = serde_json::to_value(key).expect("serializable key");
                    value["masked_secret"] = json!(if key.secret.is_empty() {
                        "无需认证".to_string()
                    } else {
                        format!(
                            "••••{}",
                            key.secret
                                .chars()
                                .rev()
                                .take(4)
                                .collect::<String>()
                                .chars()
                                .rev()
                                .collect::<String>()
                        )
                    });
                    value
                })
                .collect();
            let mut response = Json(json!({"data":data})).into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store, private"),
            );
            response
        }
        Err(err) => error(err),
    }
}

/// Reveals one key only when the user explicitly requests it from the local UI.
/// The regular list endpoint intentionally remains redacted.
pub async fn reveal(
    State(state): State<SharedState>,
    Path((channel_id, key_id)): Path<(i64, i64)>,
) -> Response {
    if let Err(err) = state.store.get(channel_id) {
        return error(err);
    }
    match state
        .store
        .list_keys(channel_id)
        .ok()
        .and_then(|keys| keys.into_iter().find(|key| key.id == key_id))
    {
        Some(key) => {
            let mut response = Json(json!({"data": {"secret": key.secret}})).into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store, private"),
            );
            response
        }
        None => error(StoreError::NotFound(key_id)),
    }
}

pub async fn add(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<KeyInput>,
) -> Response {
    match state.store.add_key(id, &body) {
        Ok(()) => list(State(state), Path(id)).await,
        Err(err) => error(err),
    }
}

pub async fn update(
    State(state): State<SharedState>,
    Path((channel, id)): Path<(i64, i64)>,
    Json(body): Json<KeyPatch>,
) -> Response {
    match state.store.patch_key(channel, id, &body) {
        Ok(()) => list(State(state), Path(channel)).await,
        Err(err) => error(err),
    }
}

pub async fn delete(
    State(state): State<SharedState>,
    Path((channel, id)): Path<(i64, i64)>,
) -> Response {
    match state.store.delete_key(channel, id) {
        Ok(()) => list(State(state), Path(channel)).await,
        Err(err) => error(err),
    }
}

pub async fn probe(
    State(state): State<SharedState>,
    Path((channel, id)): Path<(i64, i64)>,
) -> Response {
    match state.runner.probe_key_now(channel, id).await {
        Ok(results) => Json(json!({"data":results})).into_response(),
        Err(_) => (
            StatusCode::CONFLICT,
            Json(json!({"error":"Key 不可用于测试：禁用、权限限制或已有测试运行中"})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::gateway::Gateway,
        api::handlers::AppState,
        router::Router as ChannelRouter,
        runner::ProbeRunner,
        store::{Channel, Store},
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::post,
        Router,
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn key_add_app() -> (Router, Arc<Store>, i64) {
        let store = Store::open_in_memory().unwrap();
        let mut channel = Channel {
            name: "test".into(),
            api_key: "existing-key".into(),
            primary_model: "model".into(),
            ..Default::default()
        };
        store.save(&mut channel).unwrap();
        let router = Arc::new(ChannelRouter::new(store.clone()));
        let state = Arc::new(AppState {
            runner: Arc::new(ProbeRunner::new(store.clone())),
            gateway: Arc::new(Gateway::new(store.clone(), router.clone())),
            router,
            store: store.clone(),
            gateway_key: "test".into(),
            gateway_port: 28888,
            version: "test".into(),
        });
        let app = Router::new()
            .route("/channels/{id}/keys", post(add))
            .with_state(state);
        (app, store, channel.id)
    }

    async fn post_key(app: Router, id: i64, body: &str) -> StatusCode {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/channels/{id}/keys"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    #[tokio::test]
    async fn add_key_http_contract_accepts_one_key_and_rejects_duplicates_or_invalid_values() {
        let (app, store, id) = key_add_app().await;
        assert_eq!(
            post_key(app.clone(), id, r#"{"name":"Backup","secret":"new-key"}"#).await,
            StatusCode::OK
        );
        assert_eq!(
            post_key(
                app.clone(),
                id,
                r#"{"name":"Duplicate","secret":"new-key"}"#
            )
            .await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_key(app.clone(), id, r#"{"name":"Invalid","secret":"***"}"#).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(store.list_keys(id).unwrap().len(), 2);
        assert_eq!(
            post_key(app, id, r#"{"keys":[{"name":"Legacy","secret":"key"}]}"#).await,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
