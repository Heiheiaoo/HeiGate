use super::handlers::SharedState;
use crate::{
    store::StoreError,
    store_keys::{KeyInput, KeyPatch},
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
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
            Json(json!({"data":data})).into_response()
        }
        Err(err) => error(err),
    }
}

#[derive(Deserialize)]
pub struct AddKeys {
    pub keys: Vec<KeyInput>,
}

pub async fn add(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<AddKeys>,
) -> Response {
    match state.store.add_keys(id, &body.keys) {
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
            Json(json!({"error":"Key 不可用于测试：冷却、禁用、权限限制或已有测试运行中"})),
        )
            .into_response(),
    }
}
