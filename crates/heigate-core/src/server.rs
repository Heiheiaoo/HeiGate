//! The local HTTP server: binds a loopback port the desktop shell points its
//! webview at, and serves `/health`, the management API (`/api/*`), the gateway
//! (`/v1/*`) and the built single-page frontend.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, post, put};
use axum::{extract::DefaultBodyLimit, middleware, Json, Router};
use serde_json::{json, Value};
use tokio::sync::oneshot;

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
pub const PORT_CANDIDATES: std::ops::RangeInclusive<u16> = 28888..=28898;
pub const MAX_REQUEST_BODY: usize = 16 << 20;

/// A bound-but-not-yet-serving listener plus the port it landed on.
pub struct Bound {
    pub listener: TcpListener,
    pub port: u16,
}

fn configured_port(value: Option<&str>) -> Option<u16> {
    value
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
}

/// Binds a loopback listener, trying `HEIGATE_PORT` first, then 28888–28898,
/// and finally an OS-assigned port.
pub fn bind() -> std::io::Result<Bound> {
    if let Some(port) = configured_port(std::env::var("HEIGATE_PORT").ok().as_deref()) {
        if let Ok(listener) = TcpListener::bind(SocketAddr::new(HOST.into(), port)) {
            return Ok(Bound { listener, port });
        }
    }
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
pub fn build_state(gateway_port: u16) -> std::io::Result<SharedState> {
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
    let gateway = Arc::new(Gateway::new(store.clone(), router.clone()));
    let runner = Arc::new(ProbeRunner::with_key_pool(
        store.clone(),
        gateway.key_pool.clone(),
    ));
    Ok(Arc::new(AppState {
        store,
        runner,
        router,
        gateway,
        gateway_key,
        gateway_port,
        version: VERSION.trim().to_string(),
    }))
}

/// Builds the full application router.
pub fn router(state: SharedState, static_dir: PathBuf) -> Router {
    let key = state.gateway_key.clone();

    let api = Router::new()
        .route("/config/export", get(crate::api::config::export))
        .route("/config/preview", post(crate::api::config::preview))
        .route("/config/import", post(crate::api::config::import))
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
        .route("/channels/detect-protocol", post(handlers::detect_protocol))
        .route(
            "/channels/{id}",
            get(handlers::get_channel)
                .put(handlers::update_channel)
                .delete(handlers::delete_channel),
        )
        .route("/channels/{id}/probe", post(handlers::probe_channel))
        .route(
            "/channels/{id}/keys/{key_id}/probe",
            post(crate::api::keys::probe),
        )
        .route(
            "/channels/{id}/keys/{key_id}/secret",
            get(crate::api::keys::reveal),
        )
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
        .route("/clients/config", get(handlers::get_client_configs))
        .route(
            "/clients/codex/status",
            get(handlers::get_codex_config_status),
        )
        .route("/clients/codex/apply", post(handlers::apply_codex_config))
        .route(
            "/clients/codex/restore",
            post(handlers::restore_codex_config),
        )
        .route("/clients/probe", post(handlers::probe_client))
        .route("/clients/{id}/config", put(handlers::update_client_config))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY))
        .layer(middleware::from_fn(same_origin_only));

    let gateway = Router::new()
        .route("/v1/messages", post(handlers::forward_messages))
        .route(
            "/v1/chat/completions",
            post(handlers::forward_chat_completions),
        )
        .route("/v1/responses", post(handlers::forward_responses))
        .route("/v1/models", get(handlers::gateway_models))
        .route(
            "/{client}/v1/messages",
            post(handlers::forward_client_messages),
        )
        .route(
            "/{client}/v1/chat/completions",
            post(handlers::forward_client_chat_completions),
        )
        .route(
            "/{client}/v1/responses",
            post(handlers::forward_client_responses),
        )
        .route("/{client}/v1/models", get(handlers::gateway_client_models))
        .route(
            "/{client}/messages",
            post(handlers::forward_client_messages),
        )
        .route(
            "/{client}/chat/completions",
            post(handlers::forward_client_chat_completions),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY))
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
    serve_with_ready(bound, static_dir, state, None).await
}

/// Serves the application and optionally signals once the router has been
/// assembled and is ready to accept connections. Desktop shells use this to
/// avoid opening a webview against a listener that has not started serving.
pub async fn serve_with_ready(
    bound: Bound,
    static_dir: PathBuf,
    state: SharedState,
    ready: Option<oneshot::Sender<Result<(), String>>>,
) -> std::io::Result<()> {
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
    if let Some(ready) = ready {
        let _ = ready.send(Ok(()));
    }
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Convenience entry point: builds the default state and serves it.
pub async fn run_default(bound: Bound, static_dir: PathBuf) -> std::io::Result<()> {
    let state = build_state(bound.port)?;
    serve(bound, static_dir, state).await
}

/// Builds the default state and signals readiness for desktop startup.
pub async fn run_default_with_ready(
    bound: Bound,
    static_dir: PathBuf,
    ready: oneshot::Sender<Result<(), String>>,
) -> std::io::Result<()> {
    let state = match build_state(bound.port) {
        Ok(state) => state,
        Err(err) => {
            let message = err.to_string();
            let _ = ready.send(Err(message.clone()));
            return Err(std::io::Error::other(message));
        }
    };
    serve_with_ready(bound, static_dir, state, Some(ready)).await
}
