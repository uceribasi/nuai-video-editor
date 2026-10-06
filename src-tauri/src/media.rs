//! ffprobe / ffmpeg wrappers: probing, audio extraction, preview proxies, freeze detection.

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::task::{Cancel, Progress};
use crate::tools::{command, Tools};

/// Sample rate used for all audio analysis (what whisper expects).
pub const ANALYSIS_RATE: u32 = 16_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub path: String,
    pub file_name: String,
    pub duration: f64,
    pub size: u64,
    pub has_video: bool,
    pub has_audio: bool,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub container: String,
    pub bit_rate: Option<u64>,
    pub video_bit_rate: Option<u64>,
    /// Whether the system webview can play the file directly.
    pub playable: bool,
}

fn parse_rate(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let n: f64 = n.parse().ok()?;
    let d: f64 = d.parse().ok()?;
    (d > 0.0 && n > 0.0).then(|| n / d)
}

fn json_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_u64())
}

fn json_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_f64())
}

pub async fn probe(tools: &Tools, path: &Path) -> Result<MediaInfo> {
    let out = command(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .await
        .context("running ffprobe")?;
    if !out.status.success() {
        bail!(
            "ffprobe could not read the file: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing ffprobe output")?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let format = &v["format"];

    // Attached pictures (cover art) are reported as video streams; skip them.
    let video = streams.iter().find(|s| {
        s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0
    });
    let audio = streams.iter().find(|s| s["codec_type"] == "audio");

    let duration = json_f64(&format["duration"])
        .or_else(|| video.and_then(|s| json_f64(&s["duration"])))
        .or_else(|| audio.and_then(|s| json_f64(&s["duration"])))
        .ok_or_else(|| anyhow!("could not determine media duration"))?;

    let fps = video
        .and_then(|s| {
            let avg = s["avg_frame_rate"].as_str().and_then(parse_rate);
            let r = s["r_frame_rate"].as_str().and_then(parse_rate);
            // Variable-frame-rate screen recordings report absurd r_frame_rate values.
            match (avg, r) {
                (Some(a), _) if a > 1.0 && a <= 240.0 => Some(a),
                (_, Some(r)) if r > 1.0 && r <= 240.0 => Some(r),
                _ => None,
            }
        })
        .unwrap_or(30.0);

    let container = format["format_name"].as_str().unwrap_or("").to_string();
    let video_codec = video
        .and_then(|s| s["codec_name"].as_str())
        .map(str::to_string);
    let audio_codec = audio
        .and_then(|s| s["codec_name"].as_str())
        .map(str::to_string);

    // Phone videos are often stored sideways with a rotation flag; ffmpeg and the
    // webview both apply it, so report the size as displayed.
    let rotation = video.map(rotation_of).unwrap_or(0);
    let (mut width, mut height) = (
        video.and_then(|s| s["width"].as_u64()).unwrap_or(0) as u32,
        video.and_then(|s| s["height"].as_u64()).unwrap_or(0) as u32,
    );
    if rotation.rem_euclid(180) == 90 {
        std::mem::swap(&mut width, &mut height);
    }

    let mp4_like = container.contains("mp4") || container.contains("mov");
    let video_ok = match video_codec.as_deref() {
        Some("hevc") => cfg!(target_os = "macos"),
        other => matches!(other, None | Some("h264")),
    };
    let audio_ok = matches!(
        audio_codec.as_deref(),
        None | Some("aac") | Some("mp3") | Some("alac")
    );

    Ok(MediaInfo {
        path: path.to_string_lossy().into_owned(),
        file_name: path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        duration,
        size: json_u64(&format["size"]).unwrap_or(0),
        has_video: video.is_some(),
        has_audio: audio.is_some(),
        width,
        height,
        fps,
        video_codec,
        audio_codec,
        container,
        bit_rate: json_u64(&format["bit_rate"]),
        video_bit_rate: video.and_then(|s| json_u64(&s["bit_rate"])),
        playable: mp4_like && video_ok && audio_ok,
    })
}

fn rotation_of(stream: &serde_json::Value) -> i64 {
    let from_side_data = stream["side_data_list"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|d| d["rotation"].as_f64().or_else(|| json_f64(&d["rotation"])));
    from_side_data
        .or_else(|| json_f64(&stream["tags"]["rotate"]))
        .map(|r| r.round() as i64)
        .unwrap_or(0)
}

/// Names of the encoders this ffmpeg build offers.
pub async fn available_encoders(tools: &Tools) -> Result<HashSet<String>> {
    let out = command(&tools.ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .await
        .context("listing ffmpeg encoders")?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let flags = parts.next()?;
            let name = parts.next()?;
            (flags.len() == 6 && (flags.starts_with('V') || flags.starts_with('A')))
                .then(|| name.to_string())
        })
        .collect())
}

/// Runs ffmpeg with `-progress` reporting, honouring cancellation.
/// Returns the captured stderr (useful for filters that log their results).
pub async fn run_ffmpeg(
    tools: &Tools,
    args: &[String],
    duration: f64,
    progress: Option<&Progress>,
    cancel: &Cancel,
) -> Result<String> {
    let mut child = command(&tools.ffmpeg)
        .args(["-hide_banner", "-nostats", "-progress", "pipe:1"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting ffmpeg")?;

    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut ticker = tokio::time::interval(Duration::from_millis(150));
    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line? {
                    Some(line) => {
                        if let (Some(p), Some(v)) = (progress, line.strip_prefix("out_time_us=")) {
                            if let Ok(us) = v.trim().parse::<f64>() {
                                if duration > 0.0 {
                                    p((us / 1e6 / duration).clamp(0.0, 1.0));
                                }
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = ticker.tick() => {
                if cancel.is_cancelled() {
                    let _ = child.kill().await;
                    bail!("Cancelled");
                }
            }
        }
    }

    let status = child.wait().await?;
    let stderr = stderr_task.await.unwrap_or_default();
    cancel.check()?;
    if !status.success() {
        let tail: Vec<&str> = stderr.lines().rev().take(8).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        bail!("ffmpeg failed: {}", tail.join("\n"));
    }
    Ok(stderr)
}

/// Decodes the first audio track to 16 kHz mono f32 samples.
pub async fn extract_pcm(
    tools: &Tools,
    path: &Path,
    duration: f64,
    progress: &Progress,
    cancel: &Cancel,
) -> Result<Vec<f32>> {
    let mut child = command(&tools.ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-vn", "-ac", "1", "-ar"])
        .arg(ANALYSIS_RATE.to_string())
        .args(["-f", "f32le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting ffmpeg for audio extraction")?;

    let mut stdout = child.stdout.take().expect("piped stdout");
    let expected = (duration * ANALYSIS_RATE as f64 * 4.0).max(1.0);
    let mut bytes: Vec<u8> = Vec::with_capacity(expected as usize + 4096);
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        cancel.check()?;
        let n = stdout.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        progress((bytes.len() as f64 / expected).min(1.0));
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        bail!(
            "could not decode audio: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

/// Builds a small, seek-friendly H.264 copy for preview when the webview can't play the source.
pub async fn make_proxy(
    tools: &Tools,
    encoders: &HashSet<String>,
    info: &MediaInfo,
    out: &Path,
    progress: &Progress,
    cancel: &Cancel,
) -> Result<()> {
    let tmp = out.with_extension("partial.mp4");
    let mut args: Vec<String> = vec!["-y".into(), "-i".into(), info.path.clone()];
    if info.has_video {
        args.extend(["-map".into(), "0:v:0".into()]);
        args.extend([
            "-vf".into(),
            "scale=-2:'min(720,ih)':flags=bilinear,format=yuv420p".into(),
        ]);
        args.extend(proxy_video_args(encoders));
        args.extend(["-g".into(), "24".into()]);
    }
    if info.has_audio {
        args.extend([
            "-map".into(),
            "0:a:0".into(),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "128k".into(),
        ]);
    }
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        tmp.to_string_lossy().into_owned(),
    ]);
    run_ffmpeg(tools, &args, info.duration, Some(progress), cancel).await?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}

fn proxy_video_args(encoders: &HashSet<String>) -> Vec<String> {
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    if encoders.contains("h264_videotoolbox") {
        v(&["-c:v", "h264_videotoolbox", "-b:v", "2500k"])
    } else if encoders.contains("libx264") {
        v(&["-c:v", "libx264", "-preset", "veryfast", "-crf", "26"])
    } else if encoders.contains("libopenh264") {
        v(&["-c:v", "libopenh264", "-b:v", "2500k"])
    } else {
        v(&["-c:v", "mpeg4", "-q:v", "5"])
    }
}

/// Finds stretches where the picture doesn't change (frozen screen, static slide, paused game).
pub async fn detect_freezes(
    tools: &Tools,
    info: &MediaInfo,
    min_duration: f64,
    progress: &Progress,
    cancel: &Cancel,
) -> Result<Vec<(f64, f64)>> {
    if !info.has_video {
        return Ok(Vec::new());
    }
    let args: Vec<String> = vec![
        "-v".into(),
        "info".into(),
        "-i".into(),
        info.path.clone(),
        "-map".into(),
        "0:v:0".into(),
        "-vf".into(),
        format!("scale=320:-2,freezedetect=n=-55dB:d={min_duration:.2}"),
        "-an".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ];
    let log = run_ffmpeg(tools, &args, info.duration, Some(progress), cancel).await?;
    Ok(parse_freezes(&log, info.duration))
}

fn parse_freezes(log: &str, duration: f64) -> Vec<(f64, f64)> {
    let value = |line: &str, key: &str| -> Option<f64> {
        let idx = line.find(key)?;
        line[idx + key.len()..]
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    };
    let mut out = Vec::new();
    let mut start: Option<f64> = None;
    for line in log.lines() {
        if let Some(s) = value(line, "lavfi.freezedetect.freeze_start:") {
            start = Some(s);
        } else if let Some(e) = value(line, "lavfi.freezedetect.freeze_end:") {
            if let Some(s) = start.take() {
                out.push((s, e));
            }
        }
    }
    // A freeze that runs to the end of the file has no end marker.
    if let Some(s) = start {
        out.push((s, duration));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_freeze_log() {
        let log = "[freezedetect @ 0x1] lavfi.freezedetect.freeze_start: 2.5\n\
                   [freezedetect @ 0x1] lavfi.freezedetect.freeze_duration: 3\n\
                   [freezedetect @ 0x1] lavfi.freezedetect.freeze_end: 5.5\n\
                   [freezedetect @ 0x1] lavfi.freezedetect.freeze_start: 9\n";
        assert_eq!(parse_freezes(log, 12.0), vec![(2.5, 5.5), (9.0, 12.0)]);
    }

    #[test]
    fn parses_rates() {
        assert_eq!(
            parse_rate("30000/1001").map(|r| (r * 100.0).round()),
            Some(2997.0)
        );
        assert_eq!(parse_rate("0/0"), None);
    }
}
