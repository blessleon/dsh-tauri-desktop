//! Launching `npx -y @deepseek-ai/dsh web` and resolving the local URL.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use tauri::{AppHandle, Emitter, Manager};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const PACKAGE: &str = "@deepseek-ai/dsh";
/// First run can take minutes (profile init + first install). Give it room.
const START_TIMEOUT_SECS: u64 = 300;
const PORT_POLL_SECS: u64 = 120;
/// How long a local install is trusted before we ask npm to re-resolve
/// "latest" again. Balances startup speed against staying reasonably
/// current. Users can force a refresh from Settings.
const INSTALL_FRESHNESS_SECS: u64 = 24 * 60 * 60;

pub fn free_port() -> Result<u16, String> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Failed to allocate a port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to read the allocated port: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

fn workspace_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let workspace = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to locate the application data directory: {e}"))?
        .join("dsh-workspace");
    std::fs::create_dir_all(&workspace)
        .map_err(|e| format!("Failed to create workspace directory {workspace:?}: {e}"))?;
    Ok(workspace)
}

/// Path to the marker file recording when dsh was last installed/refreshed
/// locally, so we don't ask npm to re-resolve "latest" on every launch.
fn install_marker_path(workspace: &Path) -> PathBuf {
    workspace.join(".dsh-install-stamp")
}

fn local_dsh_bin(workspace: &Path) -> PathBuf {
    let bin_dir = workspace.join("node_modules").join(".bin");
    if cfg!(windows) {
        bin_dir.join("dsh.cmd")
    } else {
        bin_dir.join("dsh")
    }
}

fn is_install_fresh(workspace: &Path) -> bool {
    let marker = install_marker_path(workspace);
    let Ok(metadata) = std::fs::metadata(&marker) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    match modified.elapsed() {
        Ok(age) => age.as_secs() < INSTALL_FRESHNESS_SECS,
        Err(_) => false, // clock skew; treat as stale rather than trusting it
    }
}

fn touch_install_marker(workspace: &Path) {
    let _ = std::fs::write(install_marker_path(workspace), b"");
}

fn env_with_node_first(cmd: &mut Command, node_bin_dir: Option<&Path>) {
    if let Some(dir) = node_bin_dir {
        let mut paths = vec![dir.to_path_buf()];
        if let Some(old) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&old));
        }
        if let Ok(new_path) = std::env::join_paths(paths) {
            cmd.env("PATH", new_path);
        }
    }
}

/// Ensure `@deepseek-ai/dsh` is installed locally in the workspace via
/// `npm install`, so subsequent launches can invoke its bin shim directly
/// instead of going through `npx -y` (which re-resolves the package against
/// the registry on every single run, even when it is already cached).
///
/// Skipped entirely when a local install already exists and is younger than
/// `INSTALL_FRESHNESS_SECS`. Any failure here is non-fatal: the caller falls
/// back to `npx -y`.
fn ensure_local_dsh_install(
    app: &AppHandle,
    workspace: &Path,
    node_bin_dir: Option<&Path>,
) -> bool {
    let bin = local_dsh_bin(workspace);
    if bin.is_file() && is_install_fresh(workspace) {
        crate::log_line(app, "[dsh] Local dsh install is fresh; skipping npm install");
        return true;
    }

    crate::log_line(app, format!("[dsh] Installing {PACKAGE} locally (one-time/refresh)…"));

    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg("npm").arg("install").arg(format!("{PACKAGE}@latest")).arg("--no-audit").arg("--no-fund");
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("npm");
        c.arg("install").arg(format!("{PACKAGE}@latest")).arg("--no-audit").arg("--no-fund");
        c
    };

    env_with_node_first(&mut cmd, node_bin_dir);
    cmd.current_dir(workspace).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.output() {
        Ok(output) if output.status.success() && bin.is_file() => {
            touch_install_marker(workspace);
            crate::log_line(app, "[dsh] Local dsh install ready");
            true
        }
        Ok(output) => {
            crate::log_line(
                app,
                format!(
                    "[dsh] npm install failed (status: {}); falling back to npx -y",
                    output.status
                ),
            );
            false
        }
        Err(e) => {
            crate::log_line(app, format!("[dsh] npm install could not be started ({e}); falling back to npx -y"));
            false
        }
    }
}

/// Spawn dsh's `web` command.
///
/// Prefers a locally-installed copy of `@deepseek-ai/dsh` (installed once via
/// `npm install` into the workspace, then reused directly) so startup avoids
/// `npx -y`'s per-launch registry resolution. Falls back to `npx -y` when a
/// local install cannot be prepared.
///
/// When `node_bin_dir` is `Some` (a private Node install), its directory is
/// prepended to PATH so the bundled Node/npm/npx is used. The child's stdout
/// and stderr are returned for URL parsing.
pub fn spawn_dsh(
    app: &AppHandle,
    node_bin_dir: Option<&Path>,
    port: u16,
) -> Result<(Child, BufReader<ChildStdout>, BufReader<ChildStderr>), String> {
    let workspace = workspace_dir(app)?;

    let use_local = ensure_local_dsh_install(app, &workspace, node_bin_dir);

    let mut cmd = if use_local {
        let bin = local_dsh_bin(&workspace);
        crate::log_line(app, format!("[boot] Launching local install: {}", bin.display()));
        #[cfg(windows)]
        {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&bin).arg("web").arg("--no-open").arg("--port").arg(port.to_string());
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new(&bin);
            c.arg("web").arg("--no-open").arg("--port").arg(port.to_string());
            c
        }
    } else {
        crate::log_line(app, "[boot] Launching via npx -y (fallback)");
        #[cfg(windows)]
        {
            let mut c = Command::new("cmd");
            c.arg("/C")
                .arg("npx")
                .arg("-y")
                .arg(PACKAGE)
                .arg("web")
                .arg("--no-open")
                .arg("--port")
                .arg(port.to_string());
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new("npx");
            c.arg("-y")
                .arg(PACKAGE)
                .arg("web")
                .arg("--no-open")
                .arg("--port")
                .arg(port.to_string());
            c
        }
    };

    env_with_node_first(&mut cmd, node_bin_dir);

    cmd.current_dir(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to launch dsh ({PACKAGE}): {e}"))?;
    let stdout = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture service stdout".to_string())?,
    );
    let stderr = BufReader::new(
        child
            .stderr
            .take()
            .ok_or_else(|| "Failed to capture service stderr".to_string())?,
    );
    Ok((child, stdout, stderr))
}

/// Wait for the child process to print its local URL.
///
/// stdout + stderr are streamed to the UI (and a log file) via `log_tx`,
/// so the user can see what dsh is doing. If no URL line appears in time,
/// fall back to polling the port directly. If the child process exits before
/// either happens (e.g. `npx` failed to install the package), this returns an
/// error immediately instead of waiting out the full timeout.
pub fn wait_for_url(
    child: &mut Child,
    stdout: BufReader<ChildStdout>,
    stderr: BufReader<ChildStderr>,
    port: u16,
    log_tx: mpsc::Sender<String>,
) -> Result<String, String> {
    let (url_tx, url_rx) = mpsc::channel::<String>();
    spawn_reader(stdout, url_tx.clone(), log_tx.clone());
    spawn_reader(stderr, url_tx.clone(), log_tx.clone());

    const POLL_INTERVAL: Duration = Duration::from_millis(300);

    let url = wait_with_child_check(child, &url_rx, Duration::from_secs(START_TIMEOUT_SECS), POLL_INTERVAL)?;
    if let Some(url) = url {
        return Ok(url);
    }

    if poll_port_with_child_check(child, port, PORT_POLL_SECS)? {
        Ok(format!("http://127.0.0.1:{port}"))
    } else {
        Err(format!(
            "Service startup timed out; no local URL was detected within {} seconds",
            START_TIMEOUT_SECS
        ))
    }
}

/// Wait up to `timeout` for a URL on `url_rx`, checking every `poll_interval`
/// whether `child` has exited early. Returns `Ok(None)` on a plain timeout
/// (caller should fall back to port polling), or `Err` if the child died.
fn wait_with_child_check(
    child: &mut Child,
    url_rx: &mpsc::Receiver<String>,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<Option<String>, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match url_rx.recv_timeout(poll_interval) {
            Ok(url) => return Ok(Some(url)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(status) = child_exit_status(child)? {
                    return Err(format!(
                        "Service process exited early before printing a URL (status: {status})"
                    ));
                }
                if Instant::now() >= deadline {
                    return Ok(None);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
        }
    }
}

/// Like `poll_port`, but also fails fast if `child` exits early.
fn poll_port_with_child_check(child: &mut Child, port: u16, timeout_secs: u64) -> Result<bool, String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while Instant::now() < deadline {
        if let Some(status) = child_exit_status(child)? {
            return Err(format!("Service process exited early (status: {status})"));
        }
        let alive = ureq::get(&format!("http://127.0.0.1:{port}/"))
            .timeout(Duration::from_secs(2))
            .call()
            .map(|r| r.status() < 500)
            .unwrap_or(false);
        if alive {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok(false)
}

fn child_exit_status(child: &mut Child) -> Result<Option<std::process::ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|e| format!("Failed to check the service process status: {e}"))
}

/// Spawn a thread that forwards every log line to the UI event `dsh-log`
/// and appends it to `log_path`. Returns the channel sender to feed lines into.
pub fn start_log_pump(app: AppHandle, log_path: PathBuf) -> mpsc::Sender<String> {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut writer: Option<std::io::BufWriter<std::fs::File>> =
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .ok()
                .map(std::io::BufWriter::new);
        while let Ok(line) = rx.recv() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            let _ = app.emit("dsh-log", trimmed);
            if let Some(w) = writer.as_mut() {
                let _ = w.write_all(trimmed.as_bytes());
                let _ = w.write_all(b"\n");
            }
        }
        if let Some(mut w) = writer {
            let _ = w.flush();
        }
    });
    tx
}

fn spawn_reader<R: Read + Send + 'static>(
    stream: R,
    url_tx: mpsc::Sender<String>,
    log_tx: mpsc::Sender<String>,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf: Vec<u8> = Vec::new();
        let mut got_url = false;
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf).to_string();
                    let _ = log_tx.send(line.clone());
                    if !got_url {
                        if let Some(url) = extract_url(&line) {
                            let _ = url_tx.send(url);
                            got_url = true;
                        }
                    }
                }
            }
        }
    });
}

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):\d+[^\s"'<>]*"#)
            .expect("valid URL regex")
    })
}

fn ansi_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").expect("valid ANSI regex"))
}

fn extract_url(line: &str) -> Option<String> {
    let clean = strip_ansi(line);
    // Keep the full URL including any query string (e.g. `?token=...`).
    let m = url_regex().find(&clean)?;
    let url = m.as_str();
    Some(if url.starts_with("http://0.0.0.0") {
        url.replacen("http://0.0.0.0", "http://127.0.0.1", 1)
    } else {
        url.to_string()
    })
}

fn strip_ansi(s: &str) -> String {
    ansi_regex().replace_all(s, "").into_owned()
}

/// Kill the whole process tree (npx -> node -> dsh workers).
pub fn kill_process_tree(mut child: Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut kill = Command::new("taskkill");
        kill.args(["/PID", &child.id().to_string(), "/T", "/F"]);
        kill.creation_flags(CREATE_NO_WINDOW);
        let _ = kill.status();
        let _ = child.wait();
    }
    #[cfg(not(windows))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}
