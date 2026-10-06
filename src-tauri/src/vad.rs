//! Speech detection with Silero VAD (via whisper.cpp). Unlike a loudness threshold it
//! tells speech from music and noise, so videos with a music bed don't lose quiet
//! sentence endings to the silence cutter.

use std::path::Path;

use anyhow::{anyhow, Result};
use whisper_rs::{WhisperVadContext, WhisperVadContextParams};

use crate::audio::FRAME_SEC;
use crate::media::ANALYSIS_RATE;

/// Silero VAD v5.1.2 for whisper.cpp (MIT), bundled so analysis works offline.
const MODEL: &[u8] = include_bytes!("../resources/vad/ggml-silero-v5.1.2.bin");
const MODEL_FILE: &str = "ggml-silero-v5.1.2.bin";

/// Level used in speech envelopes: speech frames sit above it, everything else below.
pub const SPEECH_THRESHOLD_DB: f32 = -50.0;

/// Speech probability per VAD window and the window length in seconds.
pub fn speech_probabilities(pcm: &[f32], models_dir: &Path) -> Result<(Vec<f32>, f64)> {
    let path = models_dir.join(MODEL_FILE);
    if std::fs::metadata(&path)
        .map(|m| m.len() as usize != MODEL.len())
        .unwrap_or(true)
    {
        std::fs::create_dir_all(models_dir)?;
        std::fs::write(&path, MODEL)?;
    }
    let mut params = WhisperVadContextParams::new();
    params.set_n_threads(
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(8) as i32,
    );
    params.set_use_gpu(false);
    let mut ctx = WhisperVadContext::new(&path.to_string_lossy(), params)
        .map_err(|e| anyhow!("could not load the speech detector: {e:?}"))?;
    ctx.detect_speech(pcm)
        .map_err(|e| anyhow!("speech detection failed: {e:?}"))?;
    let probs = ctx.probabilities().to_vec();
    if probs.is_empty() {
        return Err(anyhow!("speech detection returned nothing"));
    }
    let window = pcm.len() as f64 / probs.len() as f64 / ANALYSIS_RATE as f64;
    Ok((probs, window))
}

/// A 10 ms "envelope" where speech is 0 dB and non-speech -100 dB, so the silence,
/// retiming and hallucination logic written for loudness envelopes can use it directly
/// with [`SPEECH_THRESHOLD_DB`]. `threshold` is the speech probability cut-off.
pub fn speech_envelope(probs: &[f32], window: f64, frames: usize, threshold: f32) -> Vec<f32> {
    (0..frames)
        .map(|f| {
            let t = (f as f64 + 0.5) * FRAME_SEC;
            let i = ((t / window) as usize).min(probs.len() - 1);
            if probs[i] >= threshold {
                0.0
            } else {
                -100.0
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_windows_onto_10ms_frames() {
        // 32 ms windows: speech in the second window only.
        let env = speech_envelope(&[0.1, 0.9, 0.2], 0.032, 9, 0.5);
        assert_eq!(
            env,
            vec![-100.0, -100.0, -100.0, 0.0, 0.0, 0.0, -100.0, -100.0, -100.0]
        );
    }
}
