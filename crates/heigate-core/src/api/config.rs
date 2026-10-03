use super::handlers::SharedState;
use crate::store_config::Backup;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

fn result<T: serde::Serialize>(value: crate::store::Result<T>) -> Response {
    match value {
        Ok(data) => Json(json!({"data":data})).into_response(),
        Err(crate::store::StoreError::Invalid(message)) => {
            (StatusCode::BAD_REQUEST, Json(json!({"error":message}))).into_response()
        }
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"配置操作失败，没有提交任何修改"})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct ExportQuery {
    #[serde(default)]
    include_secrets: bool,
}
pub async fn export(
    State(state): State<SharedState>,
    Query(query): Query<ExportQuery>,
) -> Response {
    let mut response = result(state.store.export_config(query.include_secrets));
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
pub async fn preview(State(state): State<SharedState>, Json(backup): Json<Backup>) -> Response {
    result(state.store.preview_import(&backup))
}
pub async fn import(State(state): State<SharedState>, Json(backup): Json<Backup>) -> Response {
    result(state.store.import_config(&backup))
}
