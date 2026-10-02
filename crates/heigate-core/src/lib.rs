//! HeiGate core: the local AI API gateway. Ported from the Go backend in
//! `internal/`, kept free of any Tauri dependency so it stays independently
//! testable.

pub mod api;
pub mod auth;
pub mod canonical;
pub mod datadir;
pub mod key_pool;
pub mod middleware;
pub mod probe;
pub mod protocol;
pub mod resilience;
pub mod router;
pub mod runner;
pub mod server;
pub mod store;
pub mod store_config;
pub mod store_keys;
pub mod version;
