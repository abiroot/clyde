//! Clyde — multi-account switcher for Claude Code.

mod alerts;
mod chrome_link;
mod claude_sync;
mod commands;
mod engine;
mod history;
mod import_claude;
mod model;
mod oauth;
mod open_chrome;
mod sessions;
mod settings;
mod usage;
mod vault;

use engine::Core;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, PhysicalPosition, Rect, WindowEvent};

use commands::PendingLogins;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "clyde=info,warn".into()),
        )
        .try_init();

    let core = Core::new().expect("failed to initialize Clyde engine");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        toggle_popover(app, last_tray_rect(app));
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(core.clone())
        .manage(PendingLogins::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::get_chrome_link,
            commands::get_open_chrome,
            commands::setup_open_chrome,
            commands::remove_open_chrome,
            commands::set_builtin_chrome,
            commands::reveal_open_chrome_extension,
            commands::open_chrome_extensions_page,
            open_main_window,
            quit_app,
            js_log,
            commands::copy_text,
            commands::get_settings,
            commands::set_settings,
            commands::get_history,
            commands::list_sessions,
            commands::test_notification,
            commands::get_autostart,
            commands::set_autostart,
            commands::set_active_account,
            commands::rename_account,
            commands::remove_account,
            commands::begin_login,
            commands::complete_login,
            commands::import_token,
            commands::discover_claude_accounts,
            commands::import_claude_accounts,
            commands::start_claude_login,
        ])
        .setup(move |app| {
            core.attach_app(app.handle().clone());

            // Self-heal any stale proxy integration an older Clyde left in
            // settings.json, then reflect whichever account Claude Code is set to.
            if let Ok(true) = claude_sync::cleanup_legacy_integration() {
                tracing::info!("removed a stale Clyde proxy integration from settings.json");
            }
            core.detect_active();
            core.cleanup_orphan_login_dirs();

            // Poll usage so the gauges fill even when no traffic flows: once now,
            // then on a steady interval.
            let core_for_poll = core.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    core_for_poll.poll_usage().await;
                    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                }
            });

            build_tray(app)?;
            core.update_tray_readout();
            apply_shortcut(app.handle(), &core.settings().shortcut);

            // Debug builds only: `CLYDE_SHOW_POPOVER=1` opens the popover at launch,
            // anchored to the top-right of the main screen, for screenshots.
            // Debug builds only: `CLYDE_PAGE=usage` opens that page of the main window.
            #[cfg(debug_assertions)]
            if let (Ok(page), Some(main)) =
                (std::env::var("CLYDE_PAGE"), app.get_webview_window("main"))
            {
                let page: String = page.chars().filter(|c| c.is_ascii_alphabetic()).collect();
                // After the page has loaded; an earlier hash change is lost on load.
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(6));
                    let _ = main.eval(format!("window.location.hash = '{page}'"));
                });
            }

            #[cfg(debug_assertions)]
            if std::env::var("CLYDE_SHOW_POPOVER").is_ok() {
                toggle_popover(app.handle(), last_tray_rect(app.handle()));
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Menubar-style: closing the window hides it instead of quitting.
            WindowEvent::CloseRequested { api, .. } => {
                let _ = window.hide();
                api.prevent_close();
            }
            // The popover behaves like a native menubar popover: click away, it goes.
            WindowEvent::Focused(false) if window.label() == POPOVER => {
                let _ = window.hide();
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running Clyde");
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Clyde", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Clyde", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    TrayIconBuilder::with_id(engine::TRAY_ID)
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Clyde — Claude account switcher")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                *LAST_TRAY_RECT.lock().unwrap() = Some(rect);
                toggle_popover(tray.app_handle(), rect);
            }
        })
        .build(app)?;
    Ok(())
}

const POPOVER: &str = "popover";

/// Where the tray icon was last clicked, so the global shortcut can open the
/// popover in the same place.
static LAST_TRAY_RECT: std::sync::Mutex<Option<Rect>> = std::sync::Mutex::new(None);

/// The last known tray rect, or a guess near the right of the main screen's
/// menu bar (before the icon has ever been clicked).
fn last_tray_rect(app: &tauri::AppHandle) -> Rect {
    if let Some(r) = *LAST_TRAY_RECT.lock().unwrap() {
        return r;
    }
    let (right, scale) = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            (
                (m.position().x + m.size().width as i32) as f64,
                m.scale_factor(),
            )
        })
        .unwrap_or((1440.0, 2.0));
    Rect {
        position: tauri::PhysicalPosition::new(right - 240.0 * scale, 0.0).into(),
        size: tauri::PhysicalSize::new(22.0 * scale, 24.0 * scale).into(),
    }
}

/// (Re)register the popover shortcut; an empty string disables it.
pub(crate) fn apply_shortcut(app: &tauri::AppHandle, shortcut: &str) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if !shortcut.is_empty() {
        if let Err(e) = gs.register(shortcut) {
            tracing::warn!("couldn't register shortcut {shortcut}: {e}");
        }
    }
}

/// Gap between the menu bar and the popover, in logical points.
const POPOVER_GAP: f64 = 6.0;

/// Show the popover centred under the tray icon, or hide it if it's open.
fn toggle_popover(app: &tauri::AppHandle, tray: Rect) {
    let Some(pop) = app.get_webview_window(POPOVER) else {
        return show_main(app);
    };
    if pop.is_visible().unwrap_or(false) {
        let _ = pop.hide();
        return;
    }

    let scale = pop.scale_factor().unwrap_or(2.0);
    let icon_pos = tray.position.to_physical::<f64>(scale);
    let icon_size = tray.size.to_physical::<f64>(scale);
    let win = pop
        .outer_size()
        .map(|s| s.cast::<f64>())
        .unwrap_or_default();

    let mut x = icon_pos.x + icon_size.width / 2.0 - win.width / 2.0;
    let y = icon_pos.y + icon_size.height + POPOVER_GAP * scale;
    // Keep it on the screen the icon is on (icons near the right edge).
    if let Ok(Some(m)) = app.monitor_from_point(icon_pos.x, icon_pos.y) {
        let left = m.position().x as f64;
        let right = left + m.size().width as f64;
        x = x.clamp(
            left + 8.0 * scale,
            (right - win.width - 8.0 * scale).max(left),
        );
    }

    let _ = pop.set_position(PhysicalPosition::new(x, y));
    let _ = pop.show();
    let _ = pop.set_focus();
}

/// "Open Clyde" from the popover: bring up the full window.
#[tauri::command]
fn open_main_window(app: tauri::AppHandle) {
    if let Some(pop) = app.get_webview_window(POPOVER) {
        let _ = pop.hide();
    }
    show_main(&app);
}

/// Frontend errors, forwarded to the Rust log (the webview console isn't
/// visible outside devtools).
#[tauri::command]
fn js_log(window: tauri::Window, message: String) {
    tracing::warn!("[{}] {message}", window.label());
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
