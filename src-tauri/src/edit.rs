//! The edit decision list: cuts, merging them, and turning them into kept segments.

use serde::{Deserialize, Serialize};

use crate::audio;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CutKind {
    /// Pause with no speech.
    Silence,
    /// An earlier attempt at a sentence that is said again later.
    Retake,
    /// "um", "uh", "ııı"...
    Filler,
    /// Immediately repeated words ("the the", "bu fonksiyon bu fonksiyon").
    Stutter,
    /// Frozen picture with no speech.
    Static,
    /// Meta talk / slips flagged by the AI ("sorry, let me start over").
    Mistake,
    /// Off-topic content flagged by the AI.
    Offtopic,
    /// Removed by the AI to reach a target duration.
    Shorten,
    /// Added by the user.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CutSource {
    Rule,
    Ai,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cut {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub kind: CutKind,
    /// Free text shown to the user (AI cuts are written in the UI language).
    pub reason: String,
    /// For rule-based cuts: a message key the UI localizes, with `detail` as its argument.
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub detail: String,
    pub confidence: f32,
    pub enabled: bool,
    pub source: CutSource,
}

/// Cuts below this confidence are proposed but start disabled.
pub const ENABLE_THRESHOLD: f32 = 0.6;
/// Kept fragments shorter than this would only flash on screen, so they are dropped.
const MIN_KEEP: f64 = 0.12;

impl Cut {
    pub fn new(
        kind: CutKind,
        source: CutSource,
        start: f64,
        end: f64,
        confidence: f32,
        reason: impl Into<String>,
    ) -> Self {
        Cut {
            id: String::new(),
            start,
            end,
            kind,
            reason: reason.into(),
            code: String::new(),
            detail: String::new(),
            confidence,
            enabled: confidence >= ENABLE_THRESHOLD,
            source,
        }
    }

    /// Attaches a localizable message key (see `reason.*` in the UI translations).
    pub fn coded(mut self, code: &str, detail: impl Into<String>) -> Self {
        self.code = code.to_string();
        self.detail = detail.into();
        self
    }

    pub fn len(&self) -> f64 {
        self.end - self.start
    }
}

/// Gives every cut a stable, readable id ("retake-3").
pub fn assign_ids(cuts: &mut [Cut]) {
    let mut counters = std::collections::HashMap::new();
    for c in cuts.iter_mut() {
        let n = counters.entry(c.kind).or_insert(0u32);
        *n += 1;
        let kind = serde_json::to_value(c.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        c.id = format!("{kind}-{n}");
    }
}

/// Turns silent regions into cuts, leaving `padding` seconds of air next to speech.
pub fn silence_cuts(regions: &[(f64, f64)], padding: f64, duration: f64) -> Vec<Cut> {
    regions
        .iter()
        .filter_map(|&(s, e)| {
            let start = if s <= 0.001 { 0.0 } else { s + padding };
            let end = if e >= duration - 0.02 {
                duration
            } else {
                e - padding
            };
            (end - start >= 0.1).then(|| {
                Cut::new(
                    CutKind::Silence,
                    CutSource::Rule,
                    start,
                    end,
                    0.95,
                    format!("{:.1}s pause", e - s),
                )
                .coded("pause", format!("{:.1}", e - s))
            })
        })
        .collect()
}

/// Moves word-derived cut edges onto the quietest nearby frame so no syllable is clipped.
pub fn snap_to_audio(cut: &mut Cut, env: &[f32]) {
    const RADIUS: f64 = 0.12;
    let start = audio::snap_to_quiet(env, cut.start, RADIUS);
    let end = audio::snap_to_quiet(env, cut.end, RADIUS);
    if end - start > 0.05 {
        cut.start = start;
        cut.end = end;
    }
}

/// Union of enabled cut ranges, clamped to the media and with tiny keeps absorbed.
pub fn merged_cuts(cuts: &[Cut], duration: f64) -> Vec<(f64, f64)> {
    let mut ranges: Vec<(f64, f64)> = cuts
        .iter()
        .filter(|c| c.enabled)
        .map(|c| (c.start.max(0.0), c.end.min(duration)))
        .filter(|(s, e)| e > s)
        .collect();
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 + MIN_KEEP => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    // Absorb a sliver of content left at the very start or end.
    if let Some(first) = merged.first_mut() {
        if first.0 < MIN_KEEP {
            first.0 = 0.0;
        }
    }
    if let Some(last) = merged.last_mut() {
        if duration - last.1 < MIN_KEEP {
            last.1 = duration;
        }
    }
    merged
}

/// The parts of the media that survive the edit.
pub fn keep_segments(cuts: &[Cut], duration: f64) -> Vec<(f64, f64)> {
    let mut keeps = Vec::new();
    let mut t = 0.0;
    for (s, e) in merged_cuts(cuts, duration) {
        if s > t {
            keeps.push((t, s));
        }
        t = e;
    }
    if duration > t {
        keeps.push((t, duration));
    }
    keeps.retain(|(s, e)| e - s >= MIN_KEEP);
    keeps
}

pub fn edited_duration(cuts: &[Cut], duration: f64) -> f64 {
    keep_segments(cuts, duration)
        .iter()
        .map(|(s, e)| e - s)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cut(s: f64, e: f64) -> Cut {
        Cut::new(CutKind::Silence, CutSource::Rule, s, e, 1.0, "")
    }

    #[test]
    fn merges_overlapping_and_adjacent_cuts() {
        let cuts = vec![cut(5.0, 6.0), cut(1.0, 2.0), cut(1.5, 3.0), cut(3.05, 4.0)];
        assert_eq!(merged_cuts(&cuts, 10.0), vec![(1.0, 4.0), (5.0, 6.0)]);
    }

    #[test]
    fn keeps_are_complement_of_cuts() {
        let cuts = vec![cut(0.0, 1.0), cut(4.0, 5.0), cut(9.0, 10.0)];
        assert_eq!(keep_segments(&cuts, 10.0), vec![(1.0, 4.0), (5.0, 9.0)]);
        assert!((edited_duration(&cuts, 10.0) - 7.0).abs() < 1e-9);
    }

    #[test]
    fn disabled_cuts_are_ignored() {
        let mut c = cut(2.0, 3.0);
        c.enabled = false;
        assert_eq!(keep_segments(&[c], 5.0), vec![(0.0, 5.0)]);
    }

    #[test]
    fn silence_cuts_leave_padding_except_at_edges() {
        let cuts = silence_cuts(&[(0.0, 2.0), (5.0, 6.0), (9.0, 10.0)], 0.15, 10.0);
        assert_eq!(cuts.len(), 3);
        assert_eq!((cuts[0].start, cuts[0].end), (0.0, 1.85));
        assert!((cuts[1].start - 5.15).abs() < 1e-9 && (cuts[1].end - 5.85).abs() < 1e-9);
        assert_eq!(cuts[2].end, 10.0);
    }

    #[test]
    fn ids_are_numbered_per_kind() {
        let mut cuts = vec![cut(0.0, 1.0), cut(2.0, 3.0)];
        cuts[1].kind = CutKind::Retake;
        assign_ids(&mut cuts);
        assert_eq!(cuts[0].id, "silence-1");
        assert_eq!(cuts[1].id, "retake-1");
    }
}
