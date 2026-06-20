use crate::state::{AppState, Config};
use std::path::PathBuf;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIconBuilder,
};
use tracing::info;

/// Resolve the directory holding the built web UI (index.html, assets).
/// Dev: `desktop/webapp/dist` next to the project. Release: `web/` next to the exe.
fn web_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let next = exe.parent().map(|p| p.join("web"));
        if let Some(d) = next {
            if d.join("index.html").exists() {
                return d;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../webapp/dist")
}

fn fatal_config_box(msg: &str) {
    eprintln!("{}", msg);
    let _ = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('{}', 'Bow Error')",
                msg.replace('\'', "`'")
            ),
        ])
        .spawn();
}

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
            fatal_config_box(&format!(
                "Bow failed to start:\n\n{}\n\nEdit C:\\AI\\agent Bow\\desktop\\.env and ensure all keys are set.",
                e
            ));
            std::process::exit(1);
        }
    };
    let ws_port = config.ws_port;
    let workspace = config.workspace_root.to_string_lossy().to_string();
    info!("Bow starting — http://127.0.0.1:{}", ws_port);

    let app_state = AppState::new(config.clone());

    // tokio runtime on a background thread; tao event loop owns the main thread.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server_state = app_state.clone();
    let dir = web_dir();
    rt.spawn(async move {
        // MUST run inside the async runtime: load_in_background spawns a task.
        let mcp = crate::tools::mcp::McpManager::load_in_background(workspace.clone());
        let router = crate::http::build_router(server_state, mcp, dir);
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], ws_port));
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                fatal_config_box(&format!(
                    "Bow failed to bind 127.0.0.1:{}:\n\n{}\n\nThe port may already be in use by another instance of Bow.",
                    ws_port, e
                ));
                std::process::exit(1);
            }
        };
        info!("HTTP+WS listening on http://{}", addr);
        if let Err(e) = axum::serve(listener, router).await {
            fatal_config_box(&format!("Bow HTTP server error:\n\n{}", e));
            std::process::exit(1);
        }
    });

    // Open the browser once the server is up.
    let url = format!("http://127.0.0.1:{}", ws_port);
    std::thread::spawn({
        let url = url.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &url])
                .spawn();
        }
    });

    // Tray + event loop (main thread).
    let icon = load_tray_icon();
    let menu = Menu::new();
    let open_i = MenuItem::new("Open Bow", true, None);
    let ws_i = MenuItem::new("Open Workspace", true, None);
    let env_i = MenuItem::new("Edit Settings (.env)", true, None);
    let quit_i = MenuItem::new("Quit Bow", true, None);
    menu.append_items(&[
        &open_i,
        &PredefinedMenuItem::separator(),
        &ws_i,
        &env_i,
        &PredefinedMenuItem::separator(),
        &quit_i,
    ])
    .unwrap();

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(format!("Bow Image Studio — port {}", ws_port))
        .with_icon(icon)
        .build()
        .expect("tray icon");

    let event_loop = EventLoopBuilder::new().build();
    let menu_channel = MenuEvent::receiver();
    let workspace_path = config.workspace_root.clone();
    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Ok(ev) = menu_channel.try_recv() {
            if ev.id == open_i.id() {
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", "", &url])
                    .spawn();
            } else if ev.id == ws_i.id() {
                let _ = std::process::Command::new("explorer.exe")
                    .arg(&workspace_path)
                    .spawn();
            } else if ev.id == env_i.id() {
                let _ = std::process::Command::new("notepad.exe")
                    .arg(r"C:\AI\agent Bow\desktop\.env")
                    .spawn();
            } else if ev.id == quit_i.id() {
                std::process::exit(0);
            }
        }
    });
}

fn load_tray_icon() -> tray_icon::Icon {
    let bytes = include_bytes!("../icons/icon32.png");
    let img = image::load_from_memory(bytes)
        .expect("decode tray icon")
        .to_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).expect("tray icon from rgba")
}
