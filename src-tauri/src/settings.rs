//! Persisted preferences (JSON in the app config dir) and API keys (OS keychain).

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::models::DEFAULT_MODEL;
use crate::subtitles::SubtitleOptions;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnalyzeOptions {
    pub remove_silence: bool,
    /// Shortest pause (s) that gets cut.
    pub min_silence: f64,
    /// Air (s) left on each side of speech when a pause is cut.
    pub padding: f64,
    /// Added to the automatic silence threshold in dB; higher cuts more aggressively.
    pub sensitivity: f64,
    pub transcribe: bool,
    pub remove_retakes: bool,
    pub remove_fillers: bool,
    pub remove_stutters: bool,
    pub remove_static: bool,
    /// Shortest frozen stretch (s) that counts as static.
    pub min_static: f64,
    pub use_ai: bool,
    pub instructions: String,
    /// Desired final length in seconds; 0 means no target.
    pub target_duration: f64,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        AnalyzeOptions {
            remove_silence: true,
            min_silence: 0.5,
            padding: 0.15,
            sensitivity: 0.0,
            transcribe: true,
            remove_retakes: true,
            remove_fillers: true,
            remove_stutters: true,
            remove_static: false,
            min_static: 2.0,
            use_ai: false,
            instructions: String::new(),
            target_duration: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiProvider {
    None,
    Anthropic,
    Openai,
    Compatible,
    /// The user's own Codex CLI, signed in with their ChatGPT plan.
    CodexCli,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    pub provider: AiProvider,
    pub anthropic_model: String,
    pub openai_model: String,
    pub compatible_base_url: String,
    pub compatible_model: String,
    /// Empty: Codex's default model.
    pub codex_model: String,
    /// Codex binary; empty means auto-detect.
    pub codex_path: String,
    /// Reasoning effort for models that support it: low | medium | high.
    pub effort: String,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            provider: AiProvider::None,
            anthropic_model: "claude-opus-5-5".into(),
            openai_model: String::new(),
            compatible_base_url: "http://localhost:11434/v1".into(),
            compatible_model: String::new(),
            codex_model: String::new(),
            codex_path: String::new(),
            effort: "medium".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TranscriptionSettings {
    pub model: String,
    /// Use a ggml model file from anywhere on disk instead of a catalog model.
    pub custom_model_path: String,
    /// ISO code or "auto".
    pub language: String,
    pub use_gpu: bool,
}

impl Default for TranscriptionSettings {
    fn default() -> Self {
        TranscriptionSettings {
            model: DEFAULT_MODEL.into(),
            custom_model_path: String::new(),
            language: "auto".into(),
            use_gpu: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportSettings {
    /// h264 | hevc
    pub codec: String,
    /// high | balanced | small
    pub quality: String,
    /// Empty: next to the source file.
    pub output_dir: String,
    pub suffix: String,
    /// none | embed (selectable track) | burn (drawn into the picture)
    pub subtitle_mode: String,
    /// Also write `.srt` files next to the video.
    pub subtitle_sidecar: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            codec: "h264".into(),
            quality: "high".into(),
            output_dir: String::new(),
            suffix: "_edited".into(),
            subtitle_mode: "none".into(),
            subtitle_sidecar: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubtitleSettings {
    pub options: SubtitleOptions,
    /// Burned-in caption look (drawn by the UI): small | medium | large
    pub size: String,
    /// outline | box
    pub style: String,
    /// Extra guidance for AI translations (tone, terminology).
    pub translation_instructions: String,
}

impl Default for SubtitleSettings {
    fn default() -> Self {
        SubtitleSettings {
            options: SubtitleOptions::default(),
            size: "medium".into(),
            style: "outline".into(),
            translation_instructions: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditMode {
    Review,
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// "system" | "en" | "tr"
    pub ui_language: String,
    pub mode: EditMode,
    pub analysis: AnalyzeOptions,
    pub ai: AiSettings,
    pub transcription: TranscriptionSettings,
    pub export: ExportSettings,
    pub subtitles: SubtitleSettings,
    /// ffmpeg binary or folder; empty means auto-detect.
    pub ffmpeg_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            ui_language: "system".into(),
            mode: EditMode::Review,
            analysis: AnalyzeOptions::default(),
            ai: AiSettings::default(),
            transcription: TranscriptionSettings::default(),
            export: ExportSettings::default(),
            subtitles: SubtitleSettings::default(),
            ffmpeg_path: String::new(),
        }
    }
}

pub fn load(path: &Path) -> Settings {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(settings)?;
    std::fs::write(path, json).context("writing settings")
}

const KEYRING_SERVICE: &str = "app.nuai.editor";

fn entry(provider: AiProvider) -> Result<keyring::Entry> {
    let user = match provider {
        AiProvider::Anthropic => "anthropic",
        AiProvider::Openai => "openai",
        AiProvider::Compatible => "compatible",
        AiProvider::None | AiProvider::CodexCli => {
            anyhow::bail!("this provider doesn't use an API key")
        }
    };
    keyring::Entry::new(KEYRING_SERVICE, user).context("opening the system keychain")
}

pub fn get_secret(provider: AiProvider) -> Result<Option<String>> {
    match entry(provider)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("reading the API key from the keychain"),
    }
}

pub fn set_secret(provider: AiProvider, key: &str) -> Result<()> {
    let e = entry(provider)?;
    if key.trim().is_empty() {
        match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err).context("removing the API key"),
        }
    } else {
        e.set_password(key.trim())
            .context("saving the API key to the keychain")
    }
}
