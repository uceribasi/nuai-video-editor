//! LLM providers. The AI only ever sees text and answers with JSON that refers to
//! transcript ids; it never produces timestamps or touches the media.

pub mod anthropic;
pub mod codex;
pub mod openai;
pub mod review;
pub mod translate;

use std::future::Future;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use serde_json::Value;

use crate::settings::{self, AiProvider, AiSettings};
use crate::task::Cancel;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Reasoning effort levels the model accepts (empty: unknown).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

impl ModelInfo {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        ModelInfo {
            id: id.into(),
            name: name.into(),
            description: None,
            efforts: Vec::new(),
            default_effort: None,
        }
    }
}

pub struct AiClient {
    pub provider: AiProvider,
    key: Option<String>,
    pub model: String,
    base_url: String,
    effort: String,
    codex: Option<std::path::PathBuf>,
    http: reqwest::Client,
}

impl AiClient {
    pub fn from_settings(s: &AiSettings) -> Result<Self> {
        let key = match s.provider {
            AiProvider::None => bail!("No AI provider is configured. Choose one in Settings → AI."),
            AiProvider::CodexCli => None,
            p => settings::get_secret(p)?,
        };
        let (model, base_url) = match s.provider {
            AiProvider::Anthropic => (
                s.anthropic_model.clone(),
                "https://api.anthropic.com/v1".to_string(),
            ),
            AiProvider::Openai => (
                s.openai_model.clone(),
                "https://api.openai.com/v1".to_string(),
            ),
            AiProvider::Compatible => (
                s.compatible_model.clone(),
                s.compatible_base_url
                    .trim()
                    .trim_end_matches('/')
                    .to_string(),
            ),
            AiProvider::CodexCli => (codex::resolve_model(&s.codex_model), String::new()),
            AiProvider::None => unreachable!(),
        };
        let codex = if s.provider == AiProvider::CodexCli {
            Some(codex::locate(&s.codex_path)?)
        } else {
            None
        };
        if matches!(s.provider, AiProvider::Anthropic | AiProvider::Openai) && key.is_none() {
            bail!("No API key saved for this provider. Add one in Settings → AI.");
        }
        if s.provider == AiProvider::Compatible && base_url.is_empty() {
            bail!("Set the server URL for the OpenAI-compatible provider in Settings → AI.");
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(15 * 60))
            .build()?;
        Ok(AiClient {
            provider: s.provider,
            key,
            model,
            base_url,
            effort: s.effort.clone(),
            codex,
            http,
        })
    }

    /// Display name of the model in use.
    pub fn model_label(&self) -> String {
        match (self.provider, self.model.trim().is_empty()) {
            (AiProvider::CodexCli, true) => "Codex (default model)".into(),
            (AiProvider::CodexCli, false) => format!("Codex · {}", self.model.trim()),
            _ => self.model.clone(),
        }
    }

    pub fn require_model(&self) -> Result<()> {
        // Codex picks its own default model when none is set.
        if self.model.trim().is_empty() && self.provider != AiProvider::CodexCli {
            bail!("No model selected. Pick one in Settings → AI.");
        }
        Ok(())
    }

    /// Sends one system + user prompt and returns the parsed JSON answer.
    pub async fn complete_json(
        &self,
        system: &str,
        user: &str,
        schema_name: &str,
        schema: &Value,
        cancel: &Cancel,
    ) -> Result<Value> {
        self.require_model()?;
        let text = with_cancel(cancel, async {
            match self.provider {
                AiProvider::Anthropic => {
                    anthropic::complete(
                        &self.http,
                        &self.base_url,
                        self.key.as_deref().unwrap_or(""),
                        &self.model,
                        &self.effort,
                        system,
                        user,
                        schema,
                    )
                    .await
                }
                AiProvider::Openai => {
                    openai::complete_responses(
                        &self.http,
                        &self.base_url,
                        self.key.as_deref().unwrap_or(""),
                        &self.model,
                        &self.effort,
                        system,
                        user,
                        schema_name,
                        schema,
                    )
                    .await
                }
                AiProvider::Compatible => {
                    openai::complete_chat(
                        &self.http,
                        &self.base_url,
                        self.key.as_deref(),
                        &self.model,
                        system,
                        user,
                        schema_name,
                        schema,
                    )
                    .await
                }
                AiProvider::CodexCli => {
                    let path = self
                        .codex
                        .as_deref()
                        .ok_or_else(|| anyhow!("Codex CLI not found"))?;
                    codex::complete(path, &self.model, &self.effort, system, user, schema).await
                }
                AiProvider::None => bail!("no provider"),
            }
        })
        .await??;
        parse_json_lenient(&text)
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let mut models = match self.provider {
            AiProvider::Anthropic => {
                anthropic::list_models(
                    &self.http,
                    &self.base_url,
                    self.key.as_deref().unwrap_or(""),
                )
                .await?
            }
            AiProvider::Openai | AiProvider::Compatible => {
                openai::list_models(
                    &self.http,
                    &self.base_url,
                    self.key.as_deref(),
                    self.provider == AiProvider::Openai,
                )
                .await?
            }
            AiProvider::CodexCli => match &self.codex {
                Some(path) => codex::fetch_models(path).await,
                None => codex::cached_models(),
            },
            AiProvider::None => Vec::new(),
        };
        if matches!(self.provider, AiProvider::Openai | AiProvider::Compatible) {
            models.sort_by(|a, b| a.id.cmp(&b.id));
        }
        Ok(models)
    }
}

/// Runs `fut`, giving up as soon as the user cancels.
pub async fn with_cancel<T>(cancel: &Cancel, fut: impl Future<Output = T>) -> Result<T> {
    tokio::pin!(fut);
    let mut ticker = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            out = &mut fut => return Ok(out),
            _ = ticker.tick() => cancel.check()?,
        }
    }
}

/// Extracts the JSON object from a model answer that may be wrapped in code fences
/// or preceded by reasoning text.
pub fn parse_json_lenient(text: &str) -> Result<Value> {
    let mut t = text.trim().to_string();
    if let Some(end) = t.find("</think>") {
        t = t[end + "</think>".len()..].to_string();
    }
    if let Ok(v) = serde_json::from_str::<Value>(t.trim()) {
        return Ok(v);
    }
    let start = t
        .find('{')
        .ok_or_else(|| anyhow!("the AI answer contained no JSON"))?;
    let end = t
        .rfind('}')
        .ok_or_else(|| anyhow!("the AI answer contained no JSON"))?;
    serde_json::from_str(&t[start..=end])
        .map_err(|e| anyhow!("the AI answer was not valid JSON: {e}"))
}

/// Turns a non-success HTTP response into a readable error.
pub async fn http_error(resp: reqwest::Response) -> anyhow::Error {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .or_else(|| v["message"].as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(300).collect());
    anyhow!("AI provider returned {status}: {message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_and_prefixed_json() {
        assert_eq!(
            parse_json_lenient("```json\n{\"a\":1}\n```").unwrap()["a"],
            1
        );
        assert_eq!(
            parse_json_lenient("<think>hmm {x}</think>{\"a\":2}").unwrap()["a"],
            2
        );
        assert_eq!(
            parse_json_lenient("Here you go: {\"a\":3} done").unwrap()["a"],
            3
        );
        assert!(parse_json_lenient("nothing").is_err());
    }
}
