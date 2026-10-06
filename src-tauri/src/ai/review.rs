//! Asks the AI which parts of the transcript to remove and maps the answer back to cuts.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};

use super::AiClient;
use crate::edit::{Cut, CutKind, CutSource};
use crate::speech::{fuzzy_eq, normalize, Utterance};
use crate::task::{Cancel, Progress};

const CHUNK: usize = 300;
const OVERLAP: usize = 10;

pub struct ReviewInput<'a> {
    pub utterances: &'a [Utterance],
    pub transcript_language: &'a str,
    /// Language the reasons should be written in (UI language).
    pub reason_language: &'a str,
    pub duration: f64,
    /// Duration after the automatic (non-AI) cuts.
    pub edited_duration: f64,
    pub target_duration: Option<f64>,
    pub instructions: &'a str,
    /// Rule-based retake/stutter findings for the AI to confirm or reject.
    pub candidates: &'a [Cut],
    pub remove_fillers: bool,
}

pub struct ReviewOutput {
    pub cuts: Vec<Cut>,
    pub notes: Vec<String>,
}

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["cuts", "notes"],
        "properties": {
            "cuts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["from", "to", "phrase", "kind", "reason", "confidence"],
                    "properties": {
                        "from": { "type": "string", "description": "Id of the first utterance to remove, e.g. \"u12\"." },
                        "to": { "type": "string", "description": "Id of the last utterance to remove; same as from for one utterance." },
                        "phrase": { "type": "string", "description": "Exact words to remove inside a single utterance (from = to). Empty string removes the whole utterance range." },
                        "kind": { "type": "string", "enum": ["retake", "mistake", "filler", "stutter", "offtopic", "shorten"] },
                        "reason": { "type": "string", "description": "A few words explaining the cut." },
                        "confidence": { "type": "number", "description": "0 to 1." }
                    }
                }
            },
            "notes": { "type": "string", "description": "One or two sentences about the edit, for the user." }
        }
    })
}

fn system_prompt(reason_language: &str) -> String {
    format!(
        "You are the editing brain of nuai, a video editor. You receive the transcript of a video as \
numbered utterances with start-end timestamps in seconds. Long pauses are already removed \
automatically. Decide which utterances or phrases to remove so the video is tight and clean, \
without changing its meaning or removing anything the viewer needs.

Remove:
- retake: an earlier attempt at a sentence that is said again later (false starts, abandoned or repeated takes). Keep the best take, usually the last complete one. Include short meta words between the attempts.
- mistake: slips and talk not meant for the audience (\"sorry\", \"let me start over\", \"cut that\", \"pardon\", \"baştan alıyorum\", \"kes\").
- filler: hesitation words that add nothing (um, uh, ııı, eee, \"şey\" or \"like\" used as hesitation). Use phrase cuts for these.
- stutter: words repeated by accident.
- offtopic / shorten: only when the user asks for a shorter video, gives a target duration or says so in the instructions.

Rules:
- Refer to utterances only by their ids. Never invent ids or timestamps.
- To remove whole utterances set from/to and phrase to \"\". To remove words inside ONE utterance set from = to and phrase to the exact words as written in the transcript.
- Do not cut repetition used on purpose: emphasis, lists, parallel sentences, reduplication such as Turkish \"yavaş yavaş\".
- Candidates from the automatic detector are hints: include them only if they are really retakes or stutters.
- If unsure, still propose the cut with confidence 0.3-0.6 so the user can review it; use 0.8 or more only for clear cases.
- The transcript may contain recognition errors; judge by meaning.
- Write \"reason\" and \"notes\" in this language: {reason_language}.
- The user's extra instructions, if any, take priority over these defaults (but never over the answer format).
- Answer with JSON only, matching the schema: {{\"cuts\": [{{\"from\", \"to\", \"phrase\", \"kind\", \"reason\", \"confidence\"}}], \"notes\"}}."
    )
}

fn fmt_utterances(utts: &[Utterance]) -> String {
    utts.iter()
        .map(|u| format!("{} [{:.1}-{:.1}] {}", u.id, u.start, u.end, u.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Utterance ids overlapping a time range.
fn ids_in(utts: &[Utterance], start: f64, end: f64) -> Vec<&str> {
    utts.iter()
        .filter(|u| u.end > start + 0.01 && u.start < end - 0.01)
        .map(|u| u.id.as_str())
        .collect()
}

fn user_prompt(input: &ReviewInput, chunk: &[Utterance], part: Option<(usize, usize)>) -> String {
    let mut p = String::new();
    p.push_str(&format!(
        "Transcript language: {}\n",
        input.transcript_language
    ));
    p.push_str(&format!(
        "Video duration: {:.0}s, after automatic cuts: {:.0}s\n",
        input.duration, input.edited_duration
    ));
    if let Some((i, n)) = part {
        p.push_str(&format!(
            "This is part {i} of {n} of the transcript; judge this part on its own.\n"
        ));
    }
    if let Some(target) = input.target_duration {
        let chunk_span: f64 = chunk.iter().map(|u| u.end - u.start).sum();
        let total_span: f64 = input
            .utterances
            .iter()
            .map(|u| u.end - u.start)
            .sum::<f64>()
            .max(1.0);
        let remove = ((input.edited_duration - target) * chunk_span / total_span).max(0.0);
        if remove > 1.0 {
            p.push_str(&format!(
                "Target: the final video should be about {target:.0}s. Remove about {remove:.0}s from this transcript \
(beyond retakes and mistakes) as kind \"shorten\", choosing the least important content. Keep the intro, \
the key points and the conclusion, and keep the story coherent.\n"
            ));
        }
    }
    if !input.remove_fillers {
        p.push_str("Do not remove filler words.\n");
    }
    if !input.instructions.trim().is_empty() {
        p.push_str(&format!(
            "User instructions:\n{}\n",
            input.instructions.trim()
        ));
    }
    let hints: Vec<String> = input
        .candidates
        .iter()
        .filter_map(|c| {
            let ids = ids_in(chunk, c.start, c.end);
            let (first, last) = (ids.first()?, ids.last()?);
            let range = if first == last {
                first.to_string()
            } else {
                format!("{first}-{last}")
            };
            let kind = if c.kind == CutKind::Stutter {
                "stutter"
            } else {
                "retake"
            };
            Some(format!("- {range}: {kind}: {}", c.reason))
        })
        .collect();
    if !hints.is_empty() {
        p.push_str("Automatic detector candidates:\n");
        p.push_str(&hints.join("\n"));
        p.push('\n');
    }
    p.push_str("\nTranscript:\n");
    p.push_str(&fmt_utterances(chunk));
    p
}

#[derive(Deserialize)]
struct AiCut {
    from: String,
    to: String,
    #[serde(default)]
    phrase: String,
    kind: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    confidence: f32,
}

#[derive(Deserialize)]
struct AiAnswer {
    #[serde(default)]
    cuts: Vec<AiCut>,
    #[serde(default)]
    notes: String,
}

fn kind_of(s: &str) -> CutKind {
    match s {
        "retake" => CutKind::Retake,
        "filler" => CutKind::Filler,
        "stutter" => CutKind::Stutter,
        "offtopic" => CutKind::Offtopic,
        "shorten" => CutKind::Shorten,
        _ => CutKind::Mistake,
    }
}

/// Locates `phrase` inside an utterance and returns its time span.
fn find_phrase(u: &Utterance, phrase: &str) -> Option<(f64, f64)> {
    let needle: Vec<String> = phrase
        .split_whitespace()
        .map(normalize)
        .filter(|t| !t.is_empty())
        .collect();
    if needle.is_empty() || needle.len() > u.words.len() {
        return None;
    }
    let hay: Vec<String> = u.words.iter().map(|w| normalize(&w.text)).collect();
    (0..=hay.len() - needle.len())
        .find(|&k| needle.iter().zip(&hay[k..]).all(|(a, b)| fuzzy_eq(a, b)))
        .map(|k| (u.words[k].start, u.words[k + needle.len() - 1].end))
}

fn to_cuts(answer: AiAnswer, utts: &[Utterance]) -> Vec<Cut> {
    let index: HashMap<&str, usize> = utts
        .iter()
        .enumerate()
        .map(|(i, u)| (u.id.as_str(), i))
        .collect();
    answer
        .cuts
        .into_iter()
        .filter_map(|c| {
            let a = *index.get(c.from.trim())?;
            let b = *index.get(c.to.trim()).unwrap_or(&a);
            let (a, b) = (a.min(b), a.max(b));
            let (start, end) = if !c.phrase.trim().is_empty() && a == b {
                find_phrase(&utts[a], &c.phrase)?
            } else {
                (utts[a].start, utts[b].end)
            };
            let mut cut = Cut::new(
                kind_of(&c.kind),
                CutSource::Ai,
                start,
                end,
                c.confidence.clamp(0.0, 1.0),
                c.reason,
            );
            if cut.reason.is_empty() {
                cut.reason = c.kind;
            }
            Some(cut)
        })
        .collect()
}

pub async fn review(
    client: &AiClient,
    input: &ReviewInput<'_>,
    progress: &Progress,
    cancel: &Cancel,
) -> Result<ReviewOutput> {
    let utts = input.utterances;
    let mut chunks: Vec<&[Utterance]> = Vec::new();
    if utts.len() <= CHUNK + CHUNK / 5 {
        chunks.push(utts);
    } else {
        let mut start = 0;
        while start < utts.len() {
            let end = (start + CHUNK).min(utts.len());
            chunks.push(&utts[start..end]);
            if end == utts.len() {
                break;
            }
            start = end - OVERLAP;
        }
    }

    let system = system_prompt(input.reason_language);
    let schema = schema();
    let mut cuts = Vec::new();
    let mut notes = Vec::new();
    let n = chunks.len();
    for (i, chunk) in chunks.iter().enumerate() {
        progress(i as f64 / n as f64);
        let part = (n > 1).then_some((i + 1, n));
        let user = user_prompt(input, chunk, part);
        let value = client
            .complete_json(&system, &user, "edit_decisions", &schema, cancel)
            .await?;
        let answer: AiAnswer = serde_json::from_value(value)?;
        if !answer.notes.trim().is_empty() {
            notes.push(answer.notes.trim().to_string());
        }
        cuts.extend(to_cuts(answer, utts));
    }
    progress(1.0);

    // Overlapping chunks can report the same cut twice.
    cuts.sort_by(|a, b| a.start.total_cmp(&b.start));
    cuts.dedup_by(|b, a| (a.start - b.start).abs() < 0.05 && (a.end - b.end).abs() < 0.05);
    Ok(ReviewOutput { cuts, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::{build_utterances, Word};

    fn sample() -> Vec<Utterance> {
        let words = [
            "Merhaba",
            "arkadaşlar.",
            "Bugün",
            "şey",
            "React",
            "anlatacağım.",
        ]
        .iter()
        .enumerate()
        .map(|(i, t)| Word {
            text: t.to_string(),
            start: i as f64,
            end: i as f64 + 0.8,
            prob: 0.9,
        })
        .collect();
        build_utterances(words)
    }

    #[test]
    fn maps_whole_utterance_and_phrase_cuts() {
        let utts = sample();
        let answer = AiAnswer {
            cuts: vec![
                AiCut {
                    from: "u1".into(),
                    to: "u1".into(),
                    phrase: "".into(),
                    kind: "mistake".into(),
                    reason: "x".into(),
                    confidence: 0.9,
                },
                AiCut {
                    from: "u2".into(),
                    to: "u2".into(),
                    phrase: "şey".into(),
                    kind: "filler".into(),
                    reason: "".into(),
                    confidence: 0.5,
                },
                AiCut {
                    from: "u9".into(),
                    to: "u9".into(),
                    phrase: "".into(),
                    kind: "retake".into(),
                    reason: "".into(),
                    confidence: 1.0,
                },
            ],
            notes: String::new(),
        };
        let cuts = to_cuts(answer, &utts);
        assert_eq!(cuts.len(), 2, "unknown ids are ignored");
        assert_eq!((cuts[0].start, cuts[0].end), (0.0, 1.8));
        assert_eq!((cuts[1].start, cuts[1].end), (3.0, 3.8));
        assert_eq!(cuts[1].kind, CutKind::Filler);
        assert!(!cuts[1].enabled);
        assert_eq!(cuts[1].source, CutSource::Ai);
    }

    #[test]
    fn prompt_lists_ids_and_candidates() {
        let utts = sample();
        let candidates = vec![Cut::new(
            CutKind::Retake,
            CutSource::Rule,
            0.0,
            2.0,
            0.9,
            "Said again later",
        )];
        let input = ReviewInput {
            utterances: &utts,
            transcript_language: "tr",
            reason_language: "Turkish",
            duration: 10.0,
            edited_duration: 8.0,
            target_duration: Some(4.0),
            instructions: "Girişi koru",
            candidates: &candidates,
            remove_fillers: true,
        };
        let p = user_prompt(&input, &utts, None);
        assert!(p.contains("u1 [0.0-1.8] Merhaba arkadaşlar."));
        assert!(p.contains("- u1: retake: Said again later"));
        assert!(p.contains("Girişi koru"));
        assert!(p.contains("about 4s"));
    }
}
