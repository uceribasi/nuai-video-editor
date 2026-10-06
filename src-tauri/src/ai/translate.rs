//! Subtitle translation with the configured AI provider. Cues keep their ids and
//! timing; the model only replaces the text, one translation per cue.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};

use super::AiClient;
use crate::subtitles::{language_name, Cue};
use crate::task::{Cancel, Progress};

const CHUNK: usize = 120;
const CONTEXT: usize = 4;

pub struct Translation {
    pub cues: Vec<Cue>,
    /// Cues the model skipped; they keep their original text.
    pub missing: usize,
}

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["cues"],
        "properties": {
            "cues": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "text"],
                    "properties": {
                        "id": { "type": "integer" },
                        "text": { "type": "string" }
                    }
                }
            }
        }
    })
}

fn system_prompt(from: &str, to: &str, instructions: &str) -> String {
    let mut p = format!(
        "You are a professional subtitle translator. Translate subtitle cues from {from} to {to}.

Rules:
- Return exactly one translation for every cue id you are given. Never merge, split, skip or reorder cues.
- Write natural, spoken {to} that reads well as subtitles. Keep it about as long as the original so it can be read in the same time.
- Sentences may continue across cues; translate each cue so the sequence reads naturally.
- Keep names, brands, code, URLs and numbers as they are.
- No explanations, notes or quotation marks around the text.
- Answer with JSON only: {{\"cues\": [{{\"id\": 1, \"text\": \"...\"}}]}}."
    );
    if !instructions.trim().is_empty() {
        p.push_str(&format!(
            "\n\nThe user's instructions (follow them unless they conflict with the format):\n{}",
            instructions.trim()
        ));
    }
    p
}

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    cues: Vec<AnswerCue>,
}

#[derive(Deserialize)]
struct AnswerCue {
    id: u32,
    #[serde(default)]
    text: String,
}

pub async fn translate(
    client: &AiClient,
    cues: &[Cue],
    from: &str,
    to: &str,
    instructions: &str,
    progress: &Progress,
    cancel: &Cancel,
) -> Result<Translation> {
    let (from_name, to_name) = (language_name(from), language_name(to));
    let system = system_prompt(&from_name, &to_name, instructions);
    let schema = schema();
    let mut translated: HashMap<u32, String> = HashMap::new();
    let chunks: Vec<&[Cue]> = cues.chunks(CHUNK).collect();

    for (i, chunk) in chunks.iter().enumerate() {
        progress(i as f64 / chunks.len() as f64);
        let mut user = String::new();
        let start = i * CHUNK;
        if start > 0 {
            user.push_str("Previous cues, for context only (do not return them):\n");
            for c in &cues[start.saturating_sub(CONTEXT)..start] {
                user.push_str(&format!("{}: {}\n", c.id, flat(&c.text)));
            }
            user.push('\n');
        }
        user.push_str(&format!("Cues to translate ({} of them):\n", chunk.len()));
        for c in chunk.iter() {
            user.push_str(&format!("{}: {}\n", c.id, flat(&c.text)));
        }
        let value = client
            .complete_json(&system, &user, "subtitle_translation", &schema, cancel)
            .await?;
        let answer: Answer = serde_json::from_value(value)?;
        let wanted: std::collections::HashSet<u32> = chunk.iter().map(|c| c.id).collect();
        for a in answer.cues {
            if wanted.contains(&a.id) && !a.text.trim().is_empty() {
                translated.insert(a.id, flat(&a.text));
            }
        }
    }
    progress(1.0);

    let mut missing = 0;
    let cues = cues
        .iter()
        .map(|c| match translated.get(&c.id) {
            Some(text) => Cue {
                text: text.clone(),
                ..c.clone()
            },
            None => {
                missing += 1;
                c.clone()
            }
        })
        .collect();
    Ok(Translation { cues, missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_names_languages_and_instructions() {
        let p = system_prompt("Turkish", "English", "Use a casual tone");
        assert!(p.contains("from Turkish to English"));
        assert!(p.contains("Use a casual tone"));
    }
}
