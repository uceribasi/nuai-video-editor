//! Local speech-to-text with whisper.cpp, producing word-level timestamps.

use std::path::Path;

use anyhow::{anyhow, Result};
use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext,
    WhisperContextParameters,
};

use crate::speech::Word;
use crate::task::{Cancel, Progress};

pub struct LoadedModel {
    pub path: String,
    pub ctx: WhisperContext,
}

/// Alignment-head preset for DTW word timing, guessed from the ggml file name.
fn dtw_preset(path: &Path) -> Option<DtwModelPreset> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let en = name.contains(".en");
    Some(if name.contains("large-v3-turbo") {
        DtwModelPreset::LargeV3Turbo
    } else if name.contains("large-v3") {
        DtwModelPreset::LargeV3
    } else if name.contains("large-v2") {
        DtwModelPreset::LargeV2
    } else if name.contains("large") {
        DtwModelPreset::LargeV1
    } else if name.contains("medium") {
        if en {
            DtwModelPreset::MediumEn
        } else {
            DtwModelPreset::Medium
        }
    } else if name.contains("small") {
        if en {
            DtwModelPreset::SmallEn
        } else {
            DtwModelPreset::Small
        }
    } else if name.contains("base") {
        if en {
            DtwModelPreset::BaseEn
        } else {
            DtwModelPreset::Base
        }
    } else if name.contains("tiny") {
        if en {
            DtwModelPreset::TinyEn
        } else {
            DtwModelPreset::Tiny
        }
    } else {
        return None;
    })
}

pub fn load_model(path: &Path, use_gpu: bool) -> Result<LoadedModel> {
    // Route whisper.cpp / ggml logging away from stdout.
    whisper_rs::install_logging_hooks();
    let mut params = WhisperContextParameters::default();
    params.use_gpu(use_gpu);
    if let Some(preset) = dtw_preset(path) {
        // DTW gives much better word onsets than plain token timestamps; whisper.cpp
        // disables it when flash attention is on.
        params.flash_attn = false;
        params.dtw_parameters(DtwParameters {
            mode: DtwMode::ModelPreset {
                model_preset: preset,
            },
            ..DtwParameters::default()
        });
    }
    let ctx = WhisperContext::new_with_params(path, params)
        .map_err(|e| anyhow!("could not load speech model {}: {e:?}", path.display()))?;
    Ok(LoadedModel {
        path: path.to_string_lossy().into_owned(),
        ctx,
    })
}

pub struct RawTranscript {
    pub language: String,
    pub words: Vec<Word>,
}

/// Transcribes 16 kHz mono samples. Blocking; run it on a blocking thread.
pub fn transcribe(
    model: &LoadedModel,
    pcm: &[f32],
    language: &str,
    initial_prompt: Option<&str>,
    progress: Progress,
    cancel: Cancel,
) -> Result<RawTranscript> {
    let mut state = model
        .ctx
        .create_state()
        .map_err(|e| anyhow!("could not create whisper state: {e:?}"))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);
    params.set_n_threads(threads as i32);
    params.set_language(if language == "auto" {
        None
    } else {
        Some(language)
    });
    params.set_detect_language(false);
    params.set_token_timestamps(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    params.set_no_speech_thold(0.6);
    if let Some(prompt) = initial_prompt {
        params.set_initial_prompt(prompt);
    }
    let progress_cb = progress.clone();
    params.set_progress_callback_safe(move |p: i32| progress_cb(p as f64 / 100.0));
    // whisper-rs 0.16 casts the callback's user data to `*mut F` but stores a
    // `*mut Box<dyn FnMut() -> bool>`; passing that box type as F makes the two agree.
    let cancel_cb = cancel.clone();
    let abort: Box<dyn FnMut() -> bool> = Box::new(move || cancel_cb.is_cancelled());
    params.set_abort_callback_safe::<_, Box<dyn FnMut() -> bool>>(Some(abort));

    let result = state.full(params, pcm);
    cancel.check()?;
    result.map_err(|e| anyhow!("transcription failed: {e:?}"))?;

    let eot = model.ctx.token_eot();
    let lang_id = state.full_lang_id_from_state();
    let detected = whisper_rs::get_lang_str(lang_id)
        .unwrap_or("auto")
        .to_string();

    let mut words: Vec<Word> = Vec::new();
    for segment in state.as_iter() {
        if segment.no_speech_probability() > 0.8 {
            continue;
        }
        // Tokens are byte pieces: a Turkish "ş" can be split across two of them,
        // so bytes are collected per word and decoded once the word is complete.
        let mut bytes: Vec<u8> = Vec::new();
        let mut start = 0.0f64;
        let mut end = 0.0f64;
        let mut probs: Vec<f32> = Vec::new();
        let mut flush = |bytes: &mut Vec<u8>, start: f64, end: f64, probs: &mut Vec<f32>| {
            let text = String::from_utf8_lossy(bytes).trim().to_string();
            if !text.is_empty() {
                let prob = probs.iter().sum::<f32>() / probs.len().max(1) as f32;
                words.push(Word {
                    text,
                    start,
                    end: end.max(start + 0.02),
                    prob,
                });
            }
            bytes.clear();
            probs.clear();
        };

        for i in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(i) else {
                continue;
            };
            if token.token_id() >= eot {
                continue; // timestamps, language and control tokens
            }
            let Ok(piece) = token.to_bytes() else {
                continue;
            };
            if piece.is_empty() {
                continue;
            }
            let data = token.token_data();
            // Prefer the DTW onset when available.
            let t0 = if data.t_dtw >= 0 {
                data.t_dtw as f64 / 100.0
            } else {
                data.t0 as f64 / 100.0
            };
            let t1 = data.t1 as f64 / 100.0;
            let starts_word = piece[0] == b' ' || bytes.is_empty();
            let is_punct = piece.iter().all(|b| b.is_ascii_punctuation());
            if starts_word && !bytes.is_empty() && !is_punct {
                flush(&mut bytes, start, end, &mut probs);
            }
            if bytes.is_empty() {
                start = t0;
            }
            bytes.extend_from_slice(piece);
            if !is_punct {
                end = t1;
            }
            probs.push(data.p);
        }
        flush(&mut bytes, start, end, &mut probs);
    }

    // Token timestamps can overlap slightly; keep them monotonic.
    for i in 1..words.len() {
        if words[i].start < words[i - 1].end {
            let mid = (words[i].start + words[i - 1].end) / 2.0;
            words[i - 1].end = mid.max(words[i - 1].start);
            words[i].start = mid.min(words[i].end);
        }
    }

    Ok(RawTranscript {
        language: detected,
        words,
    })
}
