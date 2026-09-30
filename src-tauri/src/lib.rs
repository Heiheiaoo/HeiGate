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

/// The window size in logical points for a display of `available` size: the
/// layout's ideal 1200x800, capped to what that display can actually show — a
/// portrait monitor is narrower than the ideal — and never below a floor that
/// keeps the window usable.
fn limits_for(available: Option<(f64, f64)>) -> (f64, f64) {
    const IDEAL: (f64, f64) = (1200.0, 800.0);
    const FLOOR: (f64, f64) = (640.0, 480.0);
    // Room for the menu bar, the Dock and the window chrome.
    const MARGIN: (f64, f64) = (40.0, 120.0);

    let (available_w, available_h) = available.unwrap_or((IDEAL.0 + MARGIN.0, IDEAL.1 + MARGIN.1));
    (
        IDEAL.0.min((available_w - MARGIN.0).max(FLOOR.0)),
        IDEAL.1.min((available_h - MARGIN.1).max(FLOOR.1)),
    )
}

/// [`limits_for`] using a real monitor's logical size.
fn size_limits(monitor: Option<&tauri::Monitor>) -> (f64, f64) {
    limits_for(monitor.map(|monitor| {
        let scale = monitor.scale_factor();
        (
            monitor.size().width as f64 / scale,
            monitor.size().height as f64 / scale,
        )
    }))
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

/// Renders the tray glyph as a macOS template image, so the menu bar draws it
/// in its own foreground colour (white on a dark bar, black on a light one) and
/// it stays legible over any wallpaper. Other platforms keep the image as-is.
fn with_tray_platform<R: tauri::Runtime>(
    builder: tauri::tray::TrayIconBuilder<R>,
) -> tauri::tray::TrayIconBuilder<R> {
    #[cfg(target_os = "macos")]
    {
        builder.icon_as_template(true)
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

            let limits = size_limits(app.primary_monitor().ok().flatten().as_ref());
            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("HeiGate")
                .inner_size(limits.0, limits.1)
                // The layout is designed for the ideal size, but the window may
                // live on a smaller display (a portrait monitor, for instance),
                // so the minimum never exceeds what that display can show.
                .min_inner_size(limits.0, limits.1)
                .initialization_script(DESKTOP_BRIDGE);

            let window = with_platform_chrome(builder).build()?;

            // Closing the window hides it: the gateway keeps serving and the
            // tray icon is how the user brings it back. Quitting is explicit.
            let hide_target = window.clone();
            let move_target = window.clone();
            window.on_window_event(move |event| {
                match event {
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        let _ = hide_target.hide();
                    }
                    // Moving to another display changes what fits: a portrait
                    // monitor is narrower than the window's ideal width, so the
                    // minimum follows the display and an oversized window is
                    // shrunk to fit rather than left hanging off the edge.
                    WindowEvent::Moved(_) => {
                        let limits =
                            size_limits(move_target.current_monitor().ok().flatten().as_ref());
                        let _ = move_target
                            .set_min_size(Some(tauri::LogicalSize::new(limits.0, limits.1)));

                        let scale = move_target.scale_factor().unwrap_or(1.0);
                        if let Ok(size) = move_target.inner_size() {
                            let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
                            if w > limits.0 || h > limits.1 {
                                let _ = move_target.set_size(tauri::LogicalSize::new(
                                    w.min(limits.0),
                                    h.min(limits.1),
                                ));
                            }
                        }
                    }
                    _ => {}
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

            // A monochrome, transparent glyph rather than the full-colour app
            // icon: menu bar art is drawn in the system's own colour.
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;

            let autostart_entry = autostart_item.clone();
            let tray = TrayIconBuilder::with_id("main")
                .icon(tray_icon)
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
                });
            with_tray_platform(tray).build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::limits_for;

    #[test]
    fn size_limits_follow_the_display() {
        // A large display gets the layout's ideal size.
        assert_eq!(limits_for(Some((2560.0, 1440.0))), (1200.0, 800.0));
        // A portrait display is too narrow for the ideal width.
        assert_eq!(limits_for(Some((1080.0, 1920.0))), (1040.0, 800.0));
        // A small display cannot give up the margin, so the floor applies.
        assert_eq!(limits_for(Some((600.0, 400.0))), (640.0, 480.0));
        // Without a monitor, assume the ideal fits.
        assert_eq!(limits_for(None), (1200.0, 800.0));
    }
}
