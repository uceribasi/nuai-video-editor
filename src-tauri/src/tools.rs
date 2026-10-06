//! Locating external binaries (ffmpeg / ffprobe) and spawning them.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Folders where CLIs installed by package managers usually live. GUI apps on macOS
/// don't inherit the shell PATH, so these are probed explicitly.
fn user_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        for d in [
            ".local/bin",
            ".cargo/bin",
            ".bun/bin",
            ".npm-global/bin",
            ".volta/bin",
            "AppData/Roaming/npm",
        ] {
            dirs.push(home.join(d));
        }
        // nvm installs each Node version separately; newest first.
        if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            let mut versions: Vec<PathBuf> =
                entries.flatten().map(|e| e.path().join("bin")).collect();
            versions.sort();
            dirs.extend(versions.into_iter().rev());
        }
    }
    dirs
}

/// Candidate directories, in priority order.
fn search_dirs(custom_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = custom_dir {
        dirs.push(d.to_path_buf());
    }
    // Bundled sidecars live next to the app executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    // GUI apps on macOS don't inherit the shell PATH, so probe the usual spots.
    for d in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/opt/local/bin",
        "/snap/bin",
        "C:\\ffmpeg\\bin",
        "C:\\Program Files\\ffmpeg\\bin",
    ] {
        dirs.push(PathBuf::from(d));
    }
    dirs.extend(user_bin_dirs());
    dirs
}

/// Finds any CLI by name; `custom` may be the binary itself or its folder.
pub fn find_program(name: &str, custom: Option<&str>) -> Option<PathBuf> {
    let custom = custom
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    if let Some(p) = custom.as_ref().filter(|p| p.is_file()) {
        return Some(p.clone());
    }
    find(name, custom.as_deref())
}

/// PATH for child processes: the program's own folder and the usual install folders
/// first, so script-based CLIs (e.g. npm installs needing `node`) still start.
pub fn child_path(program: &Path) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = program
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    dirs.extend(user_bin_dirs());
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Every copy of a program on the search path, without duplicates.
pub fn find_all(name: &str) -> Vec<PathBuf> {
    let file = exe_name(name);
    let mut seen = std::collections::HashSet::new();
    search_dirs(None)
        .into_iter()
        .map(|d| d.join(&file))
        .filter(|p| p.is_file())
        .filter(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())))
        .collect()
}

fn find(name: &str, custom_dir: Option<&Path>) -> Option<PathBuf> {
    let file = exe_name(name);
    search_dirs(custom_dir)
        .into_iter()
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

/// `custom` may point to the ffmpeg binary itself or to the directory holding it.
pub fn locate(custom: Option<&str>) -> Result<Tools> {
    let custom_dir = custom.filter(|s| !s.trim().is_empty()).map(|s| {
        let p = PathBuf::from(s.trim());
        if p.is_file() {
            p.parent().map(Path::to_path_buf).unwrap_or(p)
        } else {
            p
        }
    });
    let ffmpeg = find("ffmpeg", custom_dir.as_deref())
        .ok_or_else(|| anyhow!("ffmpeg not found. Install it or set its location in Settings."))?;
    let ffprobe = find("ffprobe", custom_dir.as_deref())
        .ok_or_else(|| anyhow!("ffprobe not found. Install it or set its location in Settings."))?;
    Ok(Tools { ffmpeg, ffprobe })
}

/// A tokio command that never opens a console window and dies with its handle.
pub fn command(program: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.kill_on_drop(true);
    cmd.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
