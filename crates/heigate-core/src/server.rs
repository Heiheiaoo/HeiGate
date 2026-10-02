//! The local HTTP server: binds a loopback port the desktop shell points its
//! webview at, and serves `/health`, the management API (`/api/*`), the gateway
//! (`/v1/*`) and the built single-page frontend.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, post, put};
use axum::{middleware, Json, Router};
use serde_json::{json, Value};

use crate::api::gateway::Gateway;
use crate::api::handlers::SharedState;
use crate::api::handlers::{self, AppState};
use crate::auth::load_or_create_gateway_key;
use crate::datadir;
use crate::middleware::{
    bearer_auth, loopback_only, same_origin_only, security_headers, GatewayKey,
};
use crate::router::Router as ChannelRouter;
use crate::runner::ProbeRunner;
use crate::store::Store;
use crate::version::VERSION;

/// Loopback host the server binds to.
pub const HOST: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// Preferred ports, mirroring the desktop shells' probing order.
pub const PORT_CANDIDATES: std::ops::RangeInclusive<u16> = 18080..=18090;

/// A bound-but-not-yet-serving listener plus the port it landed on.
pub struct Bound {
    pub listener: TcpListener,
    pub port: u16,
}

/// Binds a loopback listener, preferring 18080–18090 and falling back to an
/// OS-assigned port.
pub fn bind() -> std::io::Result<Bound> {
    for port in PORT_CANDIDATES {
        if let Ok(listener) = TcpListener::bind(SocketAddr::new(HOST.into(), port)) {
            return Ok(Bound { listener, port });
        }
    }
    let listener = TcpListener::bind(SocketAddr::new(HOST.into(), 0))?;
    let port = listener.local_addr()?.port();
    Ok(Bound { listener, port })
}

/// Assembles the application state from the local data directory.
pub fn build_state() -> std::io::Result<SharedState> {
    let dir = datadir::ensure()?;
    let store = Store::open(dir.join("heigate.sqlite"))
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    let gateway_key = load_or_create_gateway_key(&datadir::gateway_key_path())?;

    // Restore the persisted custom model mappings: the UI reads them from the
    // in-memory table, which otherwise starts empty after every restart.
    if let Ok(raw) = store.get_setting("custom_model_mappings", "{}") {
        crate::canonical::load_custom_model_mappings(&raw);
    }

    let router = Arc::new(ChannelRouter::new(store.clone()));
    let runner = Arc::new(ProbeRunner::new(store.clone()));
    let gateway = Arc::new(Gateway::new(store.clone(), router.clone()));
    Ok(Arc::new(AppState {
        store,
        runner,
        router,
        gateway,
        gateway_key,
        version: VERSION.trim().to_string(),
    }))
}

/// Builds the full application router.
pub fn router(state: SharedState, static_dir: PathBuf) -> Router {
    let key = state.gateway_key.clone();

    let api = Router::new()
        .route("/session", get(handlers::session))
        .route(
            "/channels",
            get(handlers::list_channels).post(handlers::create_channel),
        )
        .route("/channels/candidates", get(handlers::candidates))
        .route(
            "/channels/upstream-models",
            post(handlers::fetch_upstream_models),
        )
        .route(
            "/channels/{id}",
            get(handlers::get_channel)
                .put(handlers::update_channel)
                .delete(handlers::delete_channel),
        )
        .route("/channels/{id}/probe", post(handlers::probe_channel))
        .route(
            "/channels/{id}/keys",
            get(crate::api::keys::list).post(crate::api::keys::add),
        )
        .route(
            "/channels/{id}/keys/{key_id}",
            put(crate::api::keys::update).delete(crate::api::keys::delete),
        )
        .route(
            "/channels/{id}/injection-check",
            post(handlers::injection_check),
        )
        .route(
            "/channels/{id}/duplicate",
            post(handlers::duplicate_channel),
        )
        .route("/models", get(handlers::list_models))
        .route(
            "/custom-model-mappings",
            get(handlers::get_custom_mappings)
                .post(handlers::set_custom_mapping)
                .delete(handlers::delete_custom_mapping),
        )
        .route("/analytics", get(handlers::analytics))
        .route(
            "/logs",
            get(handlers::get_logs).delete(handlers::clear_logs),
        )
        .route("/logs/stream", get(handlers::stream_logs))
        .route("/logs/prune", post(handlers::prune_logs))
        .route("/system", get(handlers::system_info))
        .route("/settings", put(handlers::update_settings))
        .layer(middleware::from_fn(same_origin_only));

    let gateway = Router::new()
        .route("/v1/messages", post(handlers::forward_messages))
        .route(
            "/v1/chat/completions",
            post(handlers::forward_chat_completions),
        )
        .route("/v1/responses", post(handlers::forward_responses))
        .route("/v1/models", get(handlers::gateway_models))
        .layer(middleware::from_fn_with_state(GatewayKey(key), bearer_auth))
        .layer(middleware::from_fn(loopback_only));

    let index = static_dir.join("index.html");
    let static_files = tower_http::services::ServeDir::new(&static_dir)
        .not_found_service(tower_http::services::ServeFile::new(index));

    Router::new()
        .route("/health", get(health))
        .nest("/api", api)
        .merge(gateway)
        .fallback_service(static_files)
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "mode": "desktop",
        "version": VERSION.trim(),
    }))
}

/// Serves until the process exits.
pub async fn serve(bound: Bound, static_dir: PathBuf, state: SharedState) -> std::io::Result<()> {
    // The listener was bound with std; tokio's `from_std` requires the socket
    // to be non-blocking before it can register it.
    bound.listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(bound.listener)?;

    // A periodic prune keeps the log table bounded without a scheduler.
    {
        let store = state.store.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(15 * 60));
            loop {
                ticker.tick().await;
                let _ = store.auto_prune_logs();
            }
        });
    }

    let app = router(state, static_dir);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Convenience entry point: builds the default state and serves it.
pub async fn run_default(bound: Bound, static_dir: PathBuf) -> std::io::Result<()> {
    let state = build_state()?;
    serve(bound, static_dir, state).await
}
