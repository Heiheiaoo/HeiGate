//! Tauri desktop shell. It owns the platform window and tray, starts the local
//! `heigate-core` HTTP server in-process, and points the webview at it so the
//! frontend's relative-path API calls hit the gateway exactly as they did under
//! the previous native shells.

use std::path::PathBuf;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// Bridges the message handlers the UI probes for desktop mode onto Tauri's
/// window commands. The frontend renders its own window chrome unless
/// `window.webkit.messageHandlers.dragWindow` exists, and drives window
/// dragging / zooming through it — the previous Swift shell registered both on
/// its WKWebView. Without this shim the UI would draw a second set of traffic
/// lights under the native ones.
const DESKTOP_BRIDGE: &str = r#"
(function () {
  function call(command) {
    try {
      var internals = window.__TAURI_INTERNALS__;
      if (internals && internals.invoke) internals.invoke(command, {});
    } catch (e) { /* the native title bar stays draggable regardless */ }
  }
  var webkit = window.webkit || (window.webkit = {});
  var handlers = webkit.messageHandlers || (webkit.messageHandlers = {});
  if (!handlers.dragWindow) {
    handlers.dragWindow = { postMessage: function () { call('plugin:window|start_dragging'); } };
  }
  if (!handlers.zoomWindow) {
    handlers.zoomWindow = { postMessage: function () { call('plugin:window|toggle_maximize'); } };
  }
})();
"#;

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

/// Applies the platform window chrome the UI's layout expects. On macOS the
/// content must extend under a transparent title bar, because the UI draws its
/// own header where the traffic lights sit; the title bar style is macOS-only,
/// so other platforms keep the native chrome.
fn with_platform_chrome<'a, R: tauri::Runtime, M: tauri::Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    #[cfg(target_os = "macos")]
    {
        builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        builder
    }
}

/// Brings the main window back to the foreground.
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
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

            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("HeiGate")
                .inner_size(1200.0, 800.0)
                .min_inner_size(900.0, 600.0)
                .initialization_script(DESKTOP_BRIDGE);

            let window = with_platform_chrome(builder).build()?;

            // Closing the window hides it: the gateway keeps serving and the
            // tray icon is how the user brings it back. Quitting is explicit.
            let hide_target = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = hide_target.hide();
                }
            });

            let handle = app.handle();
            let autostart_on = handle.autolaunch().is_enabled().unwrap_or(false);
            let show_item = MenuItem::with_id(handle, "show", "显示主窗口", true, None::<&str>)?;
            let autostart_item = CheckMenuItem::with_id(
                handle,
                "autostart",
                "开机自启",
                true,
                autostart_on,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(handle, "quit", "退出 HeiGate", true, None::<&str>)?;
            let menu = Menu::with_items(
                handle,
                &[
                    &show_item,
                    &autostart_item,
                    &PredefinedMenuItem::separator(handle)?,
                    &quit_item,
                ],
            )?;

            let autostart_entry = autostart_item.clone();
            TrayIconBuilder::with_id("main")
                .icon(
                    app.default_window_icon()
                        .cloned()
                        .expect("the bundle ships a window icon"),
                )
                .tooltip("HeiGate")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "autostart" => {
                        let manager = app.autolaunch();
                        if manager.is_enabled().unwrap_or(false) {
                            let _ = manager.disable();
                        } else {
                            let _ = manager.enable();
                        }
                        let _ = autostart_entry.set_checked(manager.is_enabled().unwrap_or(false));
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
