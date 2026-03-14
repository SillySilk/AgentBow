mod anthropic;
mod auth;
mod local_llm;
mod router;
mod server;
mod state;
mod tools;

use state::{AppState, Config};
use tauri::{
    menu::{Menu, MenuItem},
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
            // Show visible error dialog instead of silent crash
            let msg = format!(
                "Bow failed to start:\n\n{}\n\nEdit C:\\AI\\agent Bow\\desktop\\.env and ensure all keys are set.",
                e
            );
            eprintln!("{}", msg);
            // Windows message box via powershell so it's visible even in release mode
            let _ = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-Command",
                    &format!("Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('{}', 'Bow Error')", msg.replace('\'', "`'"))
                ])
                .spawn();
            std::process::exit(1);
        }
    };
    info!("Bow starting — WS port {}", config.ws_port);

    let app_state = AppState::new(config.clone());

    tauri::Builder::default()
        .setup(move |app| {
            let ws_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = server::start(ws_state).await {
                    eprintln!("WebSocket server error: {}", e);
                }
            });

            let show_item = MenuItem::with_id(app, "show", "Show Bow", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Bow", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Bow AI Agent")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
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
            if let WindowEvent::CloseRequested { .. } = event {
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
