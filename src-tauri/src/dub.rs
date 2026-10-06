//! Voice cloning workflows on top of the voice engine: cloning the voice of the person
//! speaking in a video, dubbing subtitle tracks with it, and trying any recording.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::media;
use crate::pipeline::{self, AnalyzeContext};
use crate::speech::{Transcript, Word};
use crate::subtitles::SubtitleTrack;
use crate::task::{Cancel, Progress};
use crate::voice::{self, VoicePaths, VoiceWorker};

/// A short clean clip of the target voice and what is said in it.
pub struct Reference {
    pub audio: PathBuf,
    pub text: String,
}

/// Transcribes `source` and cuts its best 4–10 s of continuous speech into `dir`.
pub async fn build_reference(ctx: &AnalyzeContext, source: &Path, dir: &Path) -> Result<Reference> {
    let mut ctx_auto = AnalyzeContext {
        settings: ctx.settings.clone(),
        ..ctx.clone_shallow()
    };
    ctx_auto.settings.transcription.language = "auto".into();
    let (words, _) = pipeline::transcribe_words(&ctx_auto, source).await?;
    if words.is_empty() {
        bail!("No speech was found in the voice sample.");
    }
    let (start, end, text) = voice::pick_reference(&words, 4.0, 10.0).unwrap_or_else(|| {
        let text = words
            .iter()
            .map(|w| w.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        (words[0].start, words[words.len() - 1].end, text)
    });
    std::fs::create_dir_all(dir)?;
    let audio = dir.join(format!("ref-{}.wav", stamp()));
    let args: Vec<String> = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-ss".into(),
        format!("{:.3}", (start - 0.1).max(0.0)),
        "-to".into(),
        format!("{:.3}", end + 0.15),
        "-i".into(),
        source.to_string_lossy().into_owned(),
        "-vn".into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        "24000".into(),
        audio.to_string_lossy().into_owned(),
    ];
    media::run_ffmpeg(&ctx.tools, &args, end - start, None, &ctx.cancel).await?;
    Ok(Reference { audio, text })
}

pub struct Line {
    pub text: String,
    /// ISO code, or None to let the model infer it.
    pub language: Option<String>,
    /// Target length in seconds.
    pub duration: Option<f64>,
    pub out: PathBuf,
}

/// Speaks each line with the reference voice. Starts the worker when needed.
pub async fn speak(
    paths: &VoicePaths,
    worker: &mut Option<VoiceWorker>,
    reference: &Reference,
    lines: &[Line],
    progress: &Progress,
    cancel: &Cancel,
) -> Result<()> {
    ensure_worker(paths, worker).await?;
    let model_dir = paths
        .snapshot(voice::OMNIVOICE_REPO)
        .ok_or_else(|| anyhow!("voice model missing"))?;
    let items: Vec<_> = lines
        .iter()
        .map(|l| json!({ "text": l.text, "language": l.language, "duration": l.duration, "out": l.out }))
        .collect();
    let req = json!({
        "cmd": "clone",
        "model_dir": model_dir,
        "ref_audio": reference.audio,
        "ref_text": reference.text,
        "fast": true,
        "items": items,
    });
    let result = worker
        .as_mut()
        .unwrap()
        .request(req, progress, cancel)
        .await;
    if result.is_err() {
        *worker = None;
    }
    result.map(|_| ())
}

async fn ensure_worker<'a>(
    paths: &VoicePaths,
    worker: &'a mut Option<VoiceWorker>,
) -> Result<&'a mut VoiceWorker> {
    if worker.as_mut().is_none_or(|w| !w.is_alive()) {
        *worker = Some(VoiceWorker::start(paths).await?);
    }
    Ok(worker.as_mut().unwrap())
}

fn ffmpeg_args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The video's own voice: separated stems plus a clean reference of the speaker.
pub struct MediaVoice {
    pub vocals: PathBuf,
    pub background: PathBuf,
    pub reference: Reference,
}

/// Separates the media's speech from its background (Demucs) and cuts the speaker's
/// best 5–10 s from the clean speech as the cloning reference. Cached per media file.
#[allow(clippy::too_many_arguments)]
pub async fn prepare_media_voice(
    ctx: &AnalyzeContext,
    paths: &VoicePaths,
    worker: &mut Option<VoiceWorker>,
    media_path: &Path,
    transcript: &Transcript,
    media_dir: &Path,
) -> Result<MediaVoice> {
    let dir = media_dir.join("voice");
    std::fs::create_dir_all(&dir)?;
    let vocals = dir.join("vocals.wav");
    let background = dir.join("background.wav");
    if !(vocals.is_file() && background.is_file()) {
        let progress = (ctx.stage)("separate");
        let mix = dir.join("mix.wav");
        let mut args = ffmpeg_args(&["-y", "-v", "error", "-i"]);
        args.push(path_arg(media_path));
        args.extend(ffmpeg_args(&[
            "-vn",
            "-ac",
            "2",
            "-ar",
            "44100",
            "-c:a",
            "pcm_s16le",
        ]));
        args.push(path_arg(&mix));
        media::run_ffmpeg(&ctx.tools, &args, 0.0, None, &ctx.cancel).await?;
        let req = json!({ "cmd": "separate", "input": mix, "out_dir": dir });
        let w = ensure_worker(paths, worker).await?;
        let result = w.request(req, &progress, &ctx.cancel).await;
        let _ = std::fs::remove_file(&mix);
        if let Err(e) = result {
            *worker = None;
            return Err(e.context("separating the voice from the background"));
        }
    }

    let ref_audio = dir.join("reference.wav");
    let ref_text_path = dir.join("reference.txt");
    let text = match std::fs::read_to_string(&ref_text_path) {
        Ok(t) if ref_audio.is_file() => t,
        _ => {
            let words: Vec<Word> = transcript
                .utterances
                .iter()
                .flat_map(|u| u.words.iter().cloned())
                .collect();
            let (start, end, text) = voice::pick_reference(&words, 5.0, 10.0)
                .or_else(|| voice::pick_reference(&words, 2.0, 10.0))
                .ok_or_else(|| {
                    anyhow!("There isn't enough clear speech in this video to clone the voice.")
                })?;
            let mut args = ffmpeg_args(&["-y", "-v", "error", "-ss"]);
            args.push(format!("{:.3}", (start - 0.1).max(0.0)));
            args.push("-to".into());
            args.push(format!("{:.3}", end + 0.15));
            args.push("-i".into());
            args.push(path_arg(&vocals));
            args.extend(ffmpeg_args(&["-ac", "1", "-ar", "24000"]));
            args.push(path_arg(&ref_audio));
            media::run_ffmpeg(&ctx.tools, &args, end - start, None, &ctx.cancel).await?;
            std::fs::write(&ref_text_path, &text)?;
            text
        }
    };
    Ok(MediaVoice {
        vocals,
        background,
        reference: Reference {
            audio: ref_audio,
            text,
        },
    })
}

/// How long the dubbed line may last: the cue's speech, without running into the next cue.
fn target_duration(start: f64, end: f64, next_start: f64) -> f64 {
    let speech = (end - start - 0.2).max(0.5);
    speech.min((next_start - start - 0.05).max(0.4))
}

/// Speaks every cue of `track` with the media's voice and mixes it over the background.
/// Returns a full-length WAV on the original timeline, so cuts apply as usual on export.
#[allow(clippy::too_many_arguments)]
pub async fn dub_track(
    ctx: &AnalyzeContext,
    paths: &VoicePaths,
    worker: &mut Option<VoiceWorker>,
    mv: &MediaVoice,
    track: &SubtitleTrack,
    duration: f64,
    media_dir: &Path,
) -> Result<PathBuf> {
    let dir = media_dir.join("voice");
    let mut h = Sha256::new();
    h.update(&mv.reference.text);
    for c in &track.cues {
        h.update(format!("{:.3}|{:.3}|{}\n", c.start, c.end, c.text));
    }
    let key: String = h
        .finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect();
    let out = dir.join(format!("dub-{}-{key}.wav", track.language));
    if out.is_file() {
        return Ok(out);
    }

    let clips = dir.join(format!("clips-{key}"));
    std::fs::create_dir_all(&clips)?;
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    for (i, c) in track.cues.iter().enumerate() {
        let text = c.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        let next = track.cues.get(i + 1).map(|n| n.start).unwrap_or(duration);
        lines.push(Line {
            text,
            language: Some(track.language.clone()),
            duration: Some(target_duration(c.start, c.end, next)),
            out: clips.join(format!("{:05}.wav", c.id)),
        });
        starts.push(c.start);
    }
    if lines.is_empty() {
        bail!("The subtitle track has no text to speak.");
    }
    let progress = (ctx.stage)("speak");
    speak(paths, worker, &mv.reference, &lines, &progress, &ctx.cancel).await?;

    let mix_progress = (ctx.stage)("mix");
    let voice_track = dir.join(format!("voice-{key}.wav"));
    let placed: Vec<(f64, PathBuf)> = starts
        .into_iter()
        .zip(lines.iter().map(|l| l.out.clone()))
        .collect();
    let vt = voice_track.clone();
    tokio::task::spawn_blocking(move || assemble(&placed, duration, &vt)).await??;
    mix_progress(0.5);

    // Match the new voice's loudness to the original speech, then lay it over the background.
    let gain = (mean_volume(ctx, &mv.vocals).await? - mean_volume(ctx, &voice_track).await?)
        .clamp(-12.0, 12.0);
    let mut args = ffmpeg_args(&["-y", "-v", "error", "-i"]);
    args.push(path_arg(&mv.background));
    args.push("-i".into());
    args.push(path_arg(&voice_track));
    args.push("-filter_complex".into());
    args.push(format!(
        "[1:a]aresample=44100,aformat=channel_layouts=stereo,volume={gain:.2}dB[v];[0:a][v]amix=inputs=2:normalize=0:duration=first[a]"
    ));
    args.extend(ffmpeg_args(&["-map", "[a]", "-c:a", "pcm_s16le"]));
    args.push(path_arg(&out));
    media::run_ffmpeg(&ctx.tools, &args, duration, None, &ctx.cancel)
        .await
        .context("mixing the dub")?;
    let _ = std::fs::remove_dir_all(&clips);
    let _ = std::fs::remove_file(&voice_track);
    mix_progress(1.0);
    Ok(out)
}

/// Places mono clips at their start times on a silent track of `duration` seconds.
fn assemble(clips: &[(f64, PathBuf)], duration: f64, out: &Path) -> Result<()> {
    const SR: u32 = 24_000;
    let fade = (SR as f64 * 0.008) as usize;
    let mut track = vec![0f32; (duration * SR as f64).ceil() as usize + 1];
    for (start, path) in clips {
        let mut reader =
            hound::WavReader::open(path).with_context(|| format!("reading {}", path.display()))?;
        let spec = reader.spec();
        if spec.sample_rate != SR {
            bail!(
                "unexpected sample rate {} in {}",
                spec.sample_rate,
                path.display()
            );
        }
        let ch = spec.channels.max(1) as usize;
        let raw: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / scale))
                    .collect::<Result<_, _>>()?
            }
        };
        let mono: Vec<f32> = raw
            .chunks(ch)
            .map(|f| f.iter().sum::<f32>() / ch as f32)
            .collect();
        let offset = (start * SR as f64).round().max(0.0) as usize;
        let n = mono.len();
        for (i, s) in mono.iter().enumerate() {
            let Some(slot) = track.get_mut(offset + i) else {
                break;
            };
            let ramp = (i.min(n - 1 - i) as f32 / fade.max(1) as f32).min(1.0);
            *slot += s * ramp;
        }
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(out, spec)?;
    for s in track {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}

/// Average loudness in dB (ffmpeg volumedetect).
async fn mean_volume(ctx: &AnalyzeContext, path: &Path) -> Result<f64> {
    let mut args = ffmpeg_args(&["-v", "info", "-i"]);
    args.push(path_arg(path));
    args.extend(ffmpeg_args(&["-af", "volumedetect", "-f", "null", "-"]));
    let log = media::run_ffmpeg(&ctx.tools, &args, 0.0, None, &ctx.cancel).await?;
    log.lines()
        .find_map(|l| l.split("mean_volume:").nth(1))
        .and_then(|v| v.trim().trim_end_matches("dB").trim().parse().ok())
        .ok_or_else(|| anyhow!("could not measure loudness of {}", path.display()))
}

pub fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dub_lines_fit_their_cue_and_never_overlap_the_next() {
        // Cue 0.0–3.3 (with 0.3 s hold), next cue at 3.1.
        assert!((target_duration(0.0, 3.3, 3.1) - 3.05).abs() < 1e-9);
        // Plenty of room: speech length (cue minus hold-ish margin).
        assert!((target_duration(10.0, 13.0, 20.0) - 2.8).abs() < 1e-9);
        // Very short cue still gets a speakable minimum.
        assert!(target_duration(5.0, 5.2, 9.0) >= 0.4);
    }

    #[test]
    fn assembles_clips_at_their_times() {
        let dir = std::env::temp_dir().join(format!("nuai-assemble-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("c.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&clip, spec).unwrap();
        for _ in 0..2400 {
            w.write_sample(16_000i16).unwrap();
        }
        w.finalize().unwrap();
        let out = dir.join("t.wav");
        assemble(&[(1.0, clip)], 2.0, &out).unwrap();
        let samples: Vec<f32> = hound::WavReader::open(&out)
            .unwrap()
            .samples::<f32>()
            .map(|s| s.unwrap())
            .collect();
        assert_eq!(samples.len(), 48_001);
        assert_eq!(samples[23_000], 0.0, "silent before the clip");
        assert!(samples[24_000 + 1200] > 0.4, "clip placed at 1.0 s");
        assert_eq!(samples[30_000], 0.0, "silent after the clip");
        let _ = std::fs::remove_dir_all(dir);
    }
}
