//! Register the current executable for the current user's login.

use std::path::Path;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(any(target_os = "macos", target_os = "linux"))]
const APP_ID: &str = "com.dsh.tauri-desktop";
#[cfg(any(windows, target_os = "linux"))]
const APP_NAME: &str = "DeepSeek Harness";

pub fn enable(executable: &Path) -> Result<(), String> {
    if cfg!(windows) {
        return enable_windows(executable);
    }
    if cfg!(target_os = "macos") {
        return enable_macos(executable);
    }
    if cfg!(target_os = "linux") {
        return enable_linux(executable);
    }
    Err("Autostart is not supported on the current operating system".into())
}

pub fn disable(executable: &Path) -> Result<(), String> {
    if cfg!(windows) {
        return disable_windows(executable);
    }
    if cfg!(target_os = "macos") {
        return disable_macos(executable);
    }
    if cfg!(target_os = "linux") {
        return disable_linux(executable);
    }
    Err("Autostart is not supported on the current operating system".into())
}

#[cfg(windows)]
fn enable_windows(executable: &Path) -> Result<(), String> {
    let command = format!("\"{}\"", executable.display());
    let mut reg = Command::new("reg.exe");
    reg.args([
        "ADD",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
        "/v",
        APP_NAME,
        "/t",
        "REG_SZ",
        "/d",
        &command,
        "/f",
    ]);
    reg.creation_flags(CREATE_NO_WINDOW);
    let status = reg
        .status()
        .map_err(|e| format!("Failed to write the Windows autostart entry: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Failed to write the Windows autostart entry: {status}"))
    }
}

#[cfg(windows)]
fn disable_windows(_executable: &Path) -> Result<(), String> {
    let mut reg = Command::new("reg.exe");
    reg.args([
        "DELETE",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
        "/v",
        APP_NAME,
        "/f",
    ]);
    reg.creation_flags(CREATE_NO_WINDOW);
    let status = reg
        .status()
        .map_err(|e| format!("Failed to delete the Windows autostart entry: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Failed to delete the Windows autostart entry: {status}"))
    }
}

#[cfg(not(windows))]
fn enable_windows(_executable: &Path) -> Result<(), String> {
    Err("Windows autostart is not supported on the current operating system".into())
}

#[cfg(not(windows))]
fn disable_windows(_executable: &Path) -> Result<(), String> {
    Err("Windows autostart is not supported on the current operating system".into())
}

#[cfg(target_os = "macos")]
fn enable_macos(executable: &Path) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("Failed to locate the user home directory")?;
    let dir = Path::new(&home).join("Library/LaunchAgents");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create the LaunchAgents directory: {e}"))?;
    let path = dir.join(format!("{APP_ID}.plist"));
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{APP_ID}</string>
  <key>ProgramArguments</key>
  <array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
        xml_escape(&executable.to_string_lossy())
    );
    std::fs::write(path, content)
        .map_err(|e| format!("Failed to write the macOS autostart entry: {e}"))
}

#[cfg(target_os = "macos")]
fn disable_macos(_executable: &Path) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("Failed to locate the user home directory")?;
    let path = Path::new(&home)
        .join("Library/LaunchAgents")
        .join(format!("{APP_ID}.plist"));

    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete the macOS autostart entry: {e}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn enable_macos(_executable: &Path) -> Result<(), String> {
    Err("macOS autostart is not supported on the current operating system".into())
}

#[cfg(not(target_os = "macos"))]
fn disable_macos(_executable: &Path) -> Result<(), String> {
    Err("macOS autostart is not supported on the current operating system".into())
}

#[cfg(target_os = "linux")]
fn enable_linux(executable: &Path) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("Failed to locate the user home directory")?;
    let dir = Path::new(&home).join(".config/autostart");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create the autostart directory: {e}"))?;
    let path = dir.join(format!("{APP_ID}.desktop"));
    let content = format!(
        "[Desktop Entry]\nType=Application\nName={APP_NAME}\nExec=\"{}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        executable.display()
    );
    std::fs::write(path, content)
        .map_err(|e| format!("Failed to write the Linux autostart entry: {e}"))
}

#[cfg(target_os = "linux")]
fn disable_linux(_executable: &Path) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("Failed to locate the user home directory")?;
    let path = Path::new(&home)
        .join(".config/autostart")
        .join(format!("{APP_ID}.desktop"));

    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete the Linux autostart entry: {e}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn enable_linux(_executable: &Path) -> Result<(), String> {
    Err("Linux autostart is not supported on the current operating system".into())
}

#[cfg(not(target_os = "linux"))]
fn disable_linux(_executable: &Path) -> Result<(), String> {
    Err("Linux autostart is not supported on the current operating system".into())
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
