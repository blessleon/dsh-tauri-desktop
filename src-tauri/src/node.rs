//! Node.js detection, download and install.
//!
//! Strategy: use the app-private Node.js 24 install first; if it is missing,
//! download the latest official Node.js 24.x release and unpack it inside
//! the app-data directory (no admin rights, no system-wide changes). The
//! returned path is the directory containing the `node` executable (and `npx`).

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::{emit_status, log_line, StatusPayload};

const NODE_MAJOR: &str = "24";

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Always installs/reuses an app-private Node.js runtime and returns its bin
/// directory. A system Node.js installation is never used: it may be a
/// different major version and could break dsh's dependency tree.
pub fn ensure_node(app: &AppHandle) -> Result<PathBuf, String> {
    let install_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to locate the application data directory: {e}"))?
        .join("nodejs");
    if let Some(dir) = find_private_node(&install_dir) {
        log_line(
            app,
            format!("[node] Reusing private Node.js {NODE_MAJOR}: {}", dir.display()),
        );
        return Ok(dir);
    }

    log_line(
        app,
        format!("[node] Private Node.js {NODE_MAJOR} was not found; checking downloaded files"),
    );
    let dir = install_node(app, &install_dir)?;
    log_line(app, format!("[node] Private Node.js is ready: {}", dir.display()));
    Ok(dir)
}

fn runs_ok(node_exe: &Path) -> bool {
    let mut cmd = Command::new(node_exe);
    cmd.arg("--version");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Download the latest Node.js 24.x release and unpack it under app-data/nodejs.
fn install_node(app: &AppHandle, install_dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&install_dir)
        .map_err(|e| format!("Failed to create directory {install_dir:?}: {e}"))?;

    emit_status(
        app,
        StatusPayload {
            stage: "checking".into(),
            message: format!("正在检查本地 Node.js {NODE_MAJOR} 文件…"),
            detail: None,
            progress: None,
        },
    );
    if let Some(dir) = find_private_node(install_dir) {
        log_line(
            app,
            format!("[node] Found existing Node.js {NODE_MAJOR}: {}", dir.display()),
        );
        return Ok(dir);
    }
    // Reuse an archive left by a previous download before making a network request.
    if let Some((archive_path, version, kind)) = find_downloaded_archive(install_dir) {
        log_line(
            app,
            format!(
                "[node] Found downloaded Node.js archive {}; attempting to reuse it",
                archive_path.display()
            ),
        );
        if let Ok(dir) = extract_and_locate(&archive_path, install_dir, &version, kind) {
            log_line(app, "[node] Reused the existing Node.js archive; download skipped");
            return Ok(dir);
        }
        log_line(app, "[node] Existing Node.js archive is invalid; downloading a fresh copy");
    }

    let version = fetch_node24_version()?;
    log_line(app, format!("[node] Latest Node.js {NODE_MAJOR} version: {version}"));

    let (file_name, kind) = dist_file(&version)?;
    let url = format!("https://nodejs.org/dist/{version}/{file_name}");
    let archive_path = install_dir.join(&file_name);
    let temporary_archive_path = install_dir.join(format!("{file_name}.part"));
    log_line(app, format!("[node] Download URL: {url}"));

    emit_status(
        app,
        StatusPayload {
            stage: "downloading".into(),
            message: format!("正在下载 Node.js {version}…"),
            detail: None,
            progress: Some(0.0),
        },
    );
    download(app, &url, &temporary_archive_path)?;
    if archive_path.exists() {
        std::fs::remove_file(&archive_path).map_err(|e| {
            format!("Failed to replace existing Node.js archive {archive_path:?}: {e}")
        })?;
    }
    std::fs::rename(&temporary_archive_path, &archive_path).map_err(|e| {
        format!(
            "Failed to finalize Node.js archive from {temporary_archive_path:?} to {archive_path:?}: {e}"
        )
    })?;

    emit_status(
        app,
        StatusPayload {
            stage: "installing".into(),
            message: format!("正在安装 Node.js {version}…"),
            detail: None,
            progress: None,
        },
    );
    let bin_dir = extract_and_locate(&archive_path, install_dir, &version, kind)?;
    log_line(app, "[node] Node.js archive extracted successfully");
    Ok(bin_dir)
}

fn extract_and_locate(
    archive_path: &Path,
    install_dir: &Path,
    version: &str,
    kind: &'static str,
) -> Result<PathBuf, String> {
    match kind {
        "zip" => extract_zip(archive_path, install_dir)?,
        _ => extract_tar_gz(archive_path, install_dir)?,
    }
    locate_bin_dir(install_dir, version)
}

fn fetch_node24_version() -> Result<String, String> {
    let resp = ureq::get("https://nodejs.org/dist/index.json")
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| format!("Failed to fetch the Node.js version list: {e}"))?;
    let body = resp
        .into_string()
        .map_err(|e| format!("Failed to read the Node.js version list: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse the Node.js version list: {e}"))?;
    json.as_array()
        .and_then(|versions| {
            versions.iter().find_map(|entry| {
                let version = entry.get("version")?.as_str()?;
                version
                    .strip_prefix(&format!("v{NODE_MAJOR}."))
                    .map(|_| version.to_string())
            })
        })
        .ok_or_else(|| format!("Failed to resolve a Node.js {NODE_MAJOR} version from the official source"))
}

fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "x86",
        "aarch64" => "arm64",
        "arm" => "armv7l",
        "powerpc64" => "ppc64le",
        "s390x" => "s390x",
        other => other,
    }
}

fn dist_file(version: &str) -> Result<(String, &'static str), String> {
    match std::env::consts::OS {
        "windows" => Ok((
            format!("node-{version}-win-{}.zip", node_arch()),
            "zip",
        )),
        "macos" => Ok((
            format!("node-{version}-darwin-{}.tar.gz", node_arch()),
            "tar.gz",
        )),
        "linux" => Ok((
            format!("node-{version}-linux-{}.tar.gz", node_arch()),
            "tar.gz",
        )),
        other => Err(format!("Unsupported operating system: {other}")),
    }
}

fn download(app: &AppHandle, url: &str, dest: &Path) -> Result<(), String> {
    let resp = ureq::get(url)
        .timeout(Duration::from_secs(300))
        .call()
        .map_err(|e| format!("Failed to download Node.js: {e}"))?;
    let total: Option<u64> = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok());

    let mut reader = resp.into_reader();
    let mut file = File::create(dest).map_err(|e| format!("Failed to write {dest:?}: {e}"))?;
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("Failed to write file: {e}"))?;
        written += n as u64;
        if let Some(t) = total {
            if t > 0 {
                emit_status(
                    app,
                    StatusPayload {
                        stage: "downloading".into(),
                        message: format!("正在下载 Node.js… ({:.1} MB / {:.1} MB)", written as f64 / 1e6, t as f64 / 1e6),
                        detail: None,
                        progress: Some(written as f64 / t as f64 * 100.0),
                    },
                );
            }
        }
    }
    file.flush().map_err(|e| format!("Failed to write file: {e}"))?;
    Ok(())
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| format!("Failed to open {archive:?}: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("Failed to extract archive: {e}"))?;
    zip.extract(dest).map_err(|e| format!("Failed to extract archive: {e}"))?;
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| format!("Failed to open {archive:?}: {e}"))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(dest).map_err(|e| format!("Failed to extract archive: {e}"))?;
    Ok(())
}

fn locate_bin_dir(install_dir: &Path, version: &str) -> Result<PathBuf, String> {
    let prefix = format!("node-{version}-");
    let extracted = find_dir_starting_with(install_dir, &prefix)
        .ok_or_else(|| format!("Extracted directory node-{version}-* was not found"))?;
    let bin = if cfg!(windows) {
        extracted.join("node.exe")
    } else {
        extracted.join("bin/node")
    };
    if bin.is_file() && runs_ok(&bin) {
        Ok(if cfg!(windows) {
            extracted
        } else {
            extracted.join("bin")
        })
    } else {
        Err(format!("Node.js executable is unavailable: {bin:?}"))
    }
}

fn find_private_node(install_dir: &Path) -> Option<PathBuf> {
    // Release directories are named `node-v24.4.1-<os>-<arch>` (a dot follows
    // the major version, not a dash), so the prefix must stop right after
    // "v{NODE_MAJOR}" and let `is_node_major_dir` verify what follows is
    // either a dot (a full version) or nothing else that could collide with
    // a different major version (e.g. "v240").
    let prefix = format!("node-v{NODE_MAJOR}");
    let extracted = find_dir_matching(install_dir, |name| is_node_major_dir(name, &prefix))?;
    let bin = if cfg!(windows) {
        extracted.join("node.exe")
    } else {
        extracted.join("bin/node")
    };
    if bin.is_file() && runs_ok(&bin) {
        Some(if cfg!(windows) {
            extracted
        } else {
            extracted.join("bin")
        })
    } else {
        None
    }
}

fn find_downloaded_archive(install_dir: &Path) -> Option<(PathBuf, String, &'static str)> {
    let suffix = match std::env::consts::OS {
        "windows" => format!("-win-{}.zip", node_arch()),
        "macos" => format!("-darwin-{}.tar.gz", node_arch()),
        "linux" => format!("-linux-{}.tar.gz", node_arch()),
        _ => return None,
    };

    let entries = std::fs::read_dir(install_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(version) = file_name
            .strip_prefix("node-")
            .and_then(|rest| rest.strip_suffix(&suffix))
            .map(|v| v.to_string())
        else {
            continue;
        };
        if version.starts_with(&format!("v{NODE_MAJOR}.")) {
            let kind = if suffix.ends_with(".zip") {
                "zip"
            } else {
                "tar.gz"
            };
            return Some((path, version, kind));
        }
    }
    None
}

fn find_dir_starting_with(dir: &Path, prefix: &str) -> Option<PathBuf> {
    find_dir_matching(dir, |name| name.starts_with(prefix))
}

fn find_dir_matching(dir: &Path, matches: impl Fn(&str) -> bool) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if matches(name) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// True if `name` is a Node.js release directory for the given `major_prefix`
/// (e.g. `major_prefix = "node-v24"` matches `node-v24.4.1-win-x64` but not
/// `node-v240.0.0-win-x64` or `node-v2.4.1-win-x64`).
fn is_node_major_dir(name: &str, major_prefix: &str) -> bool {
    name.strip_prefix(major_prefix)
        .and_then(|rest| rest.chars().next())
        .map(|c| c == '.')
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::find_downloaded_archive;
    use std::fs;

    #[test]
    fn ignores_archives_from_other_node_major_versions() {
        let temp_dir = std::env::temp_dir().join(format!(
            "deepseek-harness-node-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_dir).expect("create test directory");
        let archive_name = match std::env::consts::OS {
            "windows" => "node-v22.0.0-win-x64.zip",
            "macos" => "node-v22.0.0-darwin-x64.tar.gz",
            "linux" => "node-v22.0.0-linux-x64.tar.gz",
            _ => {
                fs::remove_dir_all(temp_dir).expect("remove test directory");
                return;
            }
        };
        fs::write(temp_dir.join(archive_name), b"archive")
            .expect("write test archive");

        assert!(find_downloaded_archive(&temp_dir).is_none());

        fs::remove_dir_all(temp_dir).expect("remove test directory");
    }

    /// Regression test: a non-matching file in the install dir (e.g. a
    /// leftover `.part` from an interrupted download, or an unrelated file)
    /// must not stop the scan before it reaches a valid archive later in the
    /// directory listing.
    #[test]
    fn finds_matching_archive_even_when_preceded_by_unrelated_files() {
        let temp_dir = std::env::temp_dir().join(format!(
            "deepseek-harness-node-test-skip-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_dir).expect("create test directory");
        let (archive_name, expected_version) = match std::env::consts::OS {
            "windows" => ("node-v24.4.1-win-x64.zip", "v24.4.1"),
            "macos" => ("node-v24.4.1-darwin-x64.tar.gz", "v24.4.1"),
            "linux" => ("node-v24.4.1-linux-x64.tar.gz", "v24.4.1"),
            _ => {
                fs::remove_dir_all(temp_dir).expect("remove test directory");
                return;
            }
        };

        // Unrelated / in-progress files that must not derail the scan.
        fs::write(temp_dir.join("node-v24.4.1-win-x64.zip.part"), b"partial")
            .expect("write partial download");
        fs::write(temp_dir.join("some-other-file.txt"), b"noop").expect("write unrelated file");
        fs::write(temp_dir.join(archive_name), b"archive").expect("write test archive");

        let found = find_downloaded_archive(&temp_dir).expect("archive should be found");
        assert_eq!(found.1, expected_version);

        fs::remove_dir_all(temp_dir).expect("remove test directory");
    }
}
