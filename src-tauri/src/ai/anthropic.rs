//! Claude via the Anthropic Messages API (API key auth).

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::{http_error, ModelInfo};

const VERSION: &str = "2023-06-01";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Models that accept `output_config.effort`.
fn supports_effort(model: &str) -> bool {
    !(model.contains("haiku") || model.contains("sonnet-4-5") || model.starts_with("claude-3"))
}

/// Models that accept the server-side refusal fallback (`fallbacks: "default"`).
fn supports_fallback(model: &str) -> bool {
    model.starts_with("claude-opus-5")
        || model.starts_with("claude-fable-5")
        || model == "claude-sonnet-5-5"
}

#[allow(clippy::too_many_arguments)]
pub async fn complete(
    http: &reqwest::Client,
    base_url: &str,
    key: &str,
    model: &str,
    effort: &str,
    system: &str,
    user: &str,
    schema: &Value,
) -> Result<String> {
    let mut body = json!({
        "model": model,
        "max_tokens": 16000,
        "system": [{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }],
        "messages": [{ "role": "user", "content": user }],
        "output_config": { "format": { "type": "json_schema", "schema": schema } },
    });
    if supports_effort(model) && !effort.is_empty() {
        body["output_config"]["effort"] = json!(effort);
    }
    let fallback = supports_fallback(model);
    if fallback {
        body["fallbacks"] = json!("default");
    }

    let mut resp = send(http, base_url, key, &body, fallback).await?;
    if resp.status() == reqwest::StatusCode::BAD_REQUEST {
        // Older models may not support structured outputs / effort / fallbacks:
        // retry with a plain request and rely on the prompt for the JSON shape.
        let mut plain = body.clone();
        if let Some(o) = plain.as_object_mut() {
            o.remove("output_config");
            o.remove("fallbacks");
        }
        let first_error = http_error(resp).await;
        resp = send(http, base_url, key, &plain, false).await?;
        if !resp.status().is_success() {
            return Err(first_error);
        }
    }
    if !resp.status().is_success() {
        return Err(http_error(resp).await);
    }

    let v: Value = resp.json().await?;
    match v["stop_reason"].as_str() {
        Some("refusal") => bail!(
            "The model declined this request ({}).",
            v["stop_details"]["category"].as_str().unwrap_or("policy")
        ),
        Some("max_tokens") => {
            bail!("The AI answer was cut off; try a shorter video or fewer instructions.")
        }
        _ => {}
    }
    let text: String = v["content"]
        .as_array()
        .ok_or_else(|| anyhow!("unexpected response from Anthropic"))?
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    if text.trim().is_empty() {
        bail!("The AI returned an empty answer.");
    }
    Ok(text)
}

async fn send(
    http: &reqwest::Client,
    base_url: &str,
    key: &str,
    body: &Value,
    fallback: bool,
) -> Result<reqwest::Response> {
    let mut req = http
        .post(format!("{base_url}/messages"))
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
        .json(body);
    if fallback {
        req = req.header("anthropic-beta", FALLBACK_BETA);
    }
    Ok(req.send().await?)
}

pub async fn list_models(
    http: &reqwest::Client,
    base_url: &str,
    key: &str,
) -> Result<Vec<ModelInfo>> {
    let resp = http
        .get(format!("{base_url}/models?limit=100"))
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(http_error(resp).await);
    }
    let v: Value = resp.json().await?;
    Ok(v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?.to_string();
                    let name = m["display_name"].as_str().unwrap_or(&id).to_string();
                    Some(ModelInfo::new(id, name))
                })
                .collect()
        })
        .unwrap_or_default())
}
