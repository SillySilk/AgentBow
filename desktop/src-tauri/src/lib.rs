mod anthropic;
mod auth;
mod local_llm;
mod router;
mod server;
mod state;
mod tools;

use state::{AppState, Config};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};
use tracing::info;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bow_desktop=debug".parse().unwrap()),
        )
        .init();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!(
                "Bow failed to start:\n\n{}\n\nEdit C:\\AI\\agent Bow\\desktop\\.env and ensure all keys are set.",
                e
            );
            eprintln!("{}", msg);
            let _ = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-Command",
                    &format!("Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('{}', 'Bow Error')", msg.replace('\'', "`'"))
                ])
                .spawn();
            std::process::exit(1);
        }
    };
    info!("Bow starting — WS port {}", config.ws_port);

    // Capture values needed in tray handlers
    let ws_port = config.ws_port;
    let workspace = config.workspace_root.to_string_lossy().to_string();
    let model_display = {
        let m = &config.model;
        if m.contains("opus") { "Claude Opus".to_string() }
        else if m.contains("sonnet") { "Claude Sonnet".to_string() }
        else if m.contains("haiku") { "Claude Haiku".to_string() }
        else { m.clone() }
    };

    let app_state = AppState::new(config.clone());

    tauri::Builder::default()
        .setup(move |app| {
            // Start WebSocket server
            let ws_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = server::start(ws_state).await {
                    eprintln!("WebSocket server error: {}", e);
                }
            });

            // ── Tray menu ─────────────────────────────────────────────────────
            let status_label = format!("Bow AI  ·  {}  ·  port {}", model_display, ws_port);
            let status_item  = MenuItem::with_id(app, "status",    &status_label,         false, None::<&str>)?;
            let sep1         = PredefinedMenuItem::separator(app)?;
            let show_item    = MenuItem::with_id(app, "show",      "Open Chat Window",    true,  None::<&str>)?;
            let workspace_item = MenuItem::with_id(app, "workspace", "Open Workspace Folder", true, None::<&str>)?;
            let settings_item  = MenuItem::with_id(app, "settings",  "Edit Settings (.env)", true, None::<&str>)?;
            let sep2         = PredefinedMenuItem::separator(app)?;
            let quit_item    = MenuItem::with_id(app, "quit",      "Quit Bow",            true,  None::<&str>)?;

            let menu = Menu::with_items(app, &[
                &status_item,
                &sep1,
                &show_item,
                &workspace_item,
                &settings_item,
                &sep2,
                &quit_item,
            ])?;

            let tooltip = format!("Bow AI Agent — port {}", ws_port);
            let workspace_clone = workspace.clone();

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip(&tooltip)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "workspace" => {
                        let _ = std::process::Command::new("explorer.exe")
                            .arg(&workspace_clone)
                            .spawn();
                    }
                    "settings" => {
                        let _ = std::process::Command::new("notepad.exe")
                            .arg(r"C:\AI\agent Bow\desktop\.env")
                            .spawn();
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // Left-click toggles the chat window
                    if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // X button quits; tray left-click handles show/hide separately
            if let WindowEvent::CloseRequested { .. } = event {
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
