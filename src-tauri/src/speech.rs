//! Transcript model and rule-based speech cleanup: retakes, false starts, stutters, fillers.
//! These run without any AI; when an AI provider is configured it reviews the retake
//! candidates instead of trusting them blindly.

use serde::{Deserialize, Serialize};

use crate::edit::{Cut, CutKind, CutSource};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub prob: f32,
}

/// A phrase: words between pauses or sentence ends. The unit the AI refers to by id.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Utterance {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub language: String,
    pub model: String,
    pub utterances: Vec<Utterance>,
}

/// Pause between words that starts a new phrase.
const PHRASE_PAUSE: f64 = 0.45;
const MAX_PHRASE_WORDS: usize = 40;

fn ends_sentence(word: &str) -> bool {
    let w = word.trim_end_matches(['"', '\'', ')', '»', '”']);
    w.ends_with('.') || w.ends_with('?') || w.ends_with('!') || w.ends_with('…')
}

/// Groups a flat word stream into phrases.
pub fn build_utterances(words: Vec<Word>) -> Vec<Utterance> {
    let mut groups: Vec<Vec<Word>> = Vec::new();
    let mut current: Vec<Word> = Vec::new();
    for w in words {
        if let Some(prev) = current.last() {
            let split = w.start - prev.end >= PHRASE_PAUSE
                || ends_sentence(&prev.text)
                || current.len() >= MAX_PHRASE_WORDS;
            if split {
                groups.push(std::mem::take(&mut current));
            }
        }
        current.push(w);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(i, words)| Utterance {
            id: format!("u{}", i + 1),
            start: words.first().map(|w| w.start).unwrap_or(0.0),
            end: words.last().map(|w| w.end).unwrap_or(0.0),
            text: words
                .iter()
                .map(|w| w.text.trim())
                .collect::<Vec<_>>()
                .join(" "),
            words,
        })
        .collect()
}

/// Lowercased letters/digits only; dotless/dotted i are folded together.
pub fn normalize(word: &str) -> String {
    word.to_lowercase()
        .chars()
        .map(|c| if c == 'ı' { 'i' } else { c })
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn tokens(u: &Utterance) -> Vec<String> {
    u.words
        .iter()
        .map(|w| normalize(&w.text))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Same word, or one is a cut-off start of the other ("anlat-" / "anlatacağım").
pub fn fuzzy_eq(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.chars().count() <= b.chars().count() {
        (a, b)
    } else {
        (b, a)
    };
    short.chars().count() >= 3 && long.starts_with(short)
}

fn lcs_len(a: &[String], b: &[String]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    let mut cur = vec![0usize; b.len() + 1];
    for x in a {
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = if fuzzy_eq(x, y) {
                prev[j] + 1
            } else {
                prev[j + 1].max(cur[j])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn common_prefix(a: &[String], b: &[String]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| fuzzy_eq(x, y)).count()
}

fn preview(text: &str) -> String {
    let t: String = text.chars().take(60).collect();
    if text.chars().count() > 60 {
        format!("{t}…")
    } else {
        t
    }
}

/// A phrase that is started again shortly afterwards is an abandoned take.
/// The cut runs from the abandoned phrase up to the start of the new attempt.
pub fn detect_retakes(utts: &[Utterance]) -> Vec<Cut> {
    const LOOKAHEAD: usize = 3;
    const MAX_GAP: f64 = 20.0;
    let toks: Vec<Vec<String>> = utts.iter().map(tokens).collect();
    let mut cuts = Vec::new();
    let mut covered_until = 0usize;

    for i in 0..utts.len() {
        if i < covered_until || toks[i].is_empty() {
            continue;
        }
        let a = &toks[i];
        for j in (i + 1)..utts.len().min(i + 1 + LOOKAHEAD) {
            if utts[j].start - utts[i].end > MAX_GAP {
                break;
            }
            let b = &toks[j];
            if b.is_empty() {
                continue;
            }
            let prefix = common_prefix(a, b);
            let between_penalty = 0.85f32.powi((j - i - 1) as i32);

            // False start: a short fragment that the next phrase begins with.
            let false_start = a.len() <= 4 && b.len() > a.len() && prefix == a.len();
            // Retake: most of the phrase is said again, starting the same way.
            let lcs = lcs_len(a, b);
            let cover = lcs as f32 / a.len() as f32;
            let retake = a.len() >= 3 && lcs >= 3 && cover >= 0.6 && prefix >= 2;

            if false_start || retake {
                let confidence = if false_start {
                    if a.len() == 1 {
                        0.65
                    } else {
                        0.85
                    }
                } else {
                    0.5 + 0.45 * cover
                } * between_penalty;
                let next = preview(&utts[j].text);
                let (code, reason) = if false_start {
                    (
                        "falseStart",
                        format!("False start, restarted as \"{next}\""),
                    )
                } else {
                    ("retake", format!("Said again later: \"{next}\""))
                };
                cuts.push(
                    Cut::new(
                        CutKind::Retake,
                        CutSource::Rule,
                        utts[i].start,
                        utts[j].start,
                        confidence,
                        reason,
                    )
                    .coded(code, next),
                );
                covered_until = j;
                break;
            }
        }
    }
    cuts
}

/// Immediately repeated word groups inside a phrase ("bu fonksiyon bu fonksiyon").
/// Single repeated words are proposed with low confidence because many languages
/// repeat words on purpose ("yavaş yavaş", "very very").
pub fn detect_stutters(utts: &[Utterance]) -> Vec<Cut> {
    let mut cuts = Vec::new();
    for u in utts {
        let toks: Vec<String> = u.words.iter().map(|w| normalize(&w.text)).collect();
        let mut p = 0;
        while p < toks.len() {
            let mut found = None;
            for n in (1..=4).rev() {
                if p + 2 * n > toks.len() {
                    continue;
                }
                let first = &toks[p..p + n];
                let second = &toks[p + n..p + 2 * n];
                if first.iter().any(String::is_empty) {
                    continue;
                }
                if first.iter().zip(second).all(|(x, y)| x == y) {
                    found = Some(n);
                    break;
                }
            }
            match found {
                Some(n) => {
                    let start = u.words[p].start;
                    let end = u.words[p + n].start;
                    let gap = u.words[p + n].start - u.words[p + n - 1].end;
                    let confidence = if n >= 2 {
                        0.75
                    } else if gap >= 0.2 {
                        0.6
                    } else {
                        0.4
                    };
                    let phrase = u.words[p..p + n]
                        .iter()
                        .map(|w| w.text.trim())
                        .collect::<Vec<_>>()
                        .join(" ");
                    cuts.push(
                        Cut::new(
                            CutKind::Stutter,
                            CutSource::Rule,
                            start,
                            end,
                            confidence,
                            format!("Repeated \"{phrase}\""),
                        )
                        .coded("stutter", phrase),
                    );
                    p += n;
                }
                None => p += 1,
            }
        }
    }
    cuts
}

const FILLERS: &[&str] = &[
    // English and widely shared hesitation sounds
    "um", "umm", "uhm", "uh", "uhh", "erm", "er", "ah", "ahh", "eh", "ehh", "hm", "hmm", "hmmm",
    "mm", "mmm", // Turkish (after ı→i folding)
    "ii", "iii", "iiii", "ee", "eee", "eeee", "ihm", "öö", "ööö", "ehm",
    // German / French / Spanish
    "äh", "ähm", "öhm", "euh", "bah", "este", "eeh",
];

fn is_filler(token: &str) -> Option<f32> {
    if token.is_empty() {
        return None;
    }
    if FILLERS.contains(&token) {
        return Some(0.85);
    }
    // Drawn-out single sounds: "eeee", "mmmm", "ıııı".
    let mut chars = token.chars();
    let first = chars.next()?;
    let len = token.chars().count();
    let vowelish = "aeiouöüäm".contains(first);
    (len >= 3 && vowelish && token.chars().all(|c| c == first)).then_some(0.75)
}

pub fn detect_fillers(utts: &[Utterance]) -> Vec<Cut> {
    utts.iter()
        .flat_map(|u| u.words.iter())
        .filter_map(|w| {
            let conf = is_filler(&normalize(&w.text))?;
            let word = w
                .text
                .trim()
                .trim_matches(|c: char| c.is_ascii_punctuation());
            Some(
                Cut::new(
                    CutKind::Filler,
                    CutSource::Rule,
                    w.start,
                    w.end,
                    conf,
                    format!("Filler \"{word}\""),
                )
                .coded("filler", word),
            )
        })
        .collect()
}

/// Prompt that nudges whisper into writing hesitations down instead of skipping them.
pub fn filler_prompt(language: &str) -> Option<&'static str> {
    match language {
        "tr" => Some("Eee, şey, ııı... yani, hmm, tamam. Şimdi, ee, başlayalım."),
        "en" | "auto" => {
            Some("Umm, let me think like, hmm... Okay, here's what I'm, like, thinking.")
        }
        "de" => Some("Ähm, also, äh... ich meine, hmm, okay."),
        "fr" => Some("Euh, alors, bah... enfin, hmm, d'accord."),
        "es" => Some("Eh, este, pues... o sea, mmm, vale."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds words from "text@start" pairs, each word 0.3 s long.
    fn words(spec: &str, start: f64) -> Vec<Word> {
        spec.split_whitespace()
            .enumerate()
            .map(|(i, t)| Word {
                text: t.to_string(),
                start: start + i as f64 * 0.35,
                end: start + i as f64 * 0.35 + 0.3,
                prob: 0.9,
            })
            .collect()
    }

    fn utts(phrases: &[(&str, f64)]) -> Vec<Utterance> {
        build_utterances(phrases.iter().flat_map(|(t, s)| words(t, *s)).collect())
    }

    #[test]
    fn splits_phrases_on_pauses_and_punctuation() {
        let u = utts(&[
            ("Merhaba arkadaşlar.", 0.0),
            ("Bugün React", 0.8),
            ("anlatacağım", 3.0),
        ]);
        assert_eq!(u.len(), 3);
        assert_eq!(u[0].text, "Merhaba arkadaşlar.");
        assert_eq!(u[1].id, "u2");
    }

    #[test]
    fn detects_retake_and_keeps_last_take() {
        let u = utts(&[
            ("Bugün size React hook'larını nasıl", 0.0),
            (
                "Bugün size React hook'larını nasıl kullanacağınızı anlatacağım.",
                4.0,
            ),
            ("İlk olarak useState ile başlayalım.", 9.0),
        ]);
        let cuts = detect_retakes(&u);
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].start, u[0].start);
        assert_eq!(cuts[0].end, u[1].start);
        assert!(cuts[0].enabled);
    }

    #[test]
    fn detects_false_start_with_cut_off_word() {
        let u = utts(&[
            ("Şimdi bu anla", 0.0),
            ("Şimdi bu anlattığım yöntemi deneyelim.", 2.0),
        ]);
        let cuts = detect_retakes(&u);
        assert_eq!(cuts.len(), 1);
        assert!(cuts[0].reason.starts_with("False start"));
    }

    #[test]
    fn similar_but_different_sentences_are_not_retakes() {
        let u = utts(&[
            ("Sonra ayarlar menüsüne gidiyoruz ve kaydet diyoruz.", 0.0),
            ("Sonra dosya menüsüne gidiyoruz ve kapat diyoruz.", 5.0),
        ]);
        assert!(detect_retakes(&u).is_empty());
    }

    #[test]
    fn detects_multi_word_stutter() {
        let u = utts(&[("bu fonksiyon bu fonksiyon iki parametre alır", 0.0)]);
        let cuts = detect_stutters(&u);
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].start, u[0].words[0].start);
        assert_eq!(cuts[0].end, u[0].words[2].start);
        assert!(cuts[0].enabled);
    }

    #[test]
    fn single_word_reduplication_starts_disabled() {
        let u = utts(&[("yavaş yavaş ilerliyoruz", 0.0)]);
        let cuts = detect_stutters(&u);
        assert_eq!(cuts.len(), 1);
        assert!(!cuts[0].enabled);
    }

    #[test]
    fn detects_fillers_but_not_short_real_words() {
        let u = utts(&[("Eee, o zaman ııı başlayalım um", 0.0)]);
        let found: Vec<String> = detect_fillers(&u)
            .iter()
            .map(|c| c.reason.clone())
            .collect();
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found.iter().all(|r| !r.contains("\"o\"")));
    }

    #[test]
    fn normalizes_turkish_i() {
        assert_eq!(normalize("İstanbul'a"), "istanbula");
        assert_eq!(normalize("ILIK"), "ilik");
        assert_eq!(normalize("ılık"), "ilik");
    }
}
