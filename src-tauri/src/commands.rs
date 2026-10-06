//! Tauri commands: the boundary between the UI and the Rust core.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::ai::{AiClient, ModelInfo};
use crate::audio;
use crate::edit::Cut;
use crate::media::{self, MediaInfo};
use crate::models::{self, ModelStatus};
use crate::pipeline::{self, AnalyzeContext, Project};
use crate::render::{self, AudioPlan, ExportResult, SubtitlePlan};
use crate::settings::{self, AiProvider, AiSettings, AnalyzeOptions, Settings};
use crate::speech::Transcript;
use crate::subtitles::{self, Cue, SubtitleOptions, SubtitleTrack};
use crate::task::{Cancel, Progress};
use crate::tools::{self, Tools};
use crate::transcribe::LoadedModel;
use crate::voice::{self, VoicePaths, VoiceStatus, VoiceWorker};

pub struct AppState {
    pub settings_path: PathBuf,
    pub cache_dir: PathBuf,
    pub models_dir: PathBuf,
    pub settings: Mutex<Settings>,
    task: Mutex<Cancel>,
    download: Mutex<Cancel>,
    translate: Mutex<Cancel>,
    whisper: Arc<tokio::sync::Mutex<Option<Arc<LoadedModel>>>>,
    encoders: Mutex<Option<HashSet<String>>>,
    voice: VoicePaths,
    voice_worker: tokio::sync::Mutex<Option<VoiceWorker>>,
    voice_cancel: Mutex<Cancel>,
}

impl AppState {
    pub fn new(settings_path: PathBuf, cache_dir: PathBuf, models_dir: PathBuf) -> Self {
        let settings = settings::load(&settings_path);
        let voice = VoicePaths::new(models_dir.parent().unwrap_or(&models_dir));
        AppState {
            settings_path,
            cache_dir,
            models_dir,
            settings: Mutex::new(settings),
            task: Mutex::new(Cancel::default()),
            download: Mutex::new(Cancel::default()),
            translate: Mutex::new(Cancel::default()),
            whisper: Arc::new(tokio::sync::Mutex::new(None)),
            encoders: Mutex::new(None),
            voice,
            voice_worker: tokio::sync::Mutex::new(None),
            voice_cancel: Mutex::new(Cancel::default()),
        }
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    fn tools(&self) -> Result<Tools> {
        tools::locate(Some(&self.settings().ffmpeg_path))
    }

    /// Starts a new foreground job, cancelling whatever was running.
    fn begin_task(&self) -> Cancel {
        let mut guard = self.task.lock().unwrap();
        guard.cancel();
        *guard = Cancel::default();
        guard.clone()
    }

    async fn encoders(&self, tools: &Tools) -> Result<HashSet<String>> {
        if let Some(e) = self.encoders.lock().unwrap().clone() {
            return Ok(e);
        }
        let e = media::available_encoders(tools).await?;
        *self.encoders.lock().unwrap() = Some(e.clone());
        Ok(e)
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent<'a> {
    task: &'a str,
    stage: &'a str,
    progress: f64,
}

/// A progress callback that emits throttled `progress` events to the UI.
fn emitter(app: &AppHandle, task: &'static str, stage: &str) -> Progress {
    let app = app.clone();
    let stage = stage.to_string();
    let last = Mutex::new((Instant::now(), -1.0f64));
    let _ = app.emit(
        "progress",
        ProgressEvent {
            task,
            stage: &stage,
            progress: 0.0,
        },
    );
    Arc::new(move |p: f64| {
        let mut l = last.lock().unwrap();
        if p >= 1.0 || p - l.1 >= 0.01 || l.0.elapsed().as_millis() >= 250 {
            *l = (Instant::now(), p);
            let _ = app.emit(
                "progress",
                ProgressEvent {
                    task,
                    stage: &stage,
                    progress: p,
                },
            );
        }
    })
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<()> {
    settings::save(&state.settings_path, &settings).map_err(err)?;
    *state.settings.lock().unwrap() = settings;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    codex: crate::ai::codex::CodexStatus,
    ffmpeg: Option<Tools>,
    ffmpeg_error: Option<String>,
    models: Vec<ModelStatus>,
    models_dir: String,
    api_keys: serde_json::Value,
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> CmdResult<AppStatus> {
    let tools = state.tools();
    let has = |p| settings::get_secret(p).ok().flatten().is_some();
    let codex = crate::ai::codex::status(&state.settings().ai.codex_path).await;
    Ok(AppStatus {
        codex,
        ffmpeg_error: tools.as_ref().err().map(|e| format!("{e:#}")),
        ffmpeg: tools.ok(),
        models: models::statuses(&state.models_dir),
        models_dir: state.models_dir.to_string_lossy().into_owned(),
        api_keys: json!({
            "anthropic": has(AiProvider::Anthropic),
            "openai": has(AiProvider::Openai),
            "compatible": has(AiProvider::Compatible),
        }),
    })
}

#[tauri::command]
pub fn set_api_key(provider: AiProvider, key: String) -> CmdResult<()> {
    settings::set_secret(provider, &key).map_err(err)
}

#[tauri::command]
pub async fn list_ai_models(ai: AiSettings) -> CmdResult<Vec<ModelInfo>> {
    let client = AiClient::from_settings(&ai).map_err(err)?;
    client.list_models().await.map_err(err)
}

/// Sends a tiny structured request to confirm the key, model and JSON support work.
#[tauri::command]
pub async fn test_ai(ai: AiSettings) -> CmdResult<String> {
    let client = AiClient::from_settings(&ai).map_err(err)?;
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["ok"],
        "properties": { "ok": { "type": "boolean" } }
    });
    let v = client
        .complete_json(
            "Reply with JSON only.",
            "Return {\"ok\": true}.",
            "ping",
            &schema,
            &Cancel::default(),
        )
        .await
        .map_err(err)?;
    if v["ok"] == true {
        Ok(client.model_label())
    } else {
        Err(format!("Unexpected answer: {v}"))
    }
}

#[tauri::command]
pub async fn download_model(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<()> {
    let cancel = {
        let mut g = state.download.lock().unwrap();
        *g = Cancel::default();
        g.clone()
    };
    let progress = emitter(&app, "download", &id);
    models::download(&state.models_dir, &id, &progress, &cancel)
        .await
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn cancel_download(state: State<'_, AppState>) {
    state.download.lock().unwrap().cancel();
}

#[tauri::command]
pub fn delete_model(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if let Some(p) = models::model_path(&state.models_dir, &id) {
        if p.exists() {
            std::fs::remove_file(p).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_task(state: State<'_, AppState>) {
    state.task.lock().unwrap().cancel();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenResult {
    media: MediaInfo,
    preview_path: String,
    waveform: Vec<f32>,
    waveform_rate: f64,
    project: Option<Project>,
}

#[tauri::command]
pub async fn open_media(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<OpenResult> {
    open_media_inner(&app, &state, Path::new(&path))
        .await
        .map_err(err)
}

async fn open_media_inner(app: &AppHandle, state: &AppState, path: &Path) -> Result<OpenResult> {
    let cancel = state.begin_task();
    let tools = state.tools()?;
    let info = media::probe(&tools, path).await?;
    let media_dir = pipeline::media_cache_dir(&state.cache_dir, path)?;
    let scope = app.asset_protocol_scope();
    scope.allow_file(path).context("allowing preview access")?;

    let waveform = if info.has_audio {
        let progress = emitter(app, "open", "audio");
        let pcm = media::extract_pcm(&tools, path, info.duration, &progress, &cancel).await?;
        audio::waveform(&audio::envelope(&pcm))
    } else {
        Vec::new()
    };

    let preview_path = if info.playable {
        path.to_path_buf()
    } else {
        let proxy = media_dir.join("preview.mp4");
        if !proxy.is_file() {
            let progress = emitter(app, "open", "proxy");
            let encoders = state.encoders(&tools).await?;
            media::make_proxy(&tools, &encoders, &info, &proxy, &progress, &cancel).await?;
        }
        scope
            .allow_file(&proxy)
            .context("allowing preview access")?;
        proxy
    };

    Ok(OpenResult {
        media: info,
        preview_path: preview_path.to_string_lossy().into_owned(),
        waveform,
        waveform_rate: audio::WAVEFORM_RATE,
        project: pipeline::load_project(&media_dir),
    })
}

#[tauri::command]
pub async fn analyze(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    options: AnalyzeOptions,
    reason_language: String,
) -> CmdResult<Project> {
    let cancel = state.begin_task();
    let tools = state.tools().map_err(err)?;
    let stage_app = app.clone();
    let ctx = AnalyzeContext {
        tools,
        settings: state.settings(),
        models_dir: state.models_dir.clone(),
        cache_dir: state.cache_dir.clone(),
        whisper: state.whisper.clone(),
        reason_language,
        stage: Arc::new(move |stage: &str| emitter(&stage_app, "analyze", stage)),
        cancel,
    };
    pipeline::analyze(&ctx, Path::new(&path), &options)
        .await
        .map_err(err)
}

#[tauri::command]
pub fn save_project(state: State<'_, AppState>, project: Project) -> CmdResult<()> {
    let dir =
        pipeline::media_cache_dir(&state.cache_dir, Path::new(&project.media_path)).map_err(err)?;
    pipeline::save_project(&dir, &project).map_err(err)
}

#[tauri::command]
pub fn default_output_path(state: State<'_, AppState>, path: String) -> String {
    render::default_output(Path::new(&path), &state.settings().export)
        .to_string_lossy()
        .into_owned()
}

#[tauri::command]
pub async fn export_video(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    cuts: Vec<Cut>,
    output: String,
    subtitles: Option<SubtitlePlan>,
    audio: Option<AudioPlan>,
) -> CmdResult<ExportResult> {
    let subtitles = subtitles.unwrap_or_default();
    let audio = audio.unwrap_or_default();
    let cancel = state.begin_task();
    let result = async {
        let tools = state.tools()?;
        let encoders = state.encoders(&tools).await?;
        let info = media::probe(&tools, Path::new(&path)).await?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let work_dir = state.cache_dir.join(format!("render-{stamp}"));
        let progress = emitter(&app, "export", "render");
        let out = render::export(
            &tools,
            &encoders,
            &info,
            &cuts,
            Path::new(&output),
            &state.settings().export,
            &subtitles,
            &audio,
            &work_dir,
            &progress,
            &cancel,
        )
        .await;
        if out.is_err() {
            let _ = std::fs::remove_dir_all(&work_dir);
        }
        out
    }
    .await;
    result.map_err(err)
}

/// Groups the transcript words that survive the cuts into subtitle cues.
#[tauri::command]
pub fn build_subtitles(
    transcript: Transcript,
    cuts: Vec<Cut>,
    duration: f64,
    options: SubtitleOptions,
) -> Vec<Cue> {
    subtitles::build_cues(
        &transcript.utterances,
        &cuts,
        duration,
        &transcript.language,
        &options,
    )
}

#[tauri::command]
pub fn rewrap_subtitles(cues: Vec<Cue>, options: SubtitleOptions, language: String) -> Vec<Cue> {
    subtitles::rewrap(&cues, &options, &language)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateResult {
    cues: Vec<Cue>,
    missing: usize,
}

#[tauri::command]
pub async fn translate_subtitles(
    app: AppHandle,
    state: State<'_, AppState>,
    cues: Vec<Cue>,
    from: String,
    to: String,
    instructions: String,
    options: SubtitleOptions,
) -> CmdResult<TranslateResult> {
    let cancel = {
        let mut g = state.translate.lock().unwrap();
        g.cancel();
        *g = Cancel::default();
        g.clone()
    };
    let client = AiClient::from_settings(&state.settings().ai).map_err(err)?;
    let progress = emitter(&app, "translate", &to);
    let t = crate::ai::translate::translate(
        &client,
        &cues,
        &from,
        &to,
        &instructions,
        &progress,
        &cancel,
    )
    .await
    .map_err(err)?;
    Ok(TranslateResult {
        cues: subtitles::rewrap(&t.cues, &options, &to),
        missing: t.missing,
    })
}

#[tauri::command]
pub fn cancel_translation(state: State<'_, AppState>) {
    state.translate.lock().unwrap().cancel();
}

/// Writes one track as SRT or WebVTT, timed for the edited video.
#[tauri::command]
pub async fn export_subtitles(
    state: State<'_, AppState>,
    path: String,
    cuts: Vec<Cut>,
    track: SubtitleTrack,
    output: String,
) -> CmdResult<()> {
    async {
        let tools = state.tools()?;
        let info = media::probe(&tools, Path::new(&path)).await?;
        let keeps = render::export_keeps(&info, &cuts);
        render::write_subtitle_file(&track, &keeps, Path::new(&output))
    }
    .await
    .map_err(err)
}

#[tauri::command]
pub fn default_subtitle_path(state: State<'_, AppState>, path: String, language: String) -> String {
    let video = render::default_output(Path::new(&path), &state.settings().export);
    render::sidecar_path(&video, &language)
        .to_string_lossy()
        .into_owned()
}

#[tauri::command]
pub fn voice_status(state: State<'_, AppState>) -> VoiceStatus {
    voice::status(&state.voice)
}

fn new_voice_cancel(state: &AppState) -> Cancel {
    let mut g = state.voice_cancel.lock().unwrap();
    g.cancel();
    *g = Cancel::default();
    g.clone()
}

/// One-click install of the voice engine; progress arrives as `progress` events with
/// task "voice" and stages runtime → packages → models.
#[tauri::command]
pub async fn voice_install(app: AppHandle, state: State<'_, AppState>) -> CmdResult<VoiceStatus> {
    let cancel = new_voice_cancel(&state);
    let last = Mutex::new((String::new(), Instant::now(), -2.0f64));
    let progress = move |stage: &str, p: f64| {
        let mut l = last.lock().unwrap();
        if l.0 != stage || (p - l.2).abs() >= 0.005 || l.1.elapsed().as_millis() >= 500 {
            *l = (stage.to_string(), Instant::now(), p);
            let _ = app.emit(
                "progress",
                ProgressEvent {
                    task: "voice",
                    stage,
                    progress: p,
                },
            );
        }
    };
    voice::install(&state.voice, &progress, &cancel)
        .await
        .map_err(err)?;
    Ok(voice::status(&state.voice))
}

#[tauri::command]
pub fn voice_cancel(state: State<'_, AppState>) {
    state.voice_cancel.lock().unwrap().cancel();
}

#[tauri::command]
pub async fn voice_uninstall(state: State<'_, AppState>) -> CmdResult<VoiceStatus> {
    *state.voice_worker.lock().await = None;
    voice::uninstall(&state.voice).map_err(err)?;
    Ok(voice::status(&state.voice))
}

fn analyze_context(
    app: &AppHandle,
    state: &AppState,
    task: &'static str,
    cancel: Cancel,
) -> Result<AnalyzeContext> {
    let stage_app = app.clone();
    Ok(AnalyzeContext {
        tools: state.tools()?,
        settings: state.settings(),
        models_dir: state.models_dir.clone(),
        cache_dir: state.cache_dir.clone(),
        whisper: state.whisper.clone(),
        reason_language: "English".into(),
        stage: Arc::new(move |stage: &str| emitter(&stage_app, task, stage)),
        cancel,
    })
}

/// Clones the voice in `reference` (any audio or video) and reads `text` with it.
/// Returns the path of the generated WAV.
#[tauri::command]
pub async fn voice_try(
    app: AppHandle,
    state: State<'_, AppState>,
    reference: String,
    text: String,
    language: String,
) -> CmdResult<String> {
    let cancel = new_voice_cancel(&state);
    let result = async {
        let ctx = analyze_context(&app, &state, "voice", cancel.clone())?;
        let dir = state.cache_dir.join("voice");
        let reference = crate::dub::build_reference(&ctx, Path::new(&reference), &dir).await?;
        let out = dir.join(format!("try-{}.wav", crate::dub::stamp()));
        let line = crate::dub::Line {
            text,
            language: (language != "auto").then_some(language),
            duration: None,
            out: out.clone(),
        };
        let progress = emitter(&app, "voice", "speak");
        let mut worker = state.voice_worker.lock().await;
        crate::dub::speak(
            &state.voice,
            &mut worker,
            &reference,
            &[line],
            &progress,
            &cancel,
        )
        .await?;
        app.asset_protocol_scope().allow_file(&out)?;
        Ok(out.to_string_lossy().into_owned())
    }
    .await;
    result.map_err(err)
}

/// Dubs a subtitle track with the voice of the person speaking in the video:
/// separates speech from background, clones the speaker from the clean speech, speaks
/// every cue and mixes it over the background. Returns a full-length WAV (original
/// timeline) for preview and export. Progress: task "dub", stages separate/speak/mix.
#[tauri::command]
pub async fn dub_create(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    transcript: Transcript,
    track: SubtitleTrack,
) -> CmdResult<String> {
    let cancel = new_voice_cancel(&state);
    let result = async {
        let ctx = analyze_context(&app, &state, "dub", cancel.clone())?;
        let media_path = Path::new(&path);
        let info = media::probe(&ctx.tools, media_path).await?;
        let media_dir = pipeline::media_cache_dir(&state.cache_dir, media_path)?;
        let mut worker = state.voice_worker.lock().await;
        let mv = crate::dub::prepare_media_voice(
            &ctx,
            &state.voice,
            &mut worker,
            media_path,
            &transcript,
            &media_dir,
        )
        .await?;
        let out = crate::dub::dub_track(
            &ctx,
            &state.voice,
            &mut worker,
            &mv,
            &track,
            info.duration,
            &media_dir,
        )
        .await?;
        app.asset_protocol_scope().allow_file(&out)?;
        Ok(out.to_string_lossy().into_owned())
    }
    .await;
    result.map_err(err)
}
