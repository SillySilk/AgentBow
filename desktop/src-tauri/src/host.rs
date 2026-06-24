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

/// Free the fixed port so a fresh launch can take it over. The usual cause is a
/// previous Bow instance still running, so first kill every other `bow-desktop.exe`
/// by image name (reliable, locale-independent), then defensively kill whatever else
/// is LISTENING on the port. Windows-only; no console window flashes; never our PID.
#[cfg(windows)]
fn free_port(port: u16) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let me = std::process::id();

    // 1) Kill stale Bow instances by name, excluding ourselves. This is the reliable
    //    path — it doesn't depend on parsing netstat output.
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "bow-desktop.exe", "/FI", &format!("PID ne {}", me)])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    // 2) Defensive: kill any non-Bow process LISTENING on the port.
    let output = match std::process::Command::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => o,
        Err(_) => return,
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let needle = format!(":{}", port);
    let mut killed = std::collections::HashSet::new();
    for line in text.lines() {
        if !line.contains("LISTENING") {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        // netstat tcp row: Proto  Local  Foreign  State  PID
        if cols.len() < 5 || !cols[1].ends_with(&needle) {
            continue;
        }
        if let Ok(pid) = cols[4].parse::<u32>() {
            if pid != 0 && pid != me && killed.insert(pid) {
                info!("Port {} held by PID {} — terminating stale instance", port, pid);
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/PID", &pid.to_string()])
                    .creation_flags(CREATE_NO_WINDOW)
                    .output();
            }
        }
    }
}

#[cfg(not(windows))]
fn free_port(_port: u16) {}

/// Bind `preferred`, recovering from a stale holder. If it still can't be bound —
/// e.g. the port is reserved by Windows/Hyper-V/WSL and has *no* killable owner — fall
/// back to the next free port in a small range. Returns the listener and the port
/// actually bound (which the browser-open + tray then use).
async fn bind_with_recovery(preferred: u16) -> Option<(tokio::net::TcpListener, u16)> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], preferred));
    if let Ok(l) = tokio::net::TcpListener::bind(addr).await {
        return Some((l, preferred));
    }

    // Busy — free a stale instance and retry the preferred port over ~4s. A
    // force-killed listener can take a second or two for Windows to release.
    info!("Port {} busy — freeing stale instance and retrying", preferred);
    free_port(preferred);
    for attempt in 1..=10 {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        if let Ok(l) = tokio::net::TcpListener::bind(addr).await {
            info!("Port {} reclaimed", preferred);
            return Some((l, preferred));
        }
        if attempt == 5 {
            free_port(preferred);
        }
    }

    // Still stuck — most likely an OS-reserved port with no owner to kill. Fall back
    // to the next available port so Bow always launches.
    for p in (preferred + 1)..=(preferred + 10) {
        let a = std::net::SocketAddr::from(([127, 0, 0, 1], p));
        if let Ok(l) = tokio::net::TcpListener::bind(a).await {
            info!("Port {} unavailable — falling back to {}", preferred, p);
            return Some((l, p));
        }
    }
    None
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
                .add_directive("bow_desktop_lib=debug".parse().unwrap()),
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

    // Bind synchronously up front so the browser-open + tray use the port we actually
    // got (the preferred one after freeing a stale instance, or a fallback if reserved).
    let (listener, actual_port) = match rt.block_on(bind_with_recovery(ws_port)) {
        Some(x) => x,
        None => {
            fatal_config_box(&format!(
                "Bow failed to bind 127.0.0.1:{} or any nearby port.\n\nClose any running Bow instance (or run kill-bow.bat) and try again.",
                ws_port
            ));
            std::process::exit(1);
        }
    };
    info!("HTTP+WS listening on http://127.0.0.1:{}", actual_port);

    let server_state = app_state.clone();
    let dir = web_dir();
    rt.spawn(async move {
        // MUST run inside the async runtime: load_in_background spawns a task.
        let mcp = crate::tools::mcp::McpManager::load_in_background(workspace.clone());
        let router = crate::http::build_router(server_state, mcp, dir);
        if let Err(e) = axum::serve(listener, router).await {
            fatal_config_box(&format!("Bow HTTP server error:\n\n{}", e));
            std::process::exit(1);
        }
    });

    // Open the browser once the server is up.
    let url = format!("http://127.0.0.1:{}", actual_port);
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
        .with_tooltip(format!("Bow Image Studio — port {}", actual_port))
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
