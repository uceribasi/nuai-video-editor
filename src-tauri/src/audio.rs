//! Loudness envelope, silence detection and waveform peaks from 16 kHz mono PCM.

use crate::media::ANALYSIS_RATE;

/// Envelope resolution: one value per 10 ms.
pub const FRAME_SEC: f64 = 0.01;
const FRAME_LEN: usize = (ANALYSIS_RATE as f64 * FRAME_SEC) as usize;
const FLOOR_DB: f32 = -100.0;

/// Waveform values sent to the UI per second of media.
pub const WAVEFORM_RATE: f64 = 50.0;

/// RMS level of each 10 ms frame in dBFS.
pub fn envelope(pcm: &[f32]) -> Vec<f32> {
    pcm.chunks(FRAME_LEN)
        .map(|frame| {
            let sum: f32 = frame.iter().map(|s| s * s).sum();
            let rms = (sum / frame.len().max(1) as f32).sqrt();
            if rms <= 1e-5 {
                FLOOR_DB
            } else {
                (20.0 * rms.log10()).max(FLOOR_DB)
            }
        })
        .collect()
}

fn percentile(values: &[f32], p: f64) -> f32 {
    if values.is_empty() {
        return FLOOR_DB;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let idx = ((v.len() - 1) as f64 * p).round() as usize;
    v[idx]
}

/// Picks a silence threshold from the recording itself: well below the speech level,
/// but clear of the noise floor so room tone and hum count as silence.
pub fn auto_threshold(env: &[f32]) -> f32 {
    let speech = percentile(env, 0.95);
    let floor = percentile(env, 0.10);
    let mut thr = (speech - 26.0).max(floor + 4.0);
    thr = thr.min(speech - 8.0);
    thr.max(-75.0)
}

/// Silent stretches of at least `min_duration` seconds, in seconds.
pub fn silent_regions(env: &[f32], threshold_db: f32, min_duration: f64) -> Vec<(f64, f64)> {
    let mut silent: Vec<bool> = env.iter().map(|&db| db < threshold_db).collect();

    // Clicks, keyboard taps and mouth noises shorter than ~60 ms shouldn't break a pause.
    const MAX_BLIP: usize = 6;
    let mut i = 0;
    while i < silent.len() {
        if silent[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < silent.len() && !silent[i] {
            i += 1;
        }
        let bounded = start > 0 && i < silent.len();
        if bounded && i - start <= MAX_BLIP {
            silent[start..i].iter_mut().for_each(|s| *s = true);
        }
    }

    let min_frames = (min_duration / FRAME_SEC).ceil() as usize;
    let mut out = Vec::new();
    let mut i = 0;
    while i < silent.len() {
        if !silent[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < silent.len() && silent[i] {
            i += 1;
        }
        if i - start >= min_frames.max(1) {
            out.push((start as f64 * FRAME_SEC, i as f64 * FRAME_SEC));
        }
    }
    out
}

/// Moves `t` to the quietest frame within `radius` seconds so cuts land between sounds.
pub fn snap_to_quiet(env: &[f32], t: f64, radius: f64) -> f64 {
    if env.is_empty() {
        return t;
    }
    let center = (t / FRAME_SEC).round() as isize;
    let r = (radius / FRAME_SEC).round() as isize;
    let lo = (center - r).max(0) as usize;
    let hi = ((center + r).max(0) as usize).min(env.len() - 1);
    if lo > hi {
        return t;
    }
    let min = env[lo..=hi].iter().copied().fold(f32::INFINITY, f32::min);
    // Among frames within 1 dB of the quietest, take the one nearest the original point.
    let best = (lo..=hi)
        .filter(|&i| env[i] <= min + 1.0)
        .min_by_key(|&i| (i as isize - center).abs())
        .unwrap_or(lo);
    // Land in the middle of the 10 ms frame.
    best as f64 * FRAME_SEC + FRAME_SEC / 2.0
}

/// Peak-per-bucket waveform in 0..1 (dB-scaled so quiet speech stays visible).
pub fn waveform(env: &[f32]) -> Vec<f32> {
    let per_bucket = ((1.0 / WAVEFORM_RATE) / FRAME_SEC).round().max(1.0) as usize;
    let top = percentile(env, 0.995);
    let range = 48.0;
    env.chunks(per_bucket)
        .map(|c| {
            let peak = c.iter().copied().fold(FLOOR_DB, f32::max);
            ((peak - (top - range)) / range).clamp(0.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_from(spec: &[(f32, usize)]) -> Vec<f32> {
        spec.iter()
            .flat_map(|&(db, n)| std::iter::repeat_n(db, n))
            .collect()
    }

    #[test]
    fn finds_long_pauses_only() {
        // speech 1s, pause 0.3s, speech 1s, pause 1s, speech 0.5s
        let env = env_from(&[
            (-20.0, 100),
            (-70.0, 30),
            (-20.0, 100),
            (-70.0, 100),
            (-20.0, 50),
        ]);
        let regions = silent_regions(&env, -50.0, 0.5);
        assert_eq!(regions.len(), 1);
        assert!((regions[0].0 - 2.3).abs() < 1e-9);
        assert!((regions[0].1 - 3.3).abs() < 1e-9);
    }

    #[test]
    fn ignores_short_clicks_inside_pauses() {
        let env = env_from(&[
            (-20.0, 50),
            (-70.0, 40),
            (-15.0, 3),
            (-70.0, 40),
            (-20.0, 50),
        ]);
        let regions = silent_regions(&env, -50.0, 0.5);
        assert_eq!(regions.len(), 1);
        assert!((regions[0].1 - regions[0].0 - 0.83).abs() < 1e-9);
    }

    #[test]
    fn auto_threshold_sits_between_floor_and_speech() {
        let env = env_from(&[(-18.0, 500), (-65.0, 500)]);
        let thr = auto_threshold(&env);
        assert!(thr > -65.0 && thr < -18.0, "{thr}");
    }

    #[test]
    fn snaps_to_quietest_frame() {
        let env = env_from(&[(-20.0, 10), (-60.0, 1), (-20.0, 10)]);
        let t = snap_to_quiet(&env, 0.08, 0.05);
        assert!((t - 0.105).abs() < 1e-9, "{t}");
    }
}
