//! The analysis pipeline: audio → silences → transcript → rule-based cleanup → visual
//! freezes → AI review. Produces a `Project` (the edit decision list plus context).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ai::{review, AiClient};
use crate::audio;
use crate::edit::{self, Cut, CutKind, CutSource};
use crate::media::{self, MediaInfo};
use crate::models;
use crate::settings::{AnalyzeOptions, Settings};
use crate::speech::{self, Transcript, Word};
use crate::task::{Cancel, Progress};
use crate::tools::Tools;
use crate::transcribe::{self, LoadedModel};
use crate::vad;

pub const PROJECT_VERSION: u32 = 1;

/// Something the user should know about the analysis. `code` is localized by the UI;
/// `message` carries details such as the underlying error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub code: String,
    pub message: String,
}

fn notice(code: &str, message: impl Into<String>) -> Notice {
    Notice {
        code: code.into(),
        message: message.into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub version: u32,
    pub media_path: String,
    pub duration: f64,
    pub cuts: Vec<Cut>,
    pub transcript: Option<Transcript>,
    pub threshold_db: f32,
    pub notes: Vec<String>,
    pub warnings: Vec<Notice>,
    pub options: AnalyzeOptions,
    pub ai_model: Option<String>,
    #[serde(default)]
    pub subtitles: Option<crate::subtitles::Subtitles>,
    /// Dubbed audio per language: full-length WAVs on the original timeline.
    #[serde(default)]
    pub dubs: std::collections::BTreeMap<String, String>,
    /// The user confirmed they may clone the voice in this video.
    #[serde(default)]
    pub voice_consent: bool,
}

/// Per-file cache folder: transcripts, preview proxy, saved project.
pub fn media_cache_dir(cache_root: &Path, media: &Path) -> Result<PathBuf> {
    let meta =
        std::fs::metadata(media).with_context(|| format!("cannot read {}", media.display()))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut h = Sha256::new();
    h.update(media.to_string_lossy().as_bytes());
    h.update(meta.len().to_le_bytes());
    h.update(mtime.to_le_bytes());
    let key: String = h
        .finalize()
        .iter()
        .take(10)
        .map(|b| format!("{b:02x}"))
        .collect();
    let dir = cache_root.join("media").join(key);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn load_project(dir: &Path) -> Option<Project> {
    let bytes = std::fs::read(dir.join("project.json")).ok()?;
    serde_json::from_slice::<Project>(&bytes)
        .ok()
        .filter(|p| p.version == PROJECT_VERSION)
}

pub fn save_project(dir: &Path, project: &Project) -> Result<()> {
    std::fs::write(dir.join("project.json"), serde_json::to_vec(project)?)?;
    Ok(())
}

/// Reports which stage is running; the UI shows them as a checklist.
pub type StageReporter = Arc<dyn Fn(&str) -> Progress + Send + Sync>;

pub struct AnalyzeContext {
    pub tools: Tools,
    pub settings: Settings,
    pub models_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub whisper: Arc<tokio::sync::Mutex<Option<Arc<LoadedModel>>>>,
    pub reason_language: String,
    pub stage: StageReporter,
    pub cancel: Cancel,
}

impl AnalyzeContext {
    /// A copy sharing the same model cache, cancel token and stage reporter.
    pub fn clone_shallow(&self) -> AnalyzeContext {
        AnalyzeContext {
            tools: self.tools.clone(),
            settings: self.settings.clone(),
            models_dir: self.models_dir.clone(),
            cache_dir: self.cache_dir.clone(),
            whisper: self.whisper.clone(),
            reason_language: self.reason_language.clone(),
            stage: self.stage.clone(),
            cancel: self.cancel.clone(),
        }
    }
}

fn resolve_model_path(ctx: &AnalyzeContext) -> Result<PathBuf> {
    let t = &ctx.settings.transcription;
    if !t.custom_model_path.trim().is_empty() {
        let p = PathBuf::from(t.custom_model_path.trim());
        if !p.is_file() {
            bail!(
                "The custom speech model file was not found: {}",
                p.display()
            );
        }
        return Ok(p);
    }
    let p = models::model_path(&ctx.models_dir, &t.model)
        .ok_or_else(|| anyhow!("Unknown speech model {}", t.model))?;
    if !p.is_file() {
        bail!("The speech model \"{}\" is not downloaded yet. Download it in Settings → Transcription.", t.model);
    }
    Ok(p)
}

async fn get_model(ctx: &AnalyzeContext, path: &Path) -> Result<Arc<LoadedModel>> {
    let mut guard = ctx.whisper.lock().await;
    if let Some(m) = guard.as_ref() {
        if Path::new(&m.path) == path {
            return Ok(m.clone());
        }
    }
    *guard = None;
    let path = path.to_path_buf();
    let use_gpu = ctx.settings.transcription.use_gpu;
    let model =
        tokio::task::spawn_blocking(move || transcribe::load_model(&path, use_gpu)).await??;
    let model = Arc::new(model);
    *guard = Some(model.clone());
    Ok(model)
}

async fn get_transcript(
    ctx: &AnalyzeContext,
    media_dir: &Path,
    pcm: Vec<f32>,
    env: &[f32],
    threshold: f32,
    with_fillers: bool,
) -> Result<Transcript> {
    let model_path = resolve_model_path(ctx)?;
    let language = ctx.settings.transcription.language.clone();
    let model_name = model_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cache = media_dir.join(format!(
        "transcript-v2-{model_name}-{language}{}.json",
        if with_fillers { "-f" } else { "" }
    ));
    if let Some(t) = std::fs::read(&cache)
        .ok()
        .and_then(|b| serde_json::from_slice::<Transcript>(&b).ok())
    {
        return Ok(t);
    }

    let progress = (ctx.stage)("transcribe");
    let model = get_model(ctx, &model_path).await?;
    let prompt = if with_fillers {
        speech::filler_prompt(&language)
    } else {
        None
    };
    let cancel = ctx.cancel.clone();
    let lang = language.clone();
    let mut raw = tokio::task::spawn_blocking(move || {
        transcribe::transcribe(&model, &pcm, &lang, prompt, progress, cancel)
    })
    .await??;
    retime_words(&mut raw.words, env, threshold);
    let transcript = Transcript {
        language: raw.language,
        model: model_name,
        utterances: speech::build_utterances(raw.words),
    };
    let _ = std::fs::write(&cache, serde_json::to_vec(&transcript)?);
    Ok(transcript)
}

/// Re-anchors word timings on the audio. Whisper's onsets (DTW or token timestamps) are
/// kept as anchors, but a word may not start inside a pause, starts where its sound
/// starts, and ends where the voice stops or the next word begins.
pub fn retime_words(words: &mut [Word], env: &[f32], threshold: f32) {
    const FRAME: f64 = audio::FRAME_SEC;
    const MAX_BACK: f64 = 0.25;
    const MAX_FORWARD: f64 = 1.5;
    const QUIET_FRAMES: usize = 12;
    let total = env.len() as f64 * FRAME;
    let voiced = |t: f64| {
        env.get((t / FRAME).floor().max(0.0) as usize)
            .is_some_and(|&db| db >= threshold)
    };
    let quiet_from = |f: usize| {
        f + QUIET_FRAMES <= env.len() && env[f..f + QUIET_FRAMES].iter().all(|&db| db < threshold)
    };

    let anchors: Vec<f64> = words.iter().map(|w| w.start).collect();
    let mut prev_end = 0.0f64;
    for i in 0..words.len() {
        let anchor = anchors[i].max(prev_end);
        let mut s = anchor;
        if voiced(s) {
            while s - FRAME >= prev_end && s - FRAME >= anchor - MAX_BACK && voiced(s - FRAME) {
                s -= FRAME;
            }
        } else {
            let mut t = s;
            while t < (s + MAX_FORWARD).min(total) && !voiced(t) {
                t += FRAME;
            }
            if voiced(t) {
                s = t;
            }
        }

        let next = anchors.get(i + 1).copied().unwrap_or(total).max(s + 0.05);
        let mut f = ((s + 0.05) / FRAME) as usize;
        while f < env.len() && (f as f64 * FRAME) < next && !quiet_from(f) {
            f += 1;
        }
        let e = (f as f64 * FRAME).min(next).max(s + 0.05);
        words[i].start = s;
        words[i].end = e;
        prev_end = e;
    }
}

/// Word-timed transcript of any audio/video file (used to pick voice references).
pub async fn transcribe_words(ctx: &AnalyzeContext, path: &Path) -> Result<(Vec<Word>, String)> {
    let info = media::probe(&ctx.tools, path).await?;
    if !info.has_audio {
        bail!("The file has no audio.");
    }
    let progress = (ctx.stage)("audio");
    let pcm = media::extract_pcm(&ctx.tools, path, info.duration, &progress, &ctx.cancel).await?;
    let env = audio::envelope(&pcm);
    let (speech, speech_thr) = speech_activity(
        &pcm,
        &env,
        audio::auto_threshold(&env),
        0.0,
        &ctx.models_dir,
    );
    let model_path = resolve_model_path(ctx)?;
    let model = get_model(ctx, &model_path).await?;
    let progress = (ctx.stage)("transcribe");
    let cancel = ctx.cancel.clone();
    let lang = ctx.settings.transcription.language.clone();
    let mut raw = tokio::task::spawn_blocking(move || {
        transcribe::transcribe(&model, &pcm, &lang, None, progress, cancel)
    })
    .await??;
    retime_words(&mut raw.words, &speech, speech_thr);
    Ok((raw.words, raw.language))
}

/// Speech activity as an envelope + threshold for the silence, retiming and hallucination
/// logic: Silero VAD when available (robust to background music), loudness otherwise.
/// Positive `sensitivity` treats more frames as non-speech, i.e. cuts more.
fn speech_activity(
    pcm: &[f32],
    env: &[f32],
    energy_threshold: f32,
    sensitivity: f64,
    models_dir: &Path,
) -> (Vec<f32>, f32) {
    let vad = tokio::task::block_in_place(|| vad::speech_probabilities(pcm, models_dir));
    match vad {
        Ok((probs, window)) => {
            let threshold = (0.5 + sensitivity as f32 * 0.025).clamp(0.15, 0.85);
            (
                vad::speech_envelope(&probs, window, env.len(), threshold),
                vad::SPEECH_THRESHOLD_DB,
            )
        }
        Err(_) => (env.to_vec(), energy_threshold + sensitivity as f32),
    }
}

/// Fraction of envelope frames above the silence threshold.
fn voiced_fraction(env: &[f32], start: f64, end: f64, threshold: f32) -> f64 {
    let a = ((start / audio::FRAME_SEC) as usize).min(env.len());
    let b = ((end / audio::FRAME_SEC).ceil() as usize).min(env.len());
    if b <= a {
        return 0.0;
    }
    env[a..b].iter().filter(|&&db| db >= threshold).count() as f64 / (b - a) as f64
}

/// Whisper sometimes "hears" words in silence (credits, "thank you"); drop phrases with no sound.
fn drop_hallucinations(t: &mut Transcript, env: &[f32], threshold: f32) {
    t.utterances
        .retain(|u| voiced_fraction(env, u.start, u.end, threshold) >= 0.15);
    for (i, u) in t.utterances.iter_mut().enumerate() {
        u.id = format!("u{}", i + 1);
    }
}

/// `range` minus `spans`, keeping pieces of at least `min_len` seconds.
fn subtract(range: (f64, f64), spans: &[(f64, f64)], min_len: f64) -> Vec<(f64, f64)> {
    let mut pieces = vec![range];
    for &(s, e) in spans {
        pieces = pieces
            .into_iter()
            .flat_map(|(a, b)| {
                if e <= a || s >= b {
                    vec![(a, b)]
                } else {
                    let mut v = Vec::new();
                    if s > a {
                        v.push((a, s));
                    }
                    if e < b {
                        v.push((e, b));
                    }
                    v
                }
            })
            .collect();
    }
    pieces.retain(|(a, b)| b - a >= min_len);
    pieces
}

pub async fn analyze(ctx: &AnalyzeContext, path: &Path, opts: &AnalyzeOptions) -> Result<Project> {
    let cancel = &ctx.cancel;
    let info: MediaInfo = media::probe(&ctx.tools, path).await?;
    let media_dir = media_cache_dir(&ctx.cache_dir, path)?;
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    let mut cuts: Vec<Cut> = Vec::new();
    let mut env: Vec<f32> = Vec::new();
    let mut threshold = -50.0f32;
    // Where someone is speaking (VAD), as an envelope + threshold; see `speech_activity`.
    let mut speech: Vec<f32> = Vec::new();
    let mut speech_thr = vad::SPEECH_THRESHOLD_DB;
    let mut transcript: Option<Transcript> = None;
    let mut ai_model = None;

    if info.has_audio {
        let progress = (ctx.stage)("audio");
        let pcm = media::extract_pcm(&ctx.tools, path, info.duration, &progress, cancel).await?;
        env = audio::envelope(&pcm);
        threshold = audio::auto_threshold(&env);
        let (s_env, s_thr) =
            speech_activity(&pcm, &env, threshold, opts.sensitivity, &ctx.models_dir);
        speech = s_env;
        speech_thr = s_thr;
        if opts.remove_silence {
            let regions = audio::silent_regions(&speech, speech_thr, opts.min_silence.max(0.1));
            cuts.extend(edit::silence_cuts(
                &regions,
                opts.padding.max(0.0),
                info.duration,
            ));
        }
        cancel.check()?;

        let wants_speech =
            opts.remove_retakes || opts.remove_fillers || opts.remove_stutters || opts.use_ai;
        if opts.transcribe && wants_speech {
            match get_transcript(
                ctx,
                &media_dir,
                pcm,
                &speech,
                speech_thr,
                opts.remove_fillers,
            )
            .await
            {
                Ok(mut t) => {
                    drop_hallucinations(&mut t, &speech, speech_thr);
                    let mut speech_cuts = Vec::new();
                    if opts.remove_retakes {
                        speech_cuts.extend(speech::detect_retakes(&t.utterances));
                    }
                    if opts.remove_stutters {
                        speech_cuts.extend(speech::detect_stutters(&t.utterances));
                    }
                    if opts.remove_fillers {
                        speech_cuts.extend(speech::detect_fillers(&t.utterances));
                    }
                    for c in speech_cuts.iter_mut() {
                        edit::snap_to_audio(c, &env);
                    }
                    cuts.extend(speech_cuts);
                    transcript = Some(t);
                }
                Err(e) if cancel.is_cancelled() => return Err(e),
                Err(e) => warnings.push(notice("transcriptionFailed", format!("{e:#}"))),
            }
        }
    } else {
        warnings.push(notice("noAudio", ""));
    }
    cancel.check()?;

    if opts.remove_static && info.has_video {
        let progress = (ctx.stage)("visual");
        let freezes = media::detect_freezes(
            &ctx.tools,
            &info,
            opts.min_static.max(0.5),
            &progress,
            cancel,
        )
        .await?;
        // Never cut a frozen picture while someone is talking over it.
        let speech_spans: Vec<(f64, f64)> = match &transcript {
            Some(t) => t
                .utterances
                .iter()
                .map(|u| (u.start - 0.2, u.end + 0.2))
                .collect(),
            None if !env.is_empty() => {
                let silent = audio::silent_regions(&speech, speech_thr, 0.3);
                subtract((0.0, info.duration), &silent, 0.0)
            }
            None => Vec::new(),
        };
        for f in freezes {
            for (s, e) in subtract(f, &speech_spans, opts.min_static.max(0.5)) {
                let (s, e) = (s + 0.1, e - 0.1);
                cuts.push(
                    Cut::new(
                        CutKind::Static,
                        CutSource::Rule,
                        s,
                        e,
                        0.8,
                        format!("{:.1}s without picture changes", e - s),
                    )
                    .coded("static", format!("{:.1}", e - s)),
                );
            }
        }
    }
    cancel.check()?;

    if opts.use_ai {
        match &transcript {
            None => warnings.push(notice("aiNeedsTranscript", "")),
            Some(t) if t.utterances.is_empty() => warnings.push(notice("aiNoSpeech", "")),
            Some(t) => {
                let progress = (ctx.stage)("ai");
                let candidates: Vec<Cut> = cuts
                    .iter()
                    .filter(|c| matches!(c.kind, CutKind::Retake | CutKind::Stutter))
                    .cloned()
                    .collect();
                let base: Vec<Cut> = cuts
                    .iter()
                    .filter(|c| !matches!(c.kind, CutKind::Retake | CutKind::Stutter))
                    .cloned()
                    .collect();
                let result = async {
                    let client = AiClient::from_settings(&ctx.settings.ai)?;
                    let input = review::ReviewInput {
                        utterances: &t.utterances,
                        transcript_language: &t.language,
                        reason_language: &ctx.reason_language,
                        duration: info.duration,
                        edited_duration: edit::edited_duration(&base, info.duration),
                        target_duration: (opts.target_duration > 0.0)
                            .then_some(opts.target_duration),
                        instructions: &opts.instructions,
                        candidates: &candidates,
                        remove_fillers: opts.remove_fillers,
                    };
                    let out = review::review(&client, &input, &progress, cancel).await?;
                    Ok::<_, anyhow::Error>((client.model_label(), out))
                }
                .await;
                match result {
                    Ok((model, out)) => {
                        // The AI's verdict replaces the rule-based retake/stutter guesses.
                        cuts = base;
                        for mut c in out.cuts {
                            edit::snap_to_audio(&mut c, &env);
                            let dup = cuts.iter().any(|o| {
                                o.kind == c.kind
                                    && o.start <= c.start + 0.05
                                    && o.end >= c.end - 0.05
                            });
                            if !dup {
                                cuts.push(c);
                            }
                        }
                        notes = out.notes;
                        ai_model = Some(model);
                    }
                    Err(e) if cancel.is_cancelled() => return Err(e),
                    Err(e) => warnings.push(notice("aiFailed", format!("{e:#}"))),
                }
            }
        }
    }

    cuts.retain(|c| c.len() > 0.03);
    cuts.sort_by(|a, b| a.start.total_cmp(&b.start));
    edit::assign_ids(&mut cuts);

    let project = Project {
        version: PROJECT_VERSION,
        media_path: info.path.clone(),
        duration: info.duration,
        cuts,
        transcript,
        threshold_db: threshold,
        notes,
        warnings,
        options: opts.clone(),
        ai_model,
        subtitles: None,
        dubs: Default::default(),
        voice_consent: false,
    };
    let _ = save_project(&media_dir, &project);
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f64) -> Word {
        Word {
            text: "w".into(),
            start,
            end: start + 0.01,
            prob: 1.0,
        }
    }

    #[test]
    fn retime_moves_words_out_of_pauses_and_ends_them_at_silence() {
        // 0-1s speech, 1-3s silence, 3-4s speech, then silence.
        let mut env = vec![-20.0f32; 100];
        env.extend(vec![-70.0; 200]);
        env.extend(vec![-20.0; 100]);
        env.extend(vec![-70.0; 100]);
        // Second word's anchor wrongly sits in the pause; first word's anchor is late.
        let mut words = vec![word(0.1), word(2.0), word(3.5)];
        retime_words(&mut words, &env, -40.0);
        assert!(words[0].start < 0.01, "{:?}", words[0].start);
        assert!(
            (words[0].end - 1.0).abs() < 0.02,
            "first word ends at the silence: {}",
            words[0].end
        );
        assert!(
            (words[1].start - 3.0).abs() < 0.02,
            "moved to the next onset: {}",
            words[1].start
        );
        assert!(
            (words[1].end - 3.5).abs() < 0.02,
            "ends where the next word starts: {}",
            words[1].end
        );
        assert!((words[2].end - 4.0).abs() < 0.02, "{}", words[2].end);
    }

    #[test]
    fn subtract_removes_spans() {
        let r = subtract((0.0, 10.0), &[(2.0, 3.0), (5.0, 9.5)], 1.0);
        assert_eq!(r, vec![(0.0, 2.0), (3.0, 5.0)]);
    }
}
