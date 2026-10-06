//! Whisper model catalog and downloads.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::task::{Cancel, Progress};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpec {
    pub id: &'static str,
    pub file: &'static str,
    pub size_mb: u32,
    pub multilingual: bool,
    pub recommended: bool,
}

pub const CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: "large-v3-turbo-q5_0",
        file: "ggml-large-v3-turbo-q5_0.bin",
        size_mb: 547,
        multilingual: true,
        recommended: true,
    },
    ModelSpec {
        id: "large-v3-turbo",
        file: "ggml-large-v3-turbo.bin",
        size_mb: 1549,
        multilingual: true,
        recommended: false,
    },
    ModelSpec {
        id: "small",
        file: "ggml-small.bin",
        size_mb: 466,
        multilingual: true,
        recommended: false,
    },
    ModelSpec {
        id: "base",
        file: "ggml-base.bin",
        size_mb: 142,
        multilingual: true,
        recommended: false,
    },
    ModelSpec {
        id: "tiny",
        file: "ggml-tiny.bin",
        size_mb: 75,
        multilingual: true,
        recommended: false,
    },
];

pub const DEFAULT_MODEL: &str = "large-v3-turbo-q5_0";

pub fn spec(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|m| m.id == id)
}

pub fn model_path(models_dir: &Path, id: &str) -> Option<PathBuf> {
    spec(id).map(|s| models_dir.join(s.file))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    #[serde(flatten)]
    pub spec: ModelSpec,
    pub installed: bool,
}

pub fn statuses(models_dir: &Path) -> Vec<ModelStatus> {
    CATALOG
        .iter()
        .map(|s| ModelStatus {
            spec: s.clone(),
            installed: models_dir.join(s.file).is_file(),
        })
        .collect()
}

pub async fn download(
    models_dir: &Path,
    id: &str,
    progress: &Progress,
    cancel: &Cancel,
) -> Result<PathBuf> {
    let Some(s) = spec(id) else {
        bail!("unknown model {id}")
    };
    std::fs::create_dir_all(models_dir)?;
    let dest = models_dir.join(s.file);
    let part = models_dir.join(format!("{}.part", s.file));
    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        s.file
    );

    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?
        .error_for_status()?;
    let total = resp
        .content_length()
        .unwrap_or(s.size_mb as u64 * 1024 * 1024) as f64;
    let mut file = tokio::fs::File::create(&part).await?;
    let mut stream = resp.bytes_stream();
    let mut done = 0u64;
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            bail!("Cancelled");
        }
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        progress((done as f64 / total).min(1.0));
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&part, &dest).await?;
    Ok(dest)
}
