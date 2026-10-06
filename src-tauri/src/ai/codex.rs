//! ChatGPT plans via the user's own Codex CLI (`codex exec`), signed in with their
//! ChatGPT account. No API key is involved; usage counts against their plan.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use super::ModelInfo;
use crate::tools;

fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
}

fn cache() -> Option<Value> {
    let bytes = std::fs::read(codex_home()?.join("models_cache.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            x.as_str()
                .or_else(|| x["effort"].as_str())
                .map(str::to_string)
        })
        .collect()
}

/// Models offered to this ChatGPT account, from the list Codex itself caches
/// (`$CODEX_HOME/models_cache.json`), best first.
pub fn cached_models() -> Vec<ModelInfo> {
    cache().map(|v| parse_catalog(&v)).unwrap_or_default()
}

/// Fresh model list for the signed-in account: `codex debug models` fetches the
/// catalog from OpenAI (and refreshes Codex's cache). Falls back to the cache when
/// offline or with Codex versions that lack the command.
pub async fn fetch_models(codex: &Path) -> Vec<ModelInfo> {
    let run = tools::command(codex)
        .args(["debug", "models"])
        .env("PATH", tools::child_path(codex))
        .output();
    let fresh = match tokio::time::timeout(std::time::Duration::from_secs(20), run).await {
        Ok(Ok(out)) if out.status.success() => serde_json::from_slice::<Value>(&out.stdout)
            .map(|v| parse_catalog(&v))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if fresh.is_empty() {
        cached_models()
    } else {
        fresh
    }
}

/// Visible models from a Codex model catalog, best first.
fn parse_catalog(v: &Value) -> Vec<ModelInfo> {
    let mut models: Vec<(i64, ModelInfo)> = v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| {
            let id = m["slug"].as_str()?.to_string();
            let info = ModelInfo {
                name: m["display_name"].as_str().unwrap_or(&id).to_string(),
                description: m["description"].as_str().map(str::to_string),
                efforts: strings(&m["supported_reasoning_levels"]),
                default_effort: m["default_reasoning_level"].as_str().map(str::to_string),
                id,
            };
            Some((m["priority"].as_i64().unwrap_or(i64::MAX), info))
        })
        .collect();
    models.sort_by_key(|(p, _)| *p);
    models.into_iter().map(|(_, m)| m).collect()
}

/// The configured effort if the model accepts it, else the model's default.
fn effort_for(model: &str, effort: &str) -> String {
    match cached_models().into_iter().find(|m| m.id == model) {
        Some(m) if !m.efforts.is_empty() && !m.efforts.iter().any(|e| e == effort) => {
            m.default_effort.unwrap_or_default()
        }
        _ => effort.to_string(),
    }
}

/// The model to run: the user's choice, else the account's top listed model.
/// Codex's own default can name a model the ChatGPT plan doesn't offer.
pub fn resolve_model(configured: &str) -> String {
    let m = configured.trim();
    if !m.is_empty() {
        return m.to_string();
    }
    cached_models()
        .into_iter()
        .next()
        .map(|m| m.id)
        .unwrap_or_default()
}

/// Codex binaries bundled with OpenAI's desktop apps, which update themselves and are
/// often newer than a CLI installed separately.
fn bundled_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        for apps in [PathBuf::from("/Applications"), home.join("Applications")] {
            out.push(apps.join("Codex.app/Contents/Resources/codex-cli/bin/codex"));
            out.push(apps.join("ChatGPT.app/Contents/Resources/codex"));
        }
    }
    out.retain(|p| p.is_file());
    out
}

/// `codex-cli 0.159.0` → (0, 159, 0, true); pre-releases rank below the release.
fn parse_version(text: &str) -> Option<(u64, u64, u64, bool)> {
    let v = text
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let (core, pre) = v.split_once('-').map_or((v, false), |(c, _)| (c, true));
    let mut n = core.split('.').map(|x| x.parse::<u64>().ok());
    Some((
        n.next()??,
        n.next().flatten().unwrap_or(0),
        n.next().flatten().unwrap_or(0),
        !pre,
    ))
}

fn version_of(path: &Path) -> Option<(u64, u64, u64, bool)> {
    let out = std::process::Command::new(path)
        .arg("--version")
        .env("PATH", tools::child_path(path))
        .stdin(Stdio::null())
        .output()
        .ok()?;
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

type Found = (PathBuf, Option<String>);
static NEWEST: std::sync::Mutex<Option<(std::time::Instant, Found)>> = std::sync::Mutex::new(None);

/// The newest Codex on this machine. The model list OpenAI returns depends on the
/// client version, so an outdated CLI can't use the latest models.
fn newest() -> Option<Found> {
    let mut cache = NEWEST.lock().unwrap();
    if let Some((at, found)) = cache.as_ref() {
        if at.elapsed() < std::time::Duration::from_secs(60) {
            return Some(found.clone());
        }
    }
    let mut candidates = tools::find_all("codex");
    candidates.extend(bundled_candidates());
    let best = candidates
        .into_iter()
        .filter_map(|p| version_of(&p).map(|v| (v, p)))
        .max_by_key(|(v, _)| *v)
        .map(|((a, b, c, release), p)| {
            (
                p,
                Some(format!("{a}.{b}.{c}{}", if release { "" } else { "-pre" })),
            )
        });
    if let Some(found) = &best {
        *cache = Some((std::time::Instant::now(), found.clone()));
    }
    best
}

fn locate_with_version(custom: &str) -> Result<Found> {
    if !custom.trim().is_empty() {
        let p = tools::find_program("codex", Some(custom))
            .ok_or_else(|| anyhow!("No Codex CLI at {}", custom.trim()))?;
        let v = version_of(&p).map(|(a, b, c, _)| format!("{a}.{b}.{c}"));
        return Ok((p, v));
    }
    newest().ok_or_else(|| {
        anyhow!("Codex CLI not found. Install the Codex app or the CLI (npm i -g @openai/codex), or set its location in Settings → AI.")
    })
}

pub fn locate(custom: &str) -> Result<PathBuf> {
    locate_with_version(custom).map(|(p, _)| p)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexStatus {
    pub path: Option<String>,
    pub version: Option<String>,
    pub logged_in: bool,
    pub detail: String,
}

pub async fn status(custom: &str) -> CodexStatus {
    let custom = custom.to_string();
    let found = tokio::task::spawn_blocking(move || locate_with_version(&custom)).await;
    let Ok(Ok((path, version))) = found else {
        return CodexStatus {
            path: None,
            version: None,
            logged_in: false,
            detail: String::new(),
        };
    };
    let out = tools::command(&path)
        .args(["login", "status"])
        .env("PATH", tools::child_path(&path))
        .output()
        .await;
    let (logged_in, detail) = match out {
        Ok(o) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            (
                o.status.success() && text.contains("Logged in"),
                text.trim().to_string(),
            )
        }
        Err(e) => (false, e.to_string()),
    };
    CodexStatus {
        path: Some(path.to_string_lossy().into_owned()),
        version,
        logged_in,
        detail,
    }
}

/// One non-interactive Codex turn in an empty scratch folder with a read-only sandbox,
/// constrained to `schema`. Returns the final message text.
pub async fn complete(
    codex: &Path,
    model: &str,
    effort: &str,
    system: &str,
    user: &str,
    schema: &Value,
) -> Result<String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("nuai-codex-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    // Codex occasionally fails a turn on its own (e.g. falling back to a model from the
    // user's Codex config); one retry clears most of these.
    let mut result = run(codex, &dir, model, effort, system, user, schema).await;
    if result.is_err() {
        result = run(codex, &dir, model, effort, system, user, schema).await;
    }
    let _ = std::fs::remove_dir_all(&dir);
    result
}

async fn run(
    codex: &Path,
    dir: &Path,
    model: &str,
    effort: &str,
    system: &str,
    user: &str,
    schema: &Value,
) -> Result<String> {
    let schema_path = dir.join("schema.json");
    let answer_path = dir.join("answer.txt");
    std::fs::write(&schema_path, serde_json::to_vec(schema)?)?;

    let mut cmd = tools::command(codex);
    cmd.current_dir(dir)
        .env("PATH", tools::child_path(codex))
        .args([
            "exec",
            "--skip-git-repo-check",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--sandbox",
            "read-only",
            "--color",
            "never",
        ])
        .arg("--output-schema")
        .arg(&schema_path)
        .arg("-o")
        .arg(&answer_path);
    if !model.trim().is_empty() {
        cmd.args(["-m", model.trim()]);
    }
    let effort = effort_for(model.trim(), effort);
    if !effort.is_empty() {
        cmd.arg("-c")
            .arg(format!("model_reasoning_effort=\"{effort}\""));
    }
    cmd.arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("starting Codex CLI")?;
    let prompt = format!("{system}\n\n---\n\n{user}");
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin.write_all(prompt.as_bytes()).await?;
    drop(stdin);
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        bail!(
            "Codex failed: {}",
            error_summary(&String::from_utf8_lossy(&out.stderr))
        );
    }
    let text = std::fs::read_to_string(&answer_path).context("Codex produced no answer")?;
    if text.trim().is_empty() {
        bail!("Codex returned an empty answer.");
    }
    Ok(text)
}

/// Codex echoes the prompt on stderr; pull out the actual error.
fn error_summary(stderr: &str) -> String {
    if let Some(line) = stderr.lines().rfind(|l| l.starts_with("ERROR")) {
        let json = line.trim_start_matches("ERROR:").trim();
        return serde_json::from_str::<Value>(json)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| json.to_string());
    }
    let tail: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(3)
        .collect();
    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_versions() {
        assert_eq!(parse_version("codex-cli 0.159.0"), Some((0, 159, 0, true)));
        assert_eq!(
            parse_version("codex-cli 0.155.0-alpha.9.2"),
            Some((0, 155, 0, false))
        );
        assert!(parse_version("codex-cli 0.155.1") < parse_version("codex-cli 0.159.0"));
        assert!(parse_version("codex-cli 0.159.0-alpha.1") < parse_version("codex-cli 0.159.0"));
        assert_eq!(parse_version("no version"), None);
    }

    #[test]
    fn parses_visible_models_by_priority() {
        let v = serde_json::json!({ "models": [
            { "slug": "b", "display_name": "B", "visibility": "list", "priority": 5,
              "supported_reasoning_levels": [{ "effort": "low" }, { "effort": "high" }],
              "default_reasoning_level": "low" },
            { "slug": "hidden", "visibility": "hide", "priority": 1 },
            { "slug": "a", "display_name": "A", "visibility": "list", "priority": 2 }
        ]});
        let m = parse_catalog(&v);
        assert_eq!(
            m.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(m[1].efforts, ["low", "high"]);
        assert_eq!(m[1].default_effort.as_deref(), Some("low"));
    }

    #[test]
    fn extracts_api_error_from_stderr() {
        let stderr = "1: some prompt line\nwarning: x\nERROR: {\"type\":\"error\",\"status\":400,\"error\":{\"message\":\"model not supported\"}}\n";
        assert_eq!(error_summary(stderr), "model not supported");
    }
}
