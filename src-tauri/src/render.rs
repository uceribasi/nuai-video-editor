//! Renders the kept segments into a single MP4.
//!
//! Each kept segment is encoded on its own (frame-accurate seek, short audio fades so
//! cuts don't click) with PCM audio, then the pieces are joined with the concat demuxer
//! and the audio is encoded once. This keeps memory flat for long videos with hundreds
//! of cuts and lets segments encode in parallel.
//!
//! Subtitles can be embedded as selectable tracks, written next to the video, or burned
//! into the picture. Burned captions arrive as transparent PNGs rendered by the UI and are
//! composited with `overlay`, which every ffmpeg build has (unlike libass).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use base64::Engine;
use futures_util::{stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};

use crate::edit::{self, Cut};
use crate::media::{run_ffmpeg, MediaInfo};
use crate::settings::ExportSettings;
use crate::subtitles::{self, SubtitleTrack};
use crate::task::{Cancel, Progress};
use crate::tools::Tools;

const PARALLEL: usize = 3;
const FADE: f64 = 0.012;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub output: String,
    pub duration: f64,
    pub segments: usize,
    pub subtitle_files: Vec<String>,
}

/// Audio to use instead of, or in addition to, the original soundtrack. Paths are
/// full-length WAVs on the original timeline (e.g. dubs), so cuts apply to them too.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioPlan {
    pub replace: Option<String>,
    /// Language of the replacement track, for the file's metadata.
    pub replace_language: Option<String>,
    pub extra: Vec<AudioTrack>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub path: String,
    pub language: String,
}

/// A caption image (transparent PNG, video width) shown from `start` to `end`
/// on the original timeline.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCue {
    pub start: f64,
    pub end: f64,
    /// Base64 PNG.
    pub png: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubtitlePlan {
    /// Tracks added as selectable subtitle streams.
    pub embed: Vec<SubtitleTrack>,
    /// Tracks written as `<output>.<lang>.srt` next to the video.
    pub sidecar: Vec<SubtitleTrack>,
    /// Captions burned into the picture.
    pub burn: Vec<BurnCue>,
    /// Transparent PNG of the same size as the burn images, shown between captions.
    pub burn_blank: String,
}

/// Picks a free `<stem><suffix>.mp4` next to the source (or in `output_dir`).
pub fn default_output(source: &Path, settings: &ExportSettings) -> PathBuf {
    let dir = if settings.output_dir.trim().is_empty() {
        source.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        PathBuf::from(settings.output_dir.trim())
    };
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "video".into());
    let mut candidate = dir.join(format!("{stem}{}.mp4", settings.suffix));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}{}-{n}.mp4", settings.suffix));
        n += 1;
    }
    candidate
}

fn video_args(
    encoders: &HashSet<String>,
    info: &MediaInfo,
    settings: &ExportSettings,
) -> Vec<String> {
    let hevc = settings.codec == "hevc";
    let (crf_h264, crf_hevc, bpp) = match settings.quality.as_str() {
        "small" => (26, 30, 0.05),
        "balanced" => (22, 26, 0.08),
        _ => (19, 23, 0.12),
    };
    // Bitrate for hardware encoders: follow the source, bounded by a bits-per-pixel budget.
    let budget =
        info.width as f64 * info.height as f64 * info.fps * bpp * if hevc { 0.7 } else { 1.0 };
    let source = info
        .video_bit_rate
        .or(info.bit_rate)
        .map(|b| b as f64)
        .unwrap_or(budget);
    let bitrate = source
        .min(budget * 1.5)
        .max(budget * 0.5)
        .clamp(1_000_000.0, 80_000_000.0);
    let kbps = format!("{}k", (bitrate / 1000.0).round() as u64);

    let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut args = if hevc && encoders.contains("hevc_videotoolbox") {
        s(&["-c:v", "hevc_videotoolbox", "-b:v", &kbps, "-tag:v", "hvc1"])
    } else if hevc && encoders.contains("libx265") {
        s(&[
            "-c:v",
            "libx265",
            "-preset",
            "medium",
            "-crf",
            &crf_hevc.to_string(),
            "-tag:v",
            "hvc1",
        ])
    } else if encoders.contains("h264_videotoolbox") {
        s(&[
            "-c:v",
            "h264_videotoolbox",
            "-b:v",
            &kbps,
            "-profile:v",
            "high",
        ])
    } else if encoders.contains("libx264") {
        s(&[
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            &crf_h264.to_string(),
        ])
    } else if encoders.contains("h264_nvenc") {
        s(&["-c:v", "h264_nvenc", "-b:v", &kbps])
    } else if encoders.contains("h264_qsv") {
        s(&["-c:v", "h264_qsv", "-b:v", &kbps])
    } else if encoders.contains("h264_amf") {
        s(&["-c:v", "h264_amf", "-b:v", &kbps])
    } else if encoders.contains("libopenh264") {
        s(&["-c:v", "libopenh264", "-b:v", &kbps])
    } else {
        s(&["-c:v", "mpeg4", "-q:v", "3"])
    };
    args.extend(s(&["-pix_fmt", "yuv420p", "-fps_mode", "cfr", "-r"]));
    args.push(format!("{:.6}", info.fps));
    args
}

/// The kept segments exactly as `export` renders them. Subtitle timing must use the
/// same list, otherwise per-segment rounding would make captions drift.
pub fn export_keeps(info: &MediaInfo, cuts: &[Cut]) -> Vec<(f64, f64)> {
    let keeps = edit::keep_segments(cuts, info.duration);
    if info.has_video {
        frame_align(&keeps, info.fps)
    } else {
        keeps
    }
}

/// Writes a subtitle track mapped onto the edited timeline.
pub fn write_subtitle_file(track: &SubtitleTrack, keeps: &[(f64, f64)], path: &Path) -> Result<()> {
    let cues = subtitles::to_edited(&track.cues, keeps);
    let text = if path.extension().is_some_and(|e| e == "vtt") {
        subtitles::to_vtt(&cues)
    } else {
        subtitles::to_srt(&cues)
    };
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Per-segment caption playlists for the concat demuxer, in segment-local time.
/// Returns, for each segment, the playlist path when it has captions.
fn prepare_burn(
    plan: &SubtitlePlan,
    keeps: &[(f64, f64)],
    dir: &Path,
) -> Result<Vec<Option<PathBuf>>> {
    let b64 = base64::engine::general_purpose::STANDARD;
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join("blank.png"),
        b64.decode(&plan.burn_blank).context("caption image")?,
    )?;
    let mut mapped: Vec<(usize, f64, f64)> = Vec::new();
    for (i, c) in plan.burn.iter().enumerate() {
        if let Some((s, e)) = subtitles::map_range(keeps, c.start, c.end) {
            std::fs::write(
                dir.join(format!("cap_{i:05}.png")),
                b64.decode(&c.png).context("caption image")?,
            )?;
            mapped.push((i, s, e));
        }
    }

    let mut offset = 0.0;
    let mut lists = Vec::with_capacity(keeps.len());
    for (k, &(start, end)) in keeps.iter().enumerate() {
        let dur = end - start;
        let mut entries: Vec<(String, f64)> = Vec::new();
        let mut t = 0.0;
        for &(i, s, e) in &mapped {
            let (ls, le) = ((s - offset).max(0.0), (e - offset).min(dur));
            if le - ls < 0.05 {
                continue;
            }
            if ls > t + 0.001 {
                entries.push(("blank.png".into(), ls - t));
            }
            entries.push((format!("cap_{i:05}.png"), le - ls));
            t = le;
        }
        offset += dur;
        if entries.is_empty() {
            lists.push(None);
            continue;
        }
        if dur > t + 0.001 {
            entries.push(("blank.png".into(), dur - t));
        }
        let mut text = String::from("ffconcat version 1.0\n");
        for (file, d) in &entries {
            text.push_str(&format!("file '{file}'\nduration {d:.4}\n"));
        }
        // The demuxer ignores the last entry's duration unless the file is repeated.
        text.push_str(&format!("file '{}'\n", entries.last().unwrap().0));
        let list = dir.join(format!("seg_{k:05}.ffconcat"));
        std::fs::write(&list, text)?;
        lists.push(Some(list));
    }
    Ok(lists)
}

/// Rounds segment edges to whole frames so audio and video lengths agree.
fn frame_align(keeps: &[(f64, f64)], fps: f64) -> Vec<(f64, f64)> {
    keeps
        .iter()
        .map(|&(s, e)| ((s * fps).round() / fps, (e * fps).round() / fps))
        .filter(|(s, e)| e - s >= 1.0 / fps)
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn export(
    tools: &Tools,
    encoders: &HashSet<String>,
    info: &MediaInfo,
    cuts: &[Cut],
    output: &Path,
    settings: &ExportSettings,
    subs: &SubtitlePlan,
    audio: &AudioPlan,
    work_dir: &Path,
    progress: &Progress,
    cancel: &Cancel,
) -> Result<ExportResult> {
    let keeps = export_keeps(info, cuts);
    if keeps.is_empty() {
        bail!("Nothing is left to export: every part of the video is cut.");
    }
    std::fs::create_dir_all(work_dir)?;
    let burn_lists = if info.has_video && !subs.burn.is_empty() {
        prepare_burn(subs, &keeps, &work_dir.join("captions"))?
    } else {
        vec![None; keeps.len()]
    };
    let total: f64 = keeps.iter().map(|(s, e)| e - s).sum();
    let done = Arc::new(Mutex::new(0.0f64));
    let vargs = if info.has_video {
        video_args(encoders, info, settings)
    } else {
        Vec::new()
    };

    let specs: Vec<(usize, f64, f64)> = keeps
        .iter()
        .enumerate()
        .map(|(i, &(s, e))| (i, s, e))
        .collect();
    let jobs = specs.into_iter().map(|(i, start, end)| {
        let seg = work_dir.join(format!("seg_{i:05}.mkv"));
        let dur = end - start;
        // All inputs first (ffmpeg applies options to the next file), then outputs.
        let seek = |path: &str| -> Vec<String> {
            vec!["-ss".into(), format!("{start:.6}"), "-t".into(), format!("{dur:.6}"), "-i".into(), path.to_string()]
        };
        let mut args: Vec<String> = vec!["-y".into(), "-v".into(), "error".into()];
        args.extend(seek(&info.path));
        let mut next_input = 1;
        let caption_input = (info.has_video && burn_lists[i].is_some()).then(|| {
            args.extend(["-f", "concat", "-safe", "0", "-i"].map(String::from));
            args.push(burn_lists[i].as_ref().unwrap().to_string_lossy().into_owned());
            next_input += 1;
            next_input - 1
        });
        // Audio streams in order: the main track (original or replacement), then extras.
        let mut audio_maps: Vec<String> = Vec::new();
        if let Some(replace) = &audio.replace {
            args.extend(seek(replace));
            audio_maps.push(format!("{next_input}:a:0"));
            next_input += 1;
        } else if info.has_audio {
            audio_maps.push("0:a:0".into());
        }
        for extra in &audio.extra {
            args.extend(seek(&extra.path));
            audio_maps.push(format!("{next_input}:a:0"));
            next_input += 1;
        }

        if info.has_video {
            if let Some(ci) = caption_input {
                args.extend([
                    "-filter_complex".into(),
                    format!("[{ci}:v]format=rgba[cap];[0:v][cap]overlay=x=(main_w-overlay_w)/2:y=main_h-overlay_h:eof_action=pass:format=auto[v]"),
                    "-map".into(),
                    "[v]".into(),
                ]);
            } else {
                args.extend(["-map".into(), "0:v:0".into()]);
            }
            args.extend(vargs.iter().cloned());
        }
        if !audio_maps.is_empty() {
            let fade = FADE.min(dur / 4.0);
            for m in &audio_maps {
                args.extend(["-map".into(), m.clone()]);
            }
            args.extend([
                "-af".into(),
                format!(
                    "afade=t=in:st=0:d={fade:.4},afade=t=out:st={:.4}:d={fade:.4}",
                    (dur - fade).max(0.0)
                ),
                "-c:a".into(),
                "pcm_s16le".into(),
                "-ar".into(),
                "48000".into(),
                "-ac".into(),
                "2".into(),
            ]);
        }
        args.extend([
            "-map_metadata".into(),
            "-1".into(),
            seg.to_string_lossy().into_owned(),
        ]);
        let done = done.clone();
        let progress = progress.clone();
        async move {
            run_ffmpeg(tools, &args, dur, None, cancel)
                .await
                .with_context(|| format!("encoding segment {} ({start:.2}s–{end:.2}s)", i + 1))?;
            let mut d = done.lock().unwrap();
            *d += dur;
            progress(0.92 * *d / total);
            Ok::<PathBuf, anyhow::Error>(seg)
        }
    });
    let segments: Vec<PathBuf> = stream::iter(jobs).buffered(PARALLEL).try_collect().await?;

    let list = work_dir.join("segments.txt");
    let listing: String = segments
        .iter()
        .map(|p| format!("file '{}'\n", p.to_string_lossy().replace('\'', "'\\''")))
        .collect();
    std::fs::write(&list, listing)?;

    let tmp_out = output.with_extension("partial.mp4");
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list.to_string_lossy().into_owned(),
    ];
    for (k, track) in subs.embed.iter().enumerate() {
        let path = work_dir.join(format!("embed_{k}.srt"));
        write_subtitle_file(track, &keeps, &path)?;
        args.extend(["-i".into(), path.to_string_lossy().into_owned()]);
    }
    args.extend(["-map".into(), "0".into()]);
    for (k, track) in subs.embed.iter().enumerate() {
        args.extend(["-map".into(), format!("{}:0", k + 1)]);
        args.extend([
            format!("-metadata:s:s:{k}"),
            format!("language={}", subtitles::iso639_2(&track.language)),
            format!("-metadata:s:s:{k}"),
            format!("title={}", subtitles::language_name(&track.language)),
        ]);
    }
    if !subs.embed.is_empty() {
        args.extend(["-c:s".into(), "mov_text".into()]);
    }
    if info.has_video {
        args.extend(["-c:v".into(), "copy".into()]);
        if settings.codec == "hevc" {
            args.extend(["-tag:v".into(), "hvc1".into()]);
        }
    }
    let audio_streams = usize::from(info.has_audio || audio.replace.is_some()) + audio.extra.len();
    if audio_streams > 0 {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    }
    // Language tags so players can offer the tracks by name.
    let mut tags: Vec<Option<&str>> = Vec::new();
    if info.has_audio || audio.replace.is_some() {
        tags.push(audio.replace_language.as_deref());
    }
    tags.extend(audio.extra.iter().map(|e| Some(e.language.as_str())));
    for (k, lang) in tags.iter().enumerate() {
        if let Some(lang) = lang.filter(|l| !l.is_empty()) {
            args.extend([
                format!("-metadata:s:a:{k}"),
                format!("language={}", subtitles::iso639_2(lang)),
                format!("-metadata:s:a:{k}"),
                format!("title={}", subtitles::language_name(lang)),
            ]);
        }
    }
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        tmp_out.to_string_lossy().into_owned(),
    ]);
    let concat_progress: Progress = {
        let progress = progress.clone();
        Arc::new(move |p| progress(0.92 + 0.08 * p))
    };
    run_ffmpeg(tools, &args, total, Some(&concat_progress), cancel)
        .await
        .context("joining segments")?;
    std::fs::rename(&tmp_out, output).context("moving the exported file into place")?;
    let _ = std::fs::remove_dir_all(work_dir);

    let mut subtitle_files = Vec::new();
    for track in &subs.sidecar {
        let path = sidecar_path(output, &track.language);
        write_subtitle_file(track, &keeps, &path)?;
        subtitle_files.push(path.to_string_lossy().into_owned());
    }
    progress(1.0);

    Ok(ExportResult {
        output: output.to_string_lossy().into_owned(),
        duration: total,
        segments: keeps.len(),
        subtitle_files,
    })
}

/// `talk_edited.mp4` → `talk_edited.tr.srt`
pub fn sidecar_path(video: &Path, language: &str) -> PathBuf {
    let stem = video
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    video.with_file_name(format!("{stem}.{language}.srt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burn_playlists_are_local_to_each_segment() {
        let dir = std::env::temp_dir().join(format!("nuai-burn-test-{}", std::process::id()));
        let blank = base64::engine::general_purpose::STANDARD.encode(b"png");
        let plan = SubtitlePlan {
            burn: vec![
                BurnCue {
                    start: 1.0,
                    end: 2.0,
                    png: blank.clone(),
                },
                BurnCue {
                    start: 5.5,
                    end: 6.5,
                    png: blank.clone(),
                },
            ],
            burn_blank: blank,
            ..SubtitlePlan::default()
        };
        // Keep 0-3 and 5-8 (edited: 0-3, 3-6).
        let lists = prepare_burn(&plan, &[(0.0, 3.0), (5.0, 8.0)], &dir).unwrap();
        let first = std::fs::read_to_string(lists[0].as_ref().unwrap()).unwrap();
        assert!(first
            .contains("file 'blank.png'\nduration 1.0000\nfile 'cap_00000.png'\nduration 1.0000"));
        let second = std::fs::read_to_string(lists[1].as_ref().unwrap()).unwrap();
        assert!(
            second.contains(
                "file 'blank.png'\nduration 0.5000\nfile 'cap_00001.png'\nduration 1.0000"
            ),
            "{second}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn aligns_to_frames_and_drops_empty() {
        let k = frame_align(&[(0.01, 1.0), (2.0, 2.01)], 25.0);
        assert_eq!(k, vec![(0.0, 1.0)]);
    }

    #[test]
    fn default_output_adds_suffix() {
        let s = ExportSettings::default();
        let out = default_output(Path::new("/nonexistent/dir/talk.mov"), &s);
        assert_eq!(out, PathBuf::from("/nonexistent/dir/talk_edited.mp4"));
    }
}
