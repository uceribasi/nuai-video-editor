//! Subtitles from the word-timed transcript: grouping words into readable cues,
//! line wrapping, moving cues onto the edited timeline and writing SRT / WebVTT.

use serde::{Deserialize, Serialize};

use crate::edit::{self, Cut};
use crate::speech::{Utterance, Word};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cue {
    pub id: u32,
    /// Seconds on the original timeline (cues are mapped to the edited one on export).
    pub start: f64,
    pub end: f64,
    /// May contain line breaks.
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubtitleOptions {
    pub max_chars: usize,
    pub max_lines: usize,
    /// Longest time a cue stays on screen.
    pub max_duration: f64,
    pub min_duration: f64,
}

impl Default for SubtitleOptions {
    fn default() -> Self {
        SubtitleOptions {
            max_chars: 42,
            max_lines: 2,
            max_duration: 6.0,
            min_duration: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTrack {
    /// ISO 639-1 code.
    pub language: String,
    pub translated: bool,
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subtitles {
    /// Fingerprint of the cuts the cues were built for; the UI flags stale subtitles.
    pub cuts_signature: String,
    pub tracks: Vec<SubtitleTrack>,
}

/// Scripts written without spaces between words.
pub fn spaceless(language: &str) -> bool {
    matches!(language, "zh" | "ja" | "th" | "lo" | "km" | "my" | "yue")
}

fn joiner(language: &str) -> &'static str {
    if spaceless(language) {
        ""
    } else {
        " "
    }
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn ends_sentence(w: &str) -> bool {
    let w = w.trim_end_matches(['"', '\'', ')', '»', '”']);
    w.ends_with(['.', '?', '!', '…', '。', '？', '！'])
}

fn ends_clause(w: &str) -> bool {
    w.ends_with([',', ';', ':', '，', '、'])
}

fn inside(ranges: &[(f64, f64)], t: f64) -> bool {
    ranges.iter().any(|&(s, e)| t >= s && t < e)
}

/// Groups the words that survive the edit into cues.
pub fn build_cues(
    utts: &[Utterance],
    cuts: &[Cut],
    duration: f64,
    language: &str,
    o: &SubtitleOptions,
) -> Vec<Cue> {
    let removed = edit::merged_cuts(cuts, duration);
    let join = joiner(language);
    let max_len = o.max_chars.max(8) * o.max_lines.max(1);

    let words: Vec<(usize, &Word)> = utts
        .iter()
        .enumerate()
        .flat_map(|(ui, u)| u.words.iter().map(move |w| (ui, w)))
        .filter(|(_, w)| !inside(&removed, (w.start + w.end) / 2.0))
        .collect();

    let mut groups: Vec<Vec<&Word>> = Vec::new();
    let mut cur: Vec<&Word> = Vec::new();
    let mut cur_utt = usize::MAX;
    let mut cur_len = 0usize;
    for (ui, w) in words {
        let wlen = char_len(w.text.trim());
        if let Some(last) = cur.last() {
            let next_len = cur_len + join.len() + wlen;
            let half_full = cur_len * 2 >= max_len;
            let split = next_len > max_len
                || w.end - cur[0].start > o.max_duration
                || w.start - last.end >= 0.8
                || (ends_sentence(&last.text) && cur_len >= 12)
                || (ui != cur_utt && half_full)
                || (ends_clause(&last.text) && cur_len * 10 >= max_len * 7);
            if split {
                groups.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
        }
        cur_len += if cur.is_empty() {
            wlen
        } else {
            join.len() + wlen
        };
        cur.push(w);
        cur_utt = ui;
    }
    if !cur.is_empty() {
        groups.push(cur);
    }

    let mut cues: Vec<Cue> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let text = g
                .iter()
                .map(|w| w.text.trim())
                .collect::<Vec<_>>()
                .join(join);
            Cue {
                id: i as u32 + 1,
                start: g[0].start,
                // Keep the text up a little after the last word is spoken.
                end: g[g.len() - 1].end + 0.3,
                text: wrap(&text, o.max_chars, o.max_lines, language),
            }
        })
        .collect();
    fix_timing(&mut cues, o.min_duration);
    cues
}

/// No overlaps, and short cues stay up long enough to read when there is room.
fn fix_timing(cues: &mut [Cue], min_duration: f64) {
    for i in 0..cues.len() {
        let next_start = cues
            .get(i + 1)
            .map(|c| c.start - 0.04)
            .unwrap_or(f64::INFINITY);
        let c = &mut cues[i];
        if c.end - c.start < min_duration {
            c.end = c.start + min_duration;
        }
        c.end = c.end.min(next_start).max(c.start + 0.1);
    }
}

/// Breaks text into at most `max_lines` balanced lines of about `max_chars`.
pub fn wrap(text: &str, max_chars: usize, max_lines: usize, language: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat = if spaceless(language) {
        text.replace('\n', "")
    } else {
        flat
    };
    if max_lines <= 1 || char_len(&flat) <= max_chars {
        return flat;
    }
    let units: Vec<String> = if spaceless(language) {
        flat.chars().map(|c| c.to_string()).collect()
    } else {
        flat.split(' ').map(str::to_string).collect()
    };
    let join = joiner(language);
    let line = |part: &[String]| part.join(join);

    if max_lines == 2 {
        // Pick the split that balances the two lines, preferring breaks after punctuation.
        let mut best: Option<(f64, usize)> = None;
        for i in 1..units.len() {
            let a = char_len(&line(&units[..i]));
            let b = char_len(&line(&units[i..]));
            let mut score = a.max(b) as f64;
            if ends_sentence(&units[i - 1]) || ends_clause(&units[i - 1]) {
                score -= 4.0;
            }
            if a > max_chars || b > max_chars {
                score += 100.0;
            }
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, i));
            }
        }
        let i = best.map(|(_, i)| i).unwrap_or(units.len());
        return format!("{}\n{}", line(&units[..i]), line(&units[i..]));
    }

    let mut lines: Vec<String> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for u in units {
        let candidate = if cur.is_empty() {
            u.clone()
        } else {
            format!("{}{join}{u}", line(&cur))
        };
        if !cur.is_empty() && char_len(&candidate) > max_chars && lines.len() + 1 < max_lines {
            lines.push(line(&cur));
            cur.clear();
        }
        cur.push(u);
    }
    lines.push(line(&cur));
    lines.join("\n")
}

/// Re-wraps cue text, e.g. after translation changed its length.
pub fn rewrap(cues: &[Cue], o: &SubtitleOptions, language: &str) -> Vec<Cue> {
    cues.iter()
        .map(|c| Cue {
            text: wrap(&c.text, o.max_chars, o.max_lines, language),
            ..c.clone()
        })
        .collect()
}

/// Maps original-timeline cues onto the edited timeline defined by `keeps`.
/// Time inside a removed gap collapses onto the cut point; cues that end up
/// (almost) empty are dropped.
pub fn to_edited(cues: &[Cue], keeps: &[(f64, f64)]) -> Vec<Cue> {
    cues.iter()
        .filter_map(|c| {
            let (s, e) = map_range(keeps, c.start, c.end)?;
            Some(Cue {
                start: s,
                end: e,
                ..c.clone()
            })
        })
        .enumerate()
        .map(|(i, c)| Cue {
            id: i as u32 + 1,
            ..c
        })
        .collect()
}

/// Original → edited time for a range; `None` when (almost) nothing of it is kept.
pub fn map_range(keeps: &[(f64, f64)], start: f64, end: f64) -> Option<(f64, f64)> {
    let map = |t: f64| -> f64 {
        let mut offset = 0.0;
        for &(s, e) in keeps {
            if t < s {
                return offset;
            }
            if t <= e {
                return offset + (t - s);
            }
            offset += e - s;
        }
        offset
    };
    let (s, e) = (map(start), map(end));
    (e - s >= 0.2).then_some((s, e))
}

fn timestamp(t: f64, sep: char) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}{sep}{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

pub fn to_srt(cues: &[Cue]) -> String {
    cues.iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}\n{} --> {}\n{}\n",
                i + 1,
                timestamp(c.start, ','),
                timestamp(c.end, ','),
                c.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn to_vtt(cues: &[Cue]) -> String {
    let body: Vec<String> = cues
        .iter()
        .map(|c| {
            format!(
                "{} --> {}\n{}\n",
                timestamp(c.start, '.'),
                timestamp(c.end, '.'),
                c.text.trim()
            )
        })
        .collect();
    format!("WEBVTT\n\n{}", body.join("\n"))
}

/// (ISO 639-1, ISO 639-2/T, English name) for languages offered in the UI.
const LANGUAGES: &[(&str, &str, &str)] = &[
    ("tr", "tur", "Turkish"),
    ("en", "eng", "English"),
    ("de", "deu", "German"),
    ("fr", "fra", "French"),
    ("es", "spa", "Spanish"),
    ("it", "ita", "Italian"),
    ("pt", "por", "Portuguese"),
    ("nl", "nld", "Dutch"),
    ("pl", "pol", "Polish"),
    ("ru", "rus", "Russian"),
    ("uk", "ukr", "Ukrainian"),
    ("ar", "ara", "Arabic"),
    ("fa", "fas", "Persian"),
    ("hi", "hin", "Hindi"),
    ("ja", "jpn", "Japanese"),
    ("ko", "kor", "Korean"),
    ("zh", "zho", "Chinese (Simplified)"),
    ("az", "aze", "Azerbaijani"),
    ("el", "ell", "Greek"),
    ("he", "heb", "Hebrew"),
    ("id", "ind", "Indonesian"),
    ("ro", "ron", "Romanian"),
    ("sv", "swe", "Swedish"),
    ("cs", "ces", "Czech"),
    ("hu", "hun", "Hungarian"),
    ("vi", "vie", "Vietnamese"),
];

pub fn iso639_2(code: &str) -> &str {
    LANGUAGES
        .iter()
        .find(|l| l.0 == code)
        .map(|l| l.1)
        .unwrap_or("und")
}

pub fn language_name(code: &str) -> String {
    LANGUAGES
        .iter()
        .find(|l| l.0 == code)
        .map(|l| l.2.to_string())
        .unwrap_or_else(|| code.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{CutKind, CutSource};
    use crate::speech::build_utterances;

    fn words(text: &str, start: f64, step: f64) -> Vec<Word> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, t)| Word {
                text: t.into(),
                start: start + i as f64 * step,
                end: start + i as f64 * step + step * 0.8,
                prob: 1.0,
            })
            .collect()
    }

    #[test]
    fn groups_words_into_readable_cues() {
        let mut w = words("Merhaba arkadaşlar.", 0.0, 0.4);
        w.extend(words(
            "Bugün size yeni projemizi tanıtacağım ve nasıl çalıştığını adım adım göstereceğim.",
            1.0,
            0.35,
        ));
        let utts = build_utterances(w);
        let cues = build_cues(&utts, &[], 20.0, "tr", &SubtitleOptions::default());
        assert!(cues.len() >= 2, "{cues:?}");
        assert_eq!(cues[0].text, "Merhaba arkadaşlar.");
        for c in &cues {
            assert!(c.text.lines().count() <= 2);
            assert!(
                c.text.lines().all(|l| l.chars().count() <= 42),
                "{:?}",
                c.text
            );
            assert!(c.end > c.start);
        }
        for pair in cues.windows(2) {
            assert!(pair[0].end <= pair[1].start);
        }
    }

    #[test]
    fn words_inside_cuts_are_left_out() {
        let utts = build_utterances(words("bir iki üç dört beş", 0.0, 0.5));
        let cut = Cut::new(CutKind::Manual, CutSource::User, 0.95, 1.6, 1.0, "");
        let cues = build_cues(&utts, &[cut], 10.0, "en", &SubtitleOptions::default());
        let all: String = cues
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(all, "bir iki dört beş");
    }

    #[test]
    fn wraps_into_two_balanced_lines() {
        let w = wrap(
            "Bu uygulama videolardaki gereksiz kısımları otomatik olarak siliyor",
            42,
            2,
            "tr",
        );
        let lines: Vec<&str> = w.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.chars().count() <= 42));
        let diff = lines[0].chars().count() as i64 - lines[1].chars().count() as i64;
        assert!(diff.abs() <= 12, "{w}");
    }

    #[test]
    fn maps_cues_onto_edited_timeline() {
        // Keep 0-2 and 5-10: three seconds are removed.
        let keeps = [(0.0, 2.0), (5.0, 10.0)];
        let cues = vec![
            Cue {
                id: 1,
                start: 1.0,
                end: 1.8,
                text: "a".into(),
            },
            Cue {
                id: 2,
                start: 2.5,
                end: 4.5,
                text: "gone".into(),
            },
            Cue {
                id: 3,
                start: 6.0,
                end: 7.0,
                text: "b".into(),
            },
        ];
        let e = to_edited(&cues, &keeps);
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].start, e[0].end), (1.0, 1.8));
        assert_eq!((e[1].start, e[1].end), (3.0, 4.0));
        assert_eq!(e[1].id, 2);
    }

    #[test]
    fn writes_srt_and_vtt() {
        let cues = vec![Cue {
            id: 1,
            start: 61.5,
            end: 63.25,
            text: "Merhaba\ndünya".into(),
        }];
        assert_eq!(
            to_srt(&cues),
            "1\n00:01:01,500 --> 00:01:03,250\nMerhaba\ndünya\n"
        );
        assert!(
            to_vtt(&cues).starts_with("WEBVTT\n\n00:01:01.500 --> 00:01:03.250\nMerhaba\ndünya")
        );
    }
}
