//! Tauri commands for the settings window.

use crate::autostart;
use crate::AppState;
use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Serialize)]
pub struct AppInfo {
    tauri_version: String,
    dsh_version: String,
    node_version: String,
    app_data_dir: String,
    log_dir: String,
    workspace_dir: String,
    autostart_enabled: bool,
}

/// Get application version info, paths, and autostart status.
#[tauri::command]
pub fn get_app_info(app: AppHandle) -> Result<AppInfo, String> {
    let tauri_version = env!("CARGO_PKG_VERSION").to_string();

    // dsh version: query the actual `npx @deepseek-ai/dsh --version` output,
    // using the same private Node.js PATH the app launches dsh with.
    let dsh_version = get_dsh_version(&app);

    // Node.js version by running `node --version`
    let node_version = get_node_version(&app)?;

    // App data directory
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {e}"))?
        .display()
        .to_string();

    // Log directory
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("Failed to get log directory: {e}"))?
        .display()
        .to_string();

    // Workspace directory (dsh-workspace under app data)
    let workspace_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {e}"))?
        .join("dsh-workspace")
        .display()
        .to_string();

    // Autostart status: check if the registry entry exists (Windows only for now)
    let autostart_enabled = check_autostart_enabled();

    Ok(AppInfo {
        tauri_version,
        dsh_version,
        node_version,
        app_data_dir,
        log_dir,
        workspace_dir,
        autostart_enabled,
    })
}

/// Get Node.js version by running `node --version`.
fn get_node_version(app: &AppHandle) -> Result<String, String> {
    let node_dir = app
        .state::<AppState>()
        .node_dir
        .lock()
        .map_err(|e| e.to_string())?
        .clone();

    let node_bin = match node_dir {
        Some(dir) => {
            #[cfg(windows)]
            { dir.join("node.exe") }
            #[cfg(not(windows))]
            { dir.join("node") }
        }
        None => return Ok("Not initialized yet".to_string()),
    };

    if !node_bin.exists() {
        return Ok("Not found".to_string());
    }

    let output = std::process::Command::new(&node_bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("Failed to run node: {e}"))?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    } else {
        Ok("Unknown".to_string())
    }
}

/// Get the actual dsh version by running `npx -y @deepseek-ai/dsh --version`,
/// using the private Node.js install's `npx` (same PATH setup as `spawn_dsh`).
/// Network/timeout failures degrade to a placeholder rather than failing the
/// whole settings load.
fn get_dsh_version(app: &AppHandle) -> String {
    const PACKAGE: &str = "@deepseek-ai/dsh";
    const QUERY_TIMEOUT_SECS: u64 = 20;

    let node_dir = app
        .state::<AppState>()
        .node_dir
        .lock()
        .ok()
        .and_then(|guard| guard.clone());

    let Some(node_dir) = node_dir else {
        return "Not initialized yet".to_string();
    };

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("cmd");
            c.arg("/C").arg("npx").arg("-y").arg(PACKAGE).arg("--version");
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = std::process::Command::new("npx");
            c.arg("-y").arg(PACKAGE).arg("--version");
            c
        };

        let mut paths = vec![node_dir];
        if let Some(old) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&old));
        }
        if let Ok(new_path) = std::env::join_paths(paths) {
            cmd.env("PATH", new_path);
        }

        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let result = cmd.output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(QUERY_TIMEOUT_SECS)) {
        Ok(Ok(output)) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if version.is_empty() {
                "Unknown".to_string()
            } else {
                version
            }
        }
        Ok(Ok(_)) => "Unknown".to_string(),
        Ok(Err(_)) => "Query failed".to_string(),
        Err(_) => "Query timed out".to_string(),
    }
}

/// Open the log directory in the system file explorer.
#[tauri::command]
pub fn open_log_dir(app: AppHandle) -> Result<(), String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("Failed to get log directory: {e}"))?;

    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(log_dir)
            .spawn()
            .map_err(|e| format!("Failed to open explorer: {e}"))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(log_dir)
            .spawn()
            .map_err(|e| format!("Failed to open Finder: {e}"))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(log_dir)
            .spawn()
            .map_err(|e| format!("Failed to open file manager: {e}"))?;
    }

    Ok(())
}

/// Open the app data directory in the system file explorer.
#[tauri::command]
pub fn open_app_data_dir(app: AppHandle) -> Result<(), String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;

    open_directory(&app_data_dir)
}

/// Open the workspace directory in the system file explorer.
#[tauri::command]
pub fn open_workspace_dir(app: AppHandle) -> Result<(), String> {
    let workspace_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {e}"))?
        .join("dsh-workspace");

    open_directory(&workspace_dir)
}

/// Helper function to open a directory in the system file explorer.
fn open_directory(path: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to open explorer: {e}"))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to open Finder: {e}"))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to open file manager: {e}"))?;
    }

    Ok(())
}

/// Enable or disable autostart.
#[tauri::command]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|e| format!("Failed to get current executable: {e}"))?;

    if enabled {
        autostart::enable(&executable)?;
    } else {
        autostart::disable(&executable)?;
    }

    Ok(())
}

/// Check if autostart is currently enabled by querying the registry (Windows only).
fn check_autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use std::process::Command;

        let output = Command::new("reg.exe")
            .args([
                "QUERY",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "DeepSeek Harness",
            ])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();

        match output {
            Ok(out) => out.status.success(),
            Err(_) => false,
        }
    }

    #[cfg(not(windows))]
    {
        // macOS/Linux: check if the plist/desktop file exists
        false // Placeholder for now
    }
}

/// Open the settings window (create if not exists, focus if already open).
pub fn open_settings_window(app: &AppHandle) -> Result<(), String> {
    const SETTINGS_LABEL: &str = "settings";

    // Check if settings window already exists
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        window
            .set_focus()
            .map_err(|e| format!("Failed to focus settings window: {e}"))?;
        return Ok(());
    }

    // Create new settings window (without menu bar)
    let empty_menu = tauri::menu::MenuBuilder::new(app).build()
        .map_err(|e| format!("Failed to create empty menu: {e}"))?;

    WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::App("settings.html".into()))
        .title("设置")
        .inner_size(900.0, 600.0)
        .resizable(false)
        .center()
        .menu(empty_menu)
        .minimizable(false)
        .maximizable(false)
        .build()
        .map_err(|e| format!("Failed to create settings window: {e}"))?;

    Ok(())
}
