//! OpenAI (Responses API) and any OpenAI-compatible server (Chat Completions):
//! OpenRouter, Gemini's OpenAI endpoint, Ollama, LM Studio, vLLM...

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::{http_error, ModelInfo};

fn is_reasoning_model(model: &str) -> bool {
    model.starts_with("gpt-5")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

#[allow(clippy::too_many_arguments)]
pub async fn complete_responses(
    http: &reqwest::Client,
    base_url: &str,
    key: &str,
    model: &str,
    effort: &str,
    system: &str,
    user: &str,
    schema_name: &str,
    schema: &Value,
) -> Result<String> {
    let mut body = json!({
        "model": model,
        "instructions": system,
        "input": user,
        "store": false,
        "text": { "format": { "type": "json_schema", "name": schema_name, "schema": schema, "strict": true } },
    });
    if is_reasoning_model(model) && !effort.is_empty() {
        body["reasoning"] = json!({ "effort": effort });
    }

    let send = |body: Value| {
        http.post(format!("{base_url}/responses"))
            .bearer_auth(key)
            .json(&body)
            .send()
    };
    let mut resp = send(body.clone()).await?;
    if resp.status() == reqwest::StatusCode::BAD_REQUEST {
        let first_error = http_error(resp).await;
        let mut plain = body.clone();
        if let Some(o) = plain.as_object_mut() {
            o.remove("reasoning");
            o.insert(
                "text".into(),
                json!({ "format": { "type": "json_object" } }),
            );
        }
        resp = send(plain).await?;
        if !resp.status().is_success() {
            return Err(first_error);
        }
    }
    if !resp.status().is_success() {
        return Err(http_error(resp).await);
    }

    let v: Value = resp.json().await?;
    if v["status"] == "incomplete" {
        bail!(
            "The AI answer was incomplete ({}).",
            v["incomplete_details"]["reason"]
                .as_str()
                .unwrap_or("unknown reason")
        );
    }
    let mut text = String::new();
    for item in v["output"].as_array().into_iter().flatten() {
        if item["type"] != "message" {
            continue;
        }
        for part in item["content"].as_array().into_iter().flatten() {
            match part["type"].as_str() {
                Some("output_text") => text.push_str(part["text"].as_str().unwrap_or("")),
                Some("refusal") => bail!(
                    "The model declined: {}",
                    part["refusal"].as_str().unwrap_or("")
                ),
                _ => {}
            }
        }
    }
    if text.trim().is_empty() {
        bail!("The AI returned an empty answer.");
    }
    Ok(text)
}

#[allow(clippy::too_many_arguments)]
pub async fn complete_chat(
    http: &reqwest::Client,
    base_url: &str,
    key: Option<&str>,
    model: &str,
    system: &str,
    user: &str,
    schema_name: &str,
    schema: &Value,
) -> Result<String> {
    let base = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    // Not every server supports JSON schemas; degrade step by step.
    let formats = [
        Some(
            json!({ "type": "json_schema", "json_schema": { "name": schema_name, "schema": schema, "strict": true } }),
        ),
        Some(json!({ "type": "json_object" })),
        None,
    ];
    let mut first_error = None;
    for format in formats {
        let mut body = base.clone();
        if let Some(f) = format {
            body["response_format"] = f;
        }
        let mut req = http
            .post(format!("{base_url}/chat/completions"))
            .json(&body);
        if let Some(k) = key.filter(|k| !k.is_empty()) {
            req = req.bearer_auth(k);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| anyhow!("could not reach {base_url}: {e}"))?;
        let status = resp.status();
        if status.is_success() {
            let v: Value = resp.json().await?;
            let choice = &v["choices"][0];
            if choice["finish_reason"] == "length" {
                bail!("The AI answer was cut off; the model's output limit is too small.");
            }
            let text = choice["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            if text.trim().is_empty() {
                bail!("The AI returned an empty answer.");
            }
            return Ok(text);
        }
        let err = http_error(resp).await;
        if !(status == reqwest::StatusCode::BAD_REQUEST
            || status == reqwest::StatusCode::UNPROCESSABLE_ENTITY)
        {
            return Err(err);
        }
        first_error.get_or_insert(err);
    }
    Err(first_error.unwrap_or_else(|| anyhow!("the AI request failed")))
}

pub async fn list_models(
    http: &reqwest::Client,
    base_url: &str,
    key: Option<&str>,
    openai: bool,
) -> Result<Vec<ModelInfo>> {
    let mut req = http.get(format!("{base_url}/models"));
    if let Some(k) = key.filter(|k| !k.is_empty()) {
        req = req.bearer_auth(k);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("could not reach {base_url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(http_error(resp).await);
    }
    let v: Value = resp.json().await?;
    const NOT_CHAT: &[&str] = &[
        "embedding",
        "tts",
        "whisper",
        "dall-e",
        "audio",
        "realtime",
        "transcribe",
        "image",
        "moderation",
        "davinci",
        "babbage",
        "sora",
    ];
    Ok(v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str())
                .filter(|id| !openai || !NOT_CHAT.iter().any(|x| id.contains(x)))
                .map(|id| ModelInfo::new(id, id))
                .collect()
        })
        .unwrap_or_default())
}
