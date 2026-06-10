//! Clyde — multi-account switcher for Claude Code.

mod claude_sync;
mod commands;
mod engine;
mod import_claude;
mod model;
mod oauth;
mod usage;
mod vault;

use engine::Core;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

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
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(core.clone())
        .manage(PendingLogins::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
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
            Ok(())
        })
        .on_window_event(|window, event| {
            // Menubar-style: closing the window hides it instead of quitting.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Clyde");
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Clyde", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Clyde", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    TrayIconBuilder::with_id("clyde-tray")
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
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
