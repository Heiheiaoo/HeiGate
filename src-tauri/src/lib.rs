//! Tauri desktop shell. It owns the platform window and starts the local
//! `heigate-core` HTTP server in-process, then points the webview at it so the
//! frontend's relative-path API calls hit the gateway exactly as they did
//! under the previous native shells.

use std::path::PathBuf;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// Resolves the directory holding the built frontend: an explicit override, the
/// bundled resource, or the repository checkout during development.
fn static_dir(app: &tauri::AppHandle) -> PathBuf {
    if let Some(dir) = std::env::var_os("HEIGATE_STATIC_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = app
        .path()
        .resolve("dist", tauri::path::BaseDirectory::Resource)
    {
        if dir.join("index.html").exists() {
            return dir;
        }
    }
    // Fallback for a bundle launched without LaunchServices resolving it:
    // `<exe>/../Resources/dist`.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            let resources = macos_dir.join("..").join("Resources").join("dist");
            if resources.join("index.html").exists() {
                return resources;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("frontend")
        .join("dist")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let bound = heigate_core::server::bind()?;
            let port = bound.port;
            let static_dir = static_dir(&app.handle().clone());

            tauri::async_runtime::spawn(async move {
                if let Err(err) = heigate_core::server::run_default(bound, static_dir).await {
                    eprintln!("heigate: gateway server exited: {err}");
                }
            });

            let url = format!("http://127.0.0.1:{port}/")
                .parse::<tauri::Url>()
                .expect("valid local server url");

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("HeiGate")
                .inner_size(1200.0, 800.0)
                .min_inner_size(900.0, 600.0)
                .build()?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
