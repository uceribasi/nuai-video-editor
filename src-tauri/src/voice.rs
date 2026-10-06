//! Optional local voice engine: voice cloning (OmniVoice) and vocal separation (Demucs).
//!
//! Installed with one click into the app data folder: a private Python made by uv,
//! pinned packages, and model weights in a private Hugging Face cache (so both models
//! load offline). The app then talks to a long-lived worker (`resources/voice/nuai_voice.py`)
//! over JSON lines, keeping models loaded between requests.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::speech::Word;
use crate::task::{Cancel, Progress};
use crate::tools;

const WORKER: &str = include_str!("../resources/voice/nuai_voice.py");
const REQUIREMENTS: &str = include_str!("../resources/voice/requirements.txt");
const CONSTRAINTS: &str = include_str!("../resources/voice/constraints.txt");
const MANIFEST: &str = include_str!("../resources/voice/manifest.json");
const PYTHON_VERSION: &str = "3.11";
/// Rough download size of Python + packages, for the UI.
const RUNTIME_MB: u64 = 1200;

#[derive(Deserialize)]
struct Manifest {
    repos: Vec<RepoSpec>,
}

#[derive(Deserialize)]
struct RepoSpec {
    repo: String,
    revision: String,
    files: Vec<FileSpec>,
}

#[derive(Deserialize)]
struct FileSpec {
    path: String,
    size: u64,
    sha256: Option<String>,
}

fn manifest() -> Manifest {
    serde_json::from_str(MANIFEST).expect("bundled voice manifest is valid")
}

pub const OMNIVOICE_REPO: &str = "k2-fsa/OmniVoice";

pub struct VoicePaths {
    pub root: PathBuf,
}

impl VoicePaths {
    pub fn new(app_data: &Path) -> Self {
        VoicePaths {
            root: app_data.join("voice"),
        }
    }

    fn env_dir(&self) -> PathBuf {
        self.root.join("env")
    }

    pub fn python(&self) -> PathBuf {
        if cfg!(windows) {
            self.env_dir().join("Scripts").join("python.exe")
        } else {
            self.env_dir().join("bin").join("python3")
        }
    }

    fn hf_home(&self) -> PathBuf {
        self.root.join("models").join("hf")
    }

    fn repo_dir(&self, repo: &str) -> PathBuf {
        self.hf_home()
            .join("hub")
            .join(format!("models--{}", repo.replace('/', "--")))
    }

    pub fn snapshot(&self, repo: &str) -> Option<PathBuf> {
        let m = manifest();
        let spec = m.repos.iter().find(|r| r.repo == repo)?;
        Some(self.repo_dir(repo).join("snapshots").join(&spec.revision))
    }

    fn worker(&self) -> PathBuf {
        self.root.join("worker").join("nuai_voice.py")
    }

    /// The worker's stderr, for diagnosing failures.
    pub fn log(&self) -> PathBuf {
        self.root.join("worker.log")
    }

    fn marker(&self) -> PathBuf {
        self.root.join("installed.json")
    }
}

/// Changes whenever the pinned Python packages change, forcing a reinstall.
fn packages_fingerprint() -> String {
    let mut h = Sha256::new();
    h.update(PYTHON_VERSION);
    h.update(REQUIREMENTS);
    h.update(CONSTRAINTS);
    h.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn read_marker(paths: &VoicePaths) -> Value {
    std::fs::read(paths.marker())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(json!({}))
}

fn runtime_ready(paths: &VoicePaths) -> bool {
    paths.python().is_file() && read_marker(paths)["packages"] == packages_fingerprint()
}

/// Bytes of model files still missing (by size).
fn missing_model_bytes(paths: &VoicePaths) -> u64 {
    manifest()
        .repos
        .iter()
        .flat_map(|r| {
            let snap = paths.repo_dir(&r.repo).join("snapshots").join(&r.revision);
            r.files.iter().map(move |f| (snap.join(&f.path), f.size))
        })
        .filter(|(p, size)| {
            std::fs::metadata(p)
                .map(|m| m.len() != *size)
                .unwrap_or(true)
        })
        .map(|(_, size)| size)
        .sum()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStatus {
    pub installed: bool,
    pub runtime_ready: bool,
    pub models_ready: bool,
    /// What a full install still has to download, roughly.
    pub download_mb: u64,
    pub total_mb: u64,
    pub root: String,
}

pub fn status(paths: &VoicePaths) -> VoiceStatus {
    let runtime = runtime_ready(paths);
    let missing = missing_model_bytes(paths);
    let models_total: u64 = manifest()
        .repos
        .iter()
        .flat_map(|r| r.files.iter())
        .map(|f| f.size)
        .sum();
    VoiceStatus {
        installed: runtime && missing == 0 && paths.worker().is_file(),
        runtime_ready: runtime,
        models_ready: missing == 0,
        download_mb: missing / 1_000_000 + if runtime { 0 } else { RUNTIME_MB },
        total_mb: models_total / 1_000_000 + RUNTIME_MB,
        root: paths.root.to_string_lossy().into_owned(),
    }
}

/// Reports `(stage, fraction)`; fraction is negative while a stage has no measurable progress.
pub type StageProgress<'a> = &'a (dyn Fn(&str, f64) + Send + Sync);

pub async fn install(
    paths: &VoicePaths,
    progress: StageProgress<'_>,
    cancel: &Cancel,
) -> Result<()> {
    std::fs::create_dir_all(&paths.root)?;

    progress("runtime", -1.0);
    let uv = ensure_uv(paths, cancel).await?;
    if !runtime_ready(paths) {
        let mut venv = uv_command(&uv, paths);
        venv.args(["venv", "--clear", "--python", PYTHON_VERSION])
            .arg(paths.env_dir());
        run_logged(venv, cancel)
            .await
            .context("creating the Python environment")?;

        progress("packages", -1.0);
        let req = paths.root.join("requirements.txt");
        let cons = paths.root.join("constraints.txt");
        std::fs::write(&req, REQUIREMENTS)?;
        std::fs::write(&cons, CONSTRAINTS)?;
        // Relative paths: uv splits a `-c` path at spaces ("Application Support").
        let mut pip = uv_command(&uv, paths);
        pip.current_dir(&paths.root)
            .args(["pip", "install", "--python"])
            .arg(paths.python())
            .args(["-r", "requirements.txt", "-c", "constraints.txt"]);
        run_logged(pip, cancel)
            .await
            .context("installing the voice packages")?;
        let mut marker = read_marker(paths);
        marker["packages"] = json!(packages_fingerprint());
        std::fs::write(paths.marker(), serde_json::to_vec_pretty(&marker)?)?;
    }

    progress("models", 0.0);
    download_models(paths, &|p| progress("models", p), cancel).await?;

    std::fs::create_dir_all(paths.worker().parent().unwrap())?;
    std::fs::write(paths.worker(), WORKER)?;
    progress("models", 1.0);
    Ok(())
}

/// Deletes the whole engine (Python, packages, models).
pub fn uninstall(paths: &VoicePaths) -> Result<()> {
    if paths.root.file_name().is_some_and(|n| n == "voice") && paths.root.is_dir() {
        std::fs::remove_dir_all(&paths.root).context("removing the voice engine")?;
    }
    Ok(())
}

fn uv_command(uv: &Path, paths: &VoicePaths) -> tokio::process::Command {
    let mut cmd = tools::command(uv);
    cmd.env("UV_PYTHON_INSTALL_DIR", paths.root.join("python"))
        .env("UV_PYTHON_PREFERENCE", "only-managed")
        .env("UV_NO_PROGRESS", "1")
        .env("PATH", tools::child_path(uv));
    cmd
}

/// Runs a command to completion, honouring cancellation; errors carry its last output.
async fn run_logged(mut cmd: tokio::process::Command, cancel: &Cancel) -> Result<()> {
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
    let mut stderr = child.stderr.take().expect("piped");
    let log = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let status = loop {
        tokio::select! {
            st = child.wait() => break st?,
            _ = tick.tick() => {
                if cancel.is_cancelled() {
                    let _ = child.kill().await;
                    bail!("Cancelled");
                }
            }
        }
    };
    let out = log.await.unwrap_or_default();
    if !status.success() {
        let tail: Vec<&str> = out
            .lines()
            .filter(|l| !l.trim().is_empty())
            .rev()
            .take(6)
            .collect();
        bail!("{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    Ok(())
}

fn uv_target() -> Option<(&'static str, bool)> {
    let t = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        _ => return None,
    };
    Some((t, cfg!(windows)))
}

/// Uses an installed uv, or downloads the official standalone binary.
async fn ensure_uv(paths: &VoicePaths, cancel: &Cancel) -> Result<PathBuf> {
    if let Some(found) = tools::find_program("uv", None) {
        return Ok(found);
    }
    let (target, zip) =
        uv_target().ok_or_else(|| anyhow!("unsupported platform for the voice engine"))?;
    let bin = paths.root.join("bin");
    let exe = bin
        .join(format!("uv-{target}"))
        .join(if cfg!(windows) { "uv.exe" } else { "uv" });
    if exe.is_file() {
        return Ok(exe);
    }
    std::fs::create_dir_all(&bin)?;
    let ext = if zip { "zip" } else { "tar.gz" };
    let archive = bin.join(format!("uv.{ext}"));
    let url = format!("https://github.com/astral-sh/uv/releases/latest/download/uv-{target}.{ext}");
    download_file(&url, &archive, None, &|_| {}, cancel).await?;
    let mut tar = tools::command(Path::new("tar"));
    tar.arg(if zip { "-xf" } else { "-xzf" })
        .arg(&archive)
        .arg("-C")
        .arg(&bin);
    run_logged(tar, cancel).await.context("unpacking uv")?;
    let _ = std::fs::remove_file(&archive);
    if !exe.is_file() {
        bail!("uv was downloaded but its binary was not found");
    }
    Ok(exe)
}

/// Other Hugging Face caches on this machine; matching files are linked, not downloaded.
fn shared_hf_hubs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = std::env::var_os("HF_HOME") {
        v.push(PathBuf::from(h).join("hub"));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        v.push(
            PathBuf::from(home)
                .join(".cache")
                .join("huggingface")
                .join("hub"),
        );
    }
    v
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

async fn verify(path: &Path, size: u64, sha256: Option<&str>) -> Result<bool> {
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) != size {
        return Ok(false);
    }
    let Some(want) = sha256.map(str::to_string) else {
        return Ok(true);
    };
    let p = path.to_path_buf();
    let got = tokio::task::spawn_blocking(move || sha256_file(&p)).await??;
    Ok(got == want)
}

async fn download_models(
    paths: &VoicePaths,
    progress: &(dyn Fn(f64) + Send + Sync),
    cancel: &Cancel,
) -> Result<()> {
    let m = manifest();
    let total: u64 = m
        .repos
        .iter()
        .flat_map(|r| r.files.iter())
        .map(|f| f.size)
        .sum();
    let mut done: u64 = 0;
    for repo in &m.repos {
        let repo_dir = paths.repo_dir(&repo.repo);
        let snap = repo_dir.join("snapshots").join(&repo.revision);
        for f in &repo.files {
            cancel.check()?;
            let dest = snap.join(&f.path);
            if std::fs::metadata(&dest)
                .map(|md| md.len() == f.size)
                .unwrap_or(false)
            {
                done += f.size;
                progress(done as f64 / total as f64);
                continue;
            }
            std::fs::create_dir_all(dest.parent().unwrap())?;

            let reused = reuse_from_shared_cache(&repo.repo, &repo.revision, f, &dest).await?;
            if !reused {
                let url = format!(
                    "https://huggingface.co/{}/resolve/{}/{}",
                    repo.repo, repo.revision, f.path
                );
                let base = done;
                download_file(
                    &url,
                    &dest,
                    Some(f),
                    &|bytes| progress((base + bytes) as f64 / total as f64),
                    cancel,
                )
                .await
                .with_context(|| format!("downloading {}", f.path))?;
            }
            done += f.size;
            progress(done as f64 / total as f64);
        }
        std::fs::create_dir_all(repo_dir.join("refs"))?;
        std::fs::write(repo_dir.join("refs").join("main"), &repo.revision)?;
    }
    Ok(())
}

async fn reuse_from_shared_cache(
    repo: &str,
    revision: &str,
    f: &FileSpec,
    dest: &Path,
) -> Result<bool> {
    for hub in shared_hf_hubs() {
        let src = hub
            .join(format!("models--{}", repo.replace('/', "--")))
            .join("snapshots")
            .join(revision)
            .join(&f.path);
        let Ok(real) = std::fs::canonicalize(&src) else {
            continue;
        };
        if !verify(&real, f.size, f.sha256.as_deref()).await? {
            continue;
        }
        // Same volume: a hard link costs no space. Otherwise copy.
        if std::fs::hard_link(&real, dest).is_err() {
            std::fs::copy(&real, dest)?;
        }
        return Ok(true);
    }
    Ok(false)
}

/// Resumable download with stall detection and optional size/sha256 verification.
async fn download_file(
    url: &str,
    dest: &Path,
    spec: Option<&FileSpec>,
    on_bytes: &(dyn Fn(u64) + Send + Sync),
    cancel: &Cancel,
) -> Result<()> {
    let part = dest.with_extension(format!(
        "{}part",
        dest.extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()?;
    let mut last_err = None;
    for attempt in 0..12 {
        cancel.check()?;
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if spec.is_some_and(|s| have >= s.size) {
            break;
        }
        let mut req = client.get(url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(anyhow!(e));
                continue;
            }
        };
        let status = resp.status();
        if !(status.is_success()) {
            last_err = Some(anyhow!("server answered {status}"));
            continue;
        }
        // A server that ignores the range starts over.
        let append = have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&part)
            .await?;
        let mut written = if append { have } else { 0 };
        let mut stream = resp.bytes_stream();
        let mut failed = false;
        while let Some(chunk) = stream.next().await {
            if cancel.is_cancelled() {
                bail!("Cancelled");
            }
            match chunk {
                Ok(bytes) => {
                    file.write_all(&bytes).await?;
                    written += bytes.len() as u64;
                    on_bytes(written);
                }
                Err(e) => {
                    last_err = Some(anyhow!(e));
                    failed = true;
                    break;
                }
            }
        }
        file.flush().await?;
        if !failed {
            last_err = None;
            break;
        }
    }
    if let Some(e) = last_err {
        return Err(e.context("download kept failing"));
    }
    if let Some(s) = spec {
        if !verify(&part, s.size, s.sha256.as_deref()).await? {
            let _ = std::fs::remove_file(&part);
            bail!("{} failed verification; please try again", s.path);
        }
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

/// A running worker process with the models it has loaded.
pub struct VoiceWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    log: PathBuf,
}

impl VoiceWorker {
    pub async fn start(paths: &VoicePaths) -> Result<Self> {
        if !status(paths).installed {
            bail!("The voice engine is not installed. Install it in Settings → Voice.");
        }
        // Keep the worker script in step with the app version.
        std::fs::write(paths.worker(), WORKER)?;
        let python = paths.python();
        let mut child = tools::command(&python)
            .arg(paths.worker())
            .env("HF_HOME", paths.hf_home())
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("PYTORCH_ENABLE_MPS_FALLBACK", "1")
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONFAULTHANDLER", "1")
            .env("TOKENIZERS_PARALLELISM", "false")
            .env("PATH", tools::child_path(&python))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(
                std::fs::File::create(paths.log())
                    .map(Stdio::from)
                    .unwrap_or_else(|_| Stdio::null()),
            )
            .spawn()
            .context("starting the voice engine")?;
        let stdin = child.stdin.take().expect("piped");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped")).lines();
        let first = tokio::time::timeout(Duration::from_secs(120), stdout.next_line())
            .await
            .map_err(|_| anyhow!("the voice engine did not start"))??;
        if !first.is_some_and(|l| l.contains("\"ready\"")) {
            bail!("the voice engine failed to start");
        }
        Ok(VoiceWorker {
            child,
            stdin,
            stdout,
            next_id: 1,
            log: paths.log(),
        })
    }

    /// Sends one command and waits for its result, forwarding progress events.
    pub async fn request(
        &mut self,
        mut req: Value,
        progress: &Progress,
        cancel: &Cancel,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = json!(id);
        self.stdin.write_all(format!("{req}\n").as_bytes()).await?;
        self.stdin.flush().await?;
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                line = self.stdout.next_line() => {
                    let Some(line) = line? else {
                        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
                        let tail: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).rev().take(4).collect();
                        bail!("the voice engine stopped unexpectedly: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | "));
                    };
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                    if msg["id"] != json!(id) {
                        continue;
                    }
                    if msg["event"] == "progress" {
                        progress(msg["progress"].as_f64().unwrap_or(0.0));
                    } else if let Some(e) = msg["error"].as_str() {
                        bail!("voice engine: {e}");
                    } else if !msg["result"].is_null() {
                        return Ok(msg["result"].clone());
                    }
                }
                _ = tick.tick() => {
                    if cancel.is_cancelled() {
                        let _ = self.child.kill().await;
                        bail!("Cancelled");
                    }
                }
            }
        }
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// Picks the best stretch of speech to clone from: whole words, `min`–`max` seconds,
/// preferably bounded by pauses and transcribed with high confidence.
pub fn pick_reference(words: &[Word], min: f64, max: f64) -> Option<(f64, f64, String)> {
    let mut best: Option<(f64, usize, usize)> = None;
    for i in 0..words.len() {
        let mut j = i;
        // Grow the window over continuous speech only; a long pause inside a
        // reference makes the clone hesitate.
        while j + 1 < words.len()
            && words[j + 1].end - words[i].start <= max
            && words[j + 1].start - words[j].end <= 0.6
        {
            j += 1;
        }
        let dur = words[j].end - words[i].start;
        if dur < min {
            continue;
        }
        let pause_after = words
            .get(j + 1)
            .map(|w| w.start - words[j].end)
            .unwrap_or(1.0);
        let pause_before = if i == 0 {
            1.0
        } else {
            words[i].start - words[i - 1].end
        };
        let conf = words[i..=j].iter().map(|w| w.prob as f64).sum::<f64>() / (j - i + 1) as f64;
        let score = dur / max + pause_after.min(0.5) + pause_before.min(0.5) + conf;
        if best.is_none_or(|(s, _, _)| score > s) {
            best = Some((score, i, j));
        }
    }
    let (_, i, j) = best?;
    let text = words[i..=j]
        .iter()
        .map(|w| w.text.trim())
        .collect::<Vec<_>>()
        .join(" ");
    Some((words[i].start, words[j].end, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, start: f64, end: f64) -> Word {
        Word {
            text: text.into(),
            start,
            end,
            prob: 0.9,
        }
    }

    #[test]
    fn picks_a_window_between_pauses() {
        // Two phrases separated by a long pause; the second fits the window.
        let words = vec![
            w("kısa", 0.0, 0.4),
            w("bir", 0.5, 0.8),
            w("Geçen", 3.0, 3.5),
            w("sonbaharda", 3.6, 4.4),
            w("yağmurlu", 4.5, 5.2),
            w("bir", 5.3, 5.5),
            w("pazartesi", 5.6, 6.3),
            w("sabahı", 6.4, 7.0),
            w("karşılaştık.", 7.1, 8.6),
        ];
        let (s, e, text) = pick_reference(&words, 4.0, 10.0).unwrap();
        assert_eq!(s, 3.0);
        assert_eq!(e, 8.6);
        assert!(text.starts_with("Geçen") && text.ends_with("karşılaştık."));
    }

    #[test]
    fn manifest_parses_and_pins_revisions() {
        let m = manifest();
        assert!(m
            .repos
            .iter()
            .any(|r| r.repo == OMNIVOICE_REPO && r.revision.len() == 40));
        assert!(m.repos.iter().all(|r| !r.files.is_empty()));
    }
}
