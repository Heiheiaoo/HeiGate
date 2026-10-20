//! Tauri desktop shell. It owns the platform window and tray, starts the local
//! `heigate-core` HTTP server in-process, and points the webview at it so the
//! frontend's relative-path API calls hit the gateway exactly as they did under
//! the previous native shells.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

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

/// The window size in logical points for a display of `available` size. The
/// normal layout is 1024x680, but a small display gets a size that fits inside
/// its usable area so the minimum window size never makes the app unreachable.
fn limits_for(available: Option<(f64, f64)>) -> (f64, f64) {
    const IDEAL: (f64, f64) = (1024.0, 680.0);
    const ABSOLUTE_MIN: (f64, f64) = (320.0, 240.0);
    // Room for the menu bar, the Dock and the window chrome.
    const MARGIN: (f64, f64) = (40.0, 120.0);

    let (available_w, available_h) = available.unwrap_or((IDEAL.0 + MARGIN.0, IDEAL.1 + MARGIN.1));
    let usable_w = (available_w - MARGIN.0).max(ABSOLUTE_MIN.0);
    let usable_h = (available_h - MARGIN.1).max(ABSOLUTE_MIN.1);
    (IDEAL.0.min(usable_w), IDEAL.1.min(usable_h))
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
            let mut frontend_hasher = std::collections::hash_map::DefaultHasher::new();
            std::fs::read(static_dir.join("index.html"))
                .unwrap_or_default()
                .hash(&mut frontend_hasher);
            let frontend_cache_key = frontend_hasher.finish();
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

            tauri::async_runtime::spawn(async move {
                if let Err(err) =
                    heigate_core::server::run_default_with_ready(bound, static_dir, ready_tx).await
                {
                    eprintln!("heigate: gateway server exited: {err}");
                }
            });

            let ready = tauri::async_runtime::block_on(async {
                tokio::time::timeout(Duration::from_secs(5), ready_rx).await
            });
            match ready {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(err))) => return Err(std::io::Error::other(err).into()),
                Ok(Err(_)) | Err(_) => {
                    return Err(std::io::Error::other("HeiGate 网关启动超时").into())
                }
            }

            let url = format!("http://127.0.0.1:{port}/?ui={frontend_cache_key:016x}")
                .parse::<tauri::Url>()
                .expect("valid local server url");

            let limits = size_limits(app.primary_monitor().ok().flatten().as_ref());
            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("HeiGate")
                .inner_size(limits.0, limits.1)
                // The minimum follows the primary display when it is smaller than the ideal layout.
                .min_inner_size(limits.0, limits.1);

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
                    // Moving to another display: update the minimum and shrink only when exceeding monitor bounds.
                    WindowEvent::Moved(_) => {
                        if let Some(monitor) = move_target.current_monitor().ok().flatten() {
                            let mon_scale = monitor.scale_factor();
                            let mon_w = monitor.size().width as f64 / mon_scale;
                            let mon_h = monitor.size().height as f64 / mon_scale;
                            let limits = limits_for(Some((mon_w, mon_h)));
                            let _ = move_target
                                .set_min_size(Some(tauri::LogicalSize::new(limits.0, limits.1)));
                            let scale = move_target.scale_factor().unwrap_or(1.0);
                            if let Ok(size) = move_target.inner_size() {
                                let (w, h) =
                                    (size.width as f64 / scale, size.height as f64 / scale);
                                if w > mon_w || h > mon_h {
                                    let _ = move_target.set_size(tauri::LogicalSize::new(
                                        w.min(mon_w),
                                        h.min(mon_h),
                                    ));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            });

            let handle = app.handle();
            let app_submenu = Submenu::with_items(
                handle,
                "HeiGate",
                true,
                &[
                    &PredefinedMenuItem::about(handle, Some("关于 HeiGate"), None)?,
                    &PredefinedMenuItem::separator(handle)?,
                    &PredefinedMenuItem::hide(handle, Some("隐藏 HeiGate"))?,
                    &PredefinedMenuItem::hide_others(handle, Some("隐藏其他"))?,
                    &PredefinedMenuItem::show_all(handle, Some("显示全部"))?,
                    &PredefinedMenuItem::separator(handle)?,
                    &PredefinedMenuItem::quit(handle, Some("退出 HeiGate"))?,
                ],
            )?;
            let file_submenu = Submenu::with_items(
                handle,
                "文件",
                true,
                &[&PredefinedMenuItem::close_window(handle, Some("关闭窗口"))?],
            )?;
            let edit_submenu = Submenu::with_items(
                handle,
                "编辑",
                true,
                &[
                    &PredefinedMenuItem::undo(handle, Some("撤销"))?,
                    &PredefinedMenuItem::redo(handle, Some("重做"))?,
                    &PredefinedMenuItem::separator(handle)?,
                    &PredefinedMenuItem::cut(handle, Some("剪切"))?,
                    &PredefinedMenuItem::copy(handle, Some("复制"))?,
                    &PredefinedMenuItem::paste(handle, Some("粘贴"))?,
                    &PredefinedMenuItem::select_all(handle, Some("全选"))?,
                ],
            )?;
            let view_submenu = Submenu::with_items(
                handle,
                "视图",
                true,
                &[&PredefinedMenuItem::fullscreen(handle, Some("切换全屏幕"))?],
            )?;
            let window_submenu = Submenu::with_items(
                handle,
                "窗口",
                true,
                &[
                    &PredefinedMenuItem::minimize(handle, Some("最小化"))?,
                    &PredefinedMenuItem::separator(handle)?,
                    &MenuItem::with_id(
                        handle,
                        "show_main_win",
                        "前置全部窗口",
                        true,
                        None::<&str>,
                    )?,
                ],
            )?;
            let help_submenu = Submenu::with_items(
                handle,
                "帮助",
                true,
                &[&MenuItem::with_id(
                    handle,
                    "open_help",
                    "HeiGate 帮助",
                    true,
                    None::<&str>,
                )?],
            )?;
            let app_menu = Menu::with_items(
                handle,
                &[
                    &app_submenu,
                    &file_submenu,
                    &edit_submenu,
                    &view_submenu,
                    &window_submenu,
                    &help_submenu,
                ],
            )?;
            app.set_menu(app_menu)?;
            app.on_menu_event(move |app, event| match event.id.as_ref() {
                "show_main_win" | "open_help" => show_main_window(app),
                _ => {}
            });

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

            #[cfg(target_os = "macos")]
            let hide_dock = {
                if let Ok(dir) = heigate_core::datadir::ensure() {
                    if let Ok(store) = heigate_core::store::Store::open(dir.join("heigate.sqlite"))
                    {
                        store
                            .get_setting("hide_dock_icon", "false")
                            .map(|v| v == "true" || v == "1")
                            .unwrap_or(false)
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            #[cfg(target_os = "macos")]
            if hide_dock {
                let _ = handle.set_dock_visibility(false);
            }

            #[cfg(target_os = "macos")]
            let hide_dock_item = CheckMenuItem::with_id(
                handle,
                "hide_dock",
                "隐藏 Dock 图标",
                true,
                hide_dock,
                None::<&str>,
            )?;

            let quit_item = MenuItem::with_id(handle, "quit", "退出 HeiGate", true, None::<&str>)?;

            #[cfg(target_os = "macos")]
            let menu = Menu::with_items(
                handle,
                &[
                    &show_item,
                    &autostart_item,
                    &hide_dock_item,
                    &PredefinedMenuItem::separator(handle)?,
                    &quit_item,
                ],
            )?;

            #[cfg(not(target_os = "macos"))]
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
            #[cfg(target_os = "macos")]
            let hide_dock_entry = hide_dock_item.clone();

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
                    #[cfg(target_os = "macos")]
                    "hide_dock" => {
                        if let Ok(dir) = heigate_core::datadir::ensure() {
                            if let Ok(store) =
                                heigate_core::store::Store::open(dir.join("heigate.sqlite"))
                            {
                                let current = store
                                    .get_setting("hide_dock_icon", "false")
                                    .map(|v| v == "true" || v == "1")
                                    .unwrap_or(false);
                                let new_val = !current;
                                let _ = store.set_setting("hide_dock_icon", &new_val.to_string());
                                let _ = app.set_dock_visibility(!new_val);
                                let _ = hide_dock_entry.set_checked(new_val);
                                heigate_core::settings_events::notify_setting_change(
                                    "hide_dock_icon",
                                    &new_val.to_string(),
                                );
                            }
                        }
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

            #[cfg(target_os = "macos")]
            {
                let mut rx = heigate_core::settings_events::settings_subscribe();
                let app_handle = handle.clone();
                let hide_dock_tray = hide_dock_item.clone();
                tauri::async_runtime::spawn(async move {
                    while let Ok((key, val)) = rx.recv().await {
                        if key == "hide_dock_icon" {
                            let hide = val == "true" || val == "1";
                            let _ = app_handle.set_dock_visibility(!hide);
                            let _ = hide_dock_tray.set_checked(hide);
                        }
                    }
                });
            }

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
        // A large display gets the layout's ideal size (1024x680).
        assert_eq!(limits_for(Some((2560.0, 1440.0))), (1024.0, 680.0));
        // A portrait display is narrower than the ideal width.
        assert_eq!(limits_for(Some((900.0, 1920.0))), (860.0, 680.0));
        // A small display still gets a window that fits its usable area.
        assert_eq!(limits_for(Some((600.0, 400.0))), (560.0, 280.0));
        // Without a monitor, assume the ideal fits.
        assert_eq!(limits_for(None), (1024.0, 680.0));
    }
}
