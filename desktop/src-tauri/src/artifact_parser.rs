use std::{fs, path::Path};

use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportedArtifactType {
    Markdown,
    Text,
    Pdf,
}

pub fn supported_artifact_type(path: &Path) -> Result<SupportedArtifactType> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| anyhow!("artifact file must include an extension"))?;

    match extension.as_str() {
        "md" => Ok(SupportedArtifactType::Markdown),
        "txt" => Ok(SupportedArtifactType::Text),
        "pdf" => Ok(SupportedArtifactType::Pdf),
        _ => Err(anyhow!(
            "unsupported artifact type '{}'; supported: .md, .txt, .pdf",
            extension
        )),
    }
}

pub fn parse_text_from_artifact(path: &Path) -> Result<String> {
    let artifact_type = supported_artifact_type(path)?;

    let raw_text = match artifact_type {
        SupportedArtifactType::Markdown | SupportedArtifactType::Text => {
            fs::read_to_string(path)
                .with_context(|| format!("failed to read text artifact {}", path.display()))?
        }
        SupportedArtifactType::Pdf => pdf_extract::extract_text(path)
            .with_context(|| format!("failed to extract text from pdf {}", path.display()))?,
    };

    let cleaned = raw_text.replace('\u{0000}', "");
    let trimmed = cleaned.trim().to_string();

    if trimmed.is_empty() {
        Err(anyhow!("artifact did not produce any parseable text"))
    } else {
        Ok(trimmed)
    }
}
