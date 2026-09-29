//! DeepSeek Harness - Tauri 2 shell.
//!
//! Startup flow (runs in a background thread so the window renders immediately):
//!   1. ensure an app-private Node.js 24 runtime is available
//!   2. if not, download the latest Node.js 24.x release and unpack it into app-data
//!   3. launch `npx -y @deepseek-ai/dsh web --port <free-port>` as a child process
//!   4. parse the local URL from its output (fallback: poll the port)
//!   5. navigate the main window to that URL
//!   6. kill the child process tree on app exit

mod node;
mod server;
mod autostart;
mod commands;

use std::sync::mpsc::Sender;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

pub struct AppState {
    /// The spawned `npx` child, kept alive for the lifetime of the app.
    pub child: Mutex<Option<std::process::Child>>,
    /// Set once the log pump is running, so `log_line` can also persist
    /// Rust-side `[boot]`/`[node]` steps to `dsh.log`, not just the UI.
    pub log_tx: Mutex<Option<Sender<String>>>,
    /// Node.js bin directory path, set after bootstrap completes.
    pub node_dir: Mutex<Option<std::path::PathBuf>>,
}

#[derive(Clone, serde::Serialize)]
pub struct StatusPayload {
    /// checking | downloading | installing | starting | loading | ready | error
    pub stage: String,
    pub message: String,
    pub detail: Option<String>,
    /// 0.0 - 100.0 (only meaningful while downloading)
    pub progress: Option<f64>,
}

fn emit_status(app: &AppHandle, payload: StatusPayload) {
    let _ = app.emit("dsh-status", payload);
}

/// Push a Rust-side step line into the on-screen log (and it shares the
/// `dsh-log` channel with dsh's own stdout/stderr). Once the log pump is
/// running (see `start_log_pump`), the line is also persisted to `dsh.log`.
pub fn log_line(app: &AppHandle, line: impl Into<String>) {
    let line = line.into();
    let _ = app.emit("dsh-log", line.clone());
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(guard) = state.log_tx.lock() {
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(line);
            }
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(AppState {
                child: Mutex::new(None),
                log_tx: Mutex::new(None),
                node_dir: Mutex::new(None),
            });

            // Create menu
            let menu = tauri::menu::MenuBuilder::new(app)
                .item(
                    &tauri::menu::MenuItemBuilder::with_id("settings", "Tauri 设置")
                        .build(app)?,
                )
                .separator()
                .item(
                    &tauri::menu::MenuItemBuilder::with_id("quit", "退出")
                        .build(app)?,
                )
                .build()?;

            app.set_menu(menu)?;

            app.on_menu_event(move |app, event| {
                match event.id().as_ref() {
                    "settings" => {
                        if let Err(e) = commands::open_settings_window(app) {
                            eprintln!("Failed to open settings window: {e}");
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                }
            });

            // Only register autostart for release builds; `cargo tauri dev`
            // spawns a throwaway executable under target/debug that should
            // never be added to the user's login items.
            if cfg!(debug_assertions) {
                log_line(app.handle(), "[boot] Debug build; skipping autostart registration");
            } else if let Ok(executable) = std::env::current_exe() {
                if let Err(error) = autostart::enable(&executable) {
                    log_line(app.handle(), format!("[boot] Failed to register autostart: {error}"));
                } else {
                    log_line(app.handle(), "[boot] Autostart is enabled");
                }
            } else {
                log_line(
                    app.handle(),
                    "[boot] Current executable could not be located; skipping autostart registration",
                );
            }

            // Keep the UI thread free: all bootstrap work happens off the main thread.
            std::thread::spawn(move || {
                if let Err(e) = bootstrap(&handle) {
                    eprintln!("[deepseek-harness] bootstrap failed: {e}");
                    emit_status(
                        &handle,
                        StatusPayload {
                            stage: "error".into(),
                            message: "启动失败".into(),
                            detail: Some(e),
                            progress: None,
                        },
                    );
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::open_log_dir,
            commands::open_app_data_dir,
            commands::open_workspace_dir,
            commands::set_autostart,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    if let Ok(mut guard) = state.child.lock() {
                        if let Some(child) = guard.take() {
                            server::kill_process_tree(child);
                        }
                    }
                }
            }
        });
}

fn bootstrap(app: &AppHandle) -> Result<(), String> {
    log_line(app, "[boot] Starting DeepSeek Harness");

    // ---- 1. Node.js ----
    emit_status(
        app,
        StatusPayload {
            stage: "checking".into(),
            message: "正在检查 Node.js…".into(),
            detail: None,
            progress: None,
        },
    );
    let local_node_dir: std::path::PathBuf = node::ensure_node(app)?;
    log_line(app, format!("[boot] Using private Node.js: {}", local_node_dir.display()));

    // Store node_dir in AppState for commands to access
    {
        let state = app.state::<AppState>();
        let mut guard = state.node_dir.lock().map_err(|e| e.to_string())?;
        *guard = Some(local_node_dir.clone());
    }

    // ---- 2. Free port ----
    let port = server::free_port()?;
    log_line(app, format!("[boot] Selected port {port}"));

    // ---- 3. Launch dsh web ----
    emit_status(
        app,
        StatusPayload {
            stage: "starting".into(),
            message: "正在启动 DeepSeek Harness 服务…".into(),
            detail: None,
            progress: None,
        },
    );
    let log_path = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("Failed to locate the application log directory: {e}"))?
        .join("dsh.log");
    log_line(app, format!("[boot] Log file: {}", log_path.display()));
    let log_tx = server::start_log_pump(app.clone(), log_path);
    {
        let state = app.state::<AppState>();
        let mut guard = state.log_tx.lock().map_err(|e| e.to_string())?;
        *guard = Some(log_tx.clone());
    }
    log_line(app, format!("[boot] Launch command: npx -y @deepseek-ai/dsh web --port {port} --no-open"));
    let (child, stdout, stderr) = server::spawn_dsh(app, Some(local_node_dir.as_path()), port)?;
    log_line(app, format!("[boot] dsh child process started (pid={})", child.id()));
    {
        let state = app.state::<AppState>();
        let mut guard = state.child.lock().map_err(|e| e.to_string())?;
        *guard = Some(child);
    }

    // ---- 4. Wait for the local URL ----
    emit_status(
        app,
        StatusPayload {
            stage: "loading".into(),
            message: "正在加载界面…".into(),
            detail: None,
            progress: None,
        },
    );
    let url = {
        let state = app.state::<AppState>();
        let mut guard = state.child.lock().map_err(|e| e.to_string())?;
        let child = guard.as_mut().ok_or("The service process is unexpectedly missing")?;
        server::wait_for_url(child, stdout, stderr, port, log_tx)?
    };
    log_line(app, format!("[boot] Service URL: {url}"));

    // ---- 5. Navigate ----
    let _window = app
        .get_webview_window("main")
        .ok_or("The main window does not exist")?;
    let _parsed = url::Url::parse(&url)
        .map_err(|e| format!("Failed to parse service URL {url}: {e}"))?;
    log_line(app, "[boot] Loading the main window");
    _window
        .navigate(_parsed)
        .map_err(|e| format!("Failed to load {url}: {e}"))?;

    emit_status(
        app,
        StatusPayload {
            stage: "ready".into(),
            message: "DeepSeek Harness 已就绪".into(),
            detail: Some(url),
            progress: None,
        },
    );
    Ok(())
}
