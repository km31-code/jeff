use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::models::LocalRuntimeStatusDto;

pub const LOCAL_REASONING_MODEL_ID: &str = "local-reflex-llamacpp";
// lexical fallback embedding id. used when no semantic embedding model is
// installed and running; embeddings are then a deterministic token hash.
pub const LOCAL_EMBEDDING_MODEL_ID: &str = "local-hash-embedding-v1";
// apex b1: semantic embedding id. used when the curated on-device embedding
// model (bge-small-en-v1.5) is installed and the sidecar is serving it.
// distinct id so retrieval's lazy migration re-embeds chunks when the user
// moves from lexical fallback to the semantic model.
pub const LOCAL_SEMANTIC_EMBEDDING_MODEL_ID: &str = "local-bge-small-en-v1.5-q8_0";
pub const LOCAL_REASONING_MODEL_FILE: &str = "reflex-instruct.gguf";
pub const LOCAL_EMBEDDING_MODEL_FILE: &str = "embedding.gguf";
#[allow(dead_code)]
pub const LOCAL_RUNTIME_CHOICE: &str = "llama.cpp server";

// apex b1: curated on-device embedding model. bge-small-en-v1.5 (384-dim,
// q8_0 gguf, ~35 MB) is a real semantic embedder small enough for a one-click
// download. the checksum is the git-lfs sha-256 of the published artifact and
// is verified after download in download_model.
pub const CURATED_EMBEDDING_MODEL_URL: &str = "https://huggingface.co/CompendiumLabs/bge-small-en-v1.5-gguf/resolve/main/bge-small-en-v1.5-q8_0.gguf";
pub const CURATED_EMBEDDING_MODEL_SHA256: &str =
    "ec38e8da142596baa913124ae50550de284b6916bf59577ef2f0cb9660c2f514";
pub const CURATED_EMBEDDING_MODEL_BYTES: u64 = 36_806_944;

const DEFAULT_PORT: u16 = 17631;
const STARTUP_TIMEOUT_MS: u64 = 4_000;
const HEALTH_TIMEOUT_MS: u64 = 250;
const DOWNLOAD_HEADROOM_BYTES: u64 = 256 * 1024 * 1024;
// how long a semantic-embedding capability probe is cached before re-checking
// sidecar health. keeps model_id() and embed_text() consistent and cheap.
const EMBED_CAPABILITY_TTL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalModelKind {
    Reasoning,
    Embedding,
}

impl LocalModelKind {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "reasoning" | "reflex" | "instruct" => Ok(Self::Reasoning),
            "embedding" | "embeddings" => Ok(Self::Embedding),
            other => Err(anyhow!(
                "unknown local model kind '{other}' (expected reasoning or embedding)"
            )),
        }
    }
}

#[derive(Debug)]
struct LocalRuntimeInner {
    child: Option<Child>,
    last_error: Option<String>,
    // cached (semantic_available, checked_at) for the embedding capability
    // probe. bounds how often health_check runs on the embedding hot path.
    embed_capability: Option<(bool, Instant)>,
}

pub struct LocalRuntime {
    models_dir: PathBuf,
    endpoint: String,
    inner: Mutex<LocalRuntimeInner>,
}

impl LocalRuntime {
    pub fn new(app_data_dir: &Path) -> Self {
        let port = std::env::var("JEFF_LOCAL_RUNTIME_PORT")
            .ok()
            .and_then(|raw| raw.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let endpoint = std::env::var("JEFF_LOCAL_RUNTIME_ENDPOINT")
            .unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));
        Self {
            models_dir: app_data_dir.join("models"),
            endpoint,
            inner: Mutex::new(LocalRuntimeInner {
                child: None,
                last_error: None,
                embed_capability: None,
            }),
        }
    }

    // apex b1: whether real semantic embeddings are available right now. true
    // only when the curated embedding model file is present and the sidecar is
    // healthy. cached with a short ttl so it is cheap to consult per embed.
    pub fn semantic_embedding_available(&self) -> bool {
        if !self.embedding_model_path().is_file() {
            return false;
        }
        if let Ok(inner) = self.inner.lock() {
            if let Some((value, at)) = inner.embed_capability {
                if at.elapsed() < EMBED_CAPABILITY_TTL {
                    return value;
                }
            }
        }
        let healthy = self.health_check();
        if let Ok(mut inner) = self.inner.lock() {
            inner.embed_capability = Some((healthy, Instant::now()));
        }
        healthy
    }

    // invalidate the cached capability so the next probe re-checks. called when
    // a sidecar embed fails so model_id() and embed_text() re-agree on lexical
    // fallback rather than mislabeling a hash vector as semantic.
    pub fn mark_embedding_capability_stale(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.embed_capability = None;
        }
    }

    // apex b1: one-click curated semantic embedding model download. verifies
    // the published checksum and size.
    pub fn download_curated_embedding_model(&self) -> Result<LocalRuntimeStatusDto> {
        self.download_model(
            LocalModelKind::Embedding,
            CURATED_EMBEDDING_MODEL_URL,
            CURATED_EMBEDDING_MODEL_SHA256,
            Some(CURATED_EMBEDDING_MODEL_BYTES),
        )
    }

    pub fn status(&self) -> LocalRuntimeStatusDto {
        let _ = fs::create_dir_all(&self.models_dir);
        let mut sidecar_pid = None;
        let mut running = false;
        let mut last_error = None;
        if let Ok(mut inner) = self.inner.lock() {
            running = child_is_running(&mut inner.child);
            sidecar_pid = inner.child.as_ref().map(|child| child.id());
            last_error = inner.last_error.clone();
        }
        let healthy = self.health_check();
        let sidecar_configured = self.server_executable_path().is_some();
        let reasoning_model_path = self.reasoning_model_path();
        let embedding_model_path = self.embedding_model_path();
        let reasoning_model_present = reasoning_model_path.is_file();
        let embedding_model_present = embedding_model_path.is_file();
        let installed_model_bytes =
            file_len(&reasoning_model_path) + file_len(&embedding_model_path);
        let deterministic_fallback_enabled = true;
        // semantic embeddings require both the model file and a healthy sidecar;
        // reuse the health probe already computed above and refresh the cache.
        let semantic_embedding_available = embedding_model_present && healthy;
        if let Ok(mut inner) = self.inner.lock() {
            inner.embed_capability = Some((semantic_embedding_available, Instant::now()));
        }
        let embedding_mode = if semantic_embedding_available {
            "semantic"
        } else {
            "lexical_fallback"
        };
        let active_embedding_model_id = if semantic_embedding_available {
            LOCAL_SEMANTIC_EMBEDDING_MODEL_ID
        } else {
            LOCAL_EMBEDDING_MODEL_ID
        };
        let mode = if healthy {
            "sidecar"
        } else if deterministic_fallback_enabled {
            "deterministic_local"
        } else {
            "unavailable"
        };

        LocalRuntimeStatusDto {
            enabled: true,
            healthy,
            running,
            mode: mode.to_string(),
            sidecar_configured,
            sidecar_pid,
            endpoint: self.endpoint.clone(),
            model_dir: self.models_dir.display().to_string(),
            reasoning_model_id: LOCAL_REASONING_MODEL_ID.to_string(),
            reasoning_model_path: reasoning_model_path.display().to_string(),
            reasoning_model_present,
            embedding_model_id: active_embedding_model_id.to_string(),
            embedding_model_path: embedding_model_path.display().to_string(),
            embedding_model_present,
            embedding_mode: embedding_mode.to_string(),
            semantic_embedding_available,
            curated_embedding_url: CURATED_EMBEDDING_MODEL_URL.to_string(),
            curated_embedding_sha256: CURATED_EMBEDDING_MODEL_SHA256.to_string(),
            curated_embedding_bytes: CURATED_EMBEDDING_MODEL_BYTES,
            deterministic_fallback_enabled,
            last_error,
            disk_available_bytes: available_disk_bytes(&self.models_dir).ok().flatten(),
            installed_model_bytes,
        }
    }

    pub fn reasoning_model_path(&self) -> PathBuf {
        self.models_dir.join(LOCAL_REASONING_MODEL_FILE)
    }

    pub fn embedding_model_path(&self) -> PathBuf {
        self.models_dir.join(LOCAL_EMBEDDING_MODEL_FILE)
    }

    #[allow(dead_code)]
    pub fn server_configured(&self) -> bool {
        self.server_executable_path().is_some()
    }

    pub fn health_check(&self) -> bool {
        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(HEALTH_TIMEOUT_MS))
            .build()
        {
            Ok(client) => client,
            Err(_) => return false,
        };
        for path in ["/health", "/v1/models"] {
            let url = format!("{}{}", self.endpoint, path);
            if let Ok(response) = client.get(url).send() {
                if response.status().is_success() {
                    return true;
                }
            }
        }
        false
    }

    pub fn start(&self) -> Result<LocalRuntimeStatusDto> {
        fs::create_dir_all(&self.models_dir).with_context(|| {
            format!(
                "failed to create local model directory {}",
                self.models_dir.display()
            )
        })?;

        if self.health_check() {
            return Ok(self.status());
        }

        let executable = self
            .server_executable_path()
            .ok_or_else(|| anyhow!("llama.cpp server executable not found"))?;
        let model_path = self.reasoning_model_path();
        if !model_path.is_file() {
            return Err(anyhow!(
                "local reasoning model missing at {}",
                model_path.display()
            ));
        }

        self.stop().ok();

        let port = self
            .endpoint
            .rsplit(':')
            .next()
            .and_then(|raw| raw.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let mut command = Command::new(&executable);
        command
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--model")
            .arg(&model_path)
            .arg("--ctx-size")
            .arg("4096")
            .arg("--embedding")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if let Ok(extra_args) = std::env::var("JEFF_LOCAL_LLAMACPP_ARGS") {
            for arg in extra_args.split_whitespace() {
                command.arg(arg);
            }
        }

        let child = command.spawn().with_context(|| {
            format!(
                "failed to start local runtime {} at {}",
                executable.display(),
                self.endpoint
            )
        })?;

        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| anyhow!("local runtime lock poisoned"))?;
            inner.child = Some(child);
            inner.last_error = None;
        }

        let deadline = Instant::now() + Duration::from_millis(STARTUP_TIMEOUT_MS);
        while Instant::now() < deadline {
            if self.health_check() {
                return Ok(self.status());
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let _ = self.stop();
        let error = "local runtime did not become healthy before startup timeout".to_string();
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_error = Some(error.clone());
        }
        Err(anyhow!(error))
    }

    pub fn stop(&self) -> Result<LocalRuntimeStatusDto> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("local runtime lock poisoned"))?;
        if let Some(mut child) = inner.child.take() {
            if child
                .try_wait()
                .context("failed to inspect local runtime")?
                .is_none()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Ok(self.status())
    }

    pub fn delete_model(&self, kind: LocalModelKind) -> Result<LocalRuntimeStatusDto> {
        if matches!(kind, LocalModelKind::Reasoning) {
            let _ = self.stop();
        }
        let path = self.model_path(kind);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to delete local model {}", path.display()))?;
        }
        Ok(self.status())
    }

    pub fn download_model(
        &self,
        kind: LocalModelKind,
        url: &str,
        expected_sha256: &str,
        expected_bytes: Option<u64>,
    ) -> Result<LocalRuntimeStatusDto> {
        let url = url.trim();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(anyhow!("local model download URL must be http(s)"));
        }
        let expected_sha256 = expected_sha256.trim().to_ascii_lowercase();
        if expected_sha256.len() != 64 || !expected_sha256.chars().all(|ch| ch.is_ascii_hexdigit())
        {
            return Err(anyhow!(
                "expected_sha256 must be a 64-character SHA-256 hex digest"
            ));
        }

        fs::create_dir_all(&self.models_dir).with_context(|| {
            format!(
                "failed to create local model directory {}",
                self.models_dir.display()
            )
        })?;
        if let (Some(required), Ok(Some(available))) =
            (expected_bytes, available_disk_bytes(&self.models_dir))
        {
            let needed = required.saturating_add(DOWNLOAD_HEADROOM_BYTES);
            if available < needed {
                return Err(anyhow!(
                    "not enough free disk for local model: need at least {} bytes, have {}",
                    needed,
                    available
                ));
            }
        }

        let target = self.model_path(kind);
        let temp = target.with_extension("part");
        if temp.exists() {
            let _ = fs::remove_file(&temp);
        }

        let mut response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .context("failed to build local model downloader")?
            .get(url)
            .send()
            .context("failed to download local model")?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "local model download failed with status {}",
                response.status()
            ));
        }

        if let (Some(required), Some(content_length)) = (expected_bytes, response.content_length())
        {
            if content_length > required.saturating_add(4096) {
                return Err(anyhow!(
                    "download size {} exceeds expected size {}",
                    content_length,
                    required
                ));
            }
        }

        let mut file = File::create(&temp)
            .with_context(|| format!("failed to create temp model file {}", temp.display()))?;
        let mut hasher = Sha256::new();
        let mut downloaded = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = response
                .read(&mut buffer)
                .context("failed while reading local model download")?;
            if read == 0 {
                break;
            }
            downloaded += read as u64;
            if let Some(required) = expected_bytes {
                if downloaded > required.saturating_add(4096) {
                    let _ = fs::remove_file(&temp);
                    return Err(anyhow!(
                        "download exceeded expected size ({} > {})",
                        downloaded,
                        required
                    ));
                }
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])
                .context("failed while writing local model download")?;
        }
        file.sync_all().ok();

        let actual = format!("{:x}", hasher.finalize());
        if actual != expected_sha256 {
            let _ = fs::remove_file(&temp);
            return Err(anyhow!(
                "local model checksum mismatch: expected {}, got {}",
                expected_sha256,
                actual
            ));
        }

        fs::rename(&temp, &target).with_context(|| {
            format!(
                "failed to move verified local model into {}",
                target.display()
            )
        })?;
        Ok(self.status())
    }

    pub fn chat_completion(
        &self,
        model: &str,
        system: &str,
        user: &str,
        temperature: f32,
        max_tokens: Option<u32>,
        json_object: bool,
    ) -> Result<String> {
        if !self.health_check() {
            self.start()
                .context("local runtime was not healthy and restart failed")?;
        }

        let mut body = serde_json::json!({
            "model": model,
            "temperature": temperature,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ]
        });
        if let Some(max_tokens) = max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }
        if json_object {
            body["response_format"] = serde_json::json!({ "type": "json_object" });
        }

        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build local runtime client")?
            .post(format!("{}/v1/chat/completions", self.endpoint))
            .json(&body)
            .send()
            .context("failed to call local runtime chat endpoint")?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "local runtime chat failed with status {}",
                response.status()
            ));
        }

        let payload: ChatCompletionResponse = response
            .json()
            .context("failed to parse local runtime chat response")?;
        payload
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| anyhow!("local runtime returned no chat content"))
    }

    pub fn embed_text_via_sidecar(&self, input: &str) -> Result<Vec<f32>> {
        if !self.health_check() {
            self.start()
                .context("local runtime was not healthy and restart failed")?;
        }
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build local runtime client")?
            .post(format!("{}/v1/embeddings", self.endpoint))
            .json(&serde_json::json!({
                "model": LOCAL_EMBEDDING_MODEL_ID,
                "input": input,
            }))
            .send()
            .context("failed to call local runtime embeddings endpoint")?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "local runtime embeddings failed with status {}",
                response.status()
            ));
        }

        let payload: EmbeddingResponse = response
            .json()
            .context("failed to parse local runtime embedding response")?;
        payload
            .data
            .into_iter()
            .next()
            .map(|row| row.embedding)
            .filter(|embedding| !embedding.is_empty())
            .ok_or_else(|| anyhow!("local runtime returned no embedding vector"))
    }

    fn model_path(&self, kind: LocalModelKind) -> PathBuf {
        match kind {
            LocalModelKind::Reasoning => self.reasoning_model_path(),
            LocalModelKind::Embedding => self.embedding_model_path(),
        }
    }

    fn server_executable_path(&self) -> Option<PathBuf> {
        if let Ok(raw) = std::env::var("JEFF_LOCAL_LLAMACPP_SERVER") {
            let path = PathBuf::from(raw.trim());
            if path.is_file() {
                return Some(path);
            }
        }

        for candidate in [
            self.models_dir.join("llama-server"),
            PathBuf::from("/opt/homebrew/bin/llama-server"),
            PathBuf::from("/usr/local/bin/llama-server"),
            PathBuf::from("/usr/bin/llama-server"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        std::env::var_os("PATH").and_then(|path_var| {
            std::env::split_paths(&path_var)
                .map(|dir| dir.join("llama-server"))
                .find(|candidate| candidate.is_file())
        })
    }
}

impl Drop for LocalRuntime {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(mut child) = inner.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn child_is_running(child: &mut Option<Child>) -> bool {
    match child {
        Some(process) => match process.try_wait() {
            Ok(Some(_)) => {
                *child = None;
                false
            }
            Ok(None) => true,
            Err(_) => false,
        },
        None => false,
    }
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn available_disk_bytes(path: &Path) -> Result<Option<u64>> {
    fs::create_dir_all(path).ok();
    let output = Command::new("df")
        .arg("-k")
        .arg(path)
        .output()
        .context("failed to run df for local model disk-space check")?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(line) = text.lines().nth(1) else {
        return Ok(None);
    };
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 4 {
        return Ok(None);
    }
    Ok(fields[3].parse::<u64>().ok().map(|kb| kb * 1024))
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingRow>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingRow {
    embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a3_status_reports_model_paths_and_deterministic_mode() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::new(dir.path());
        let status = runtime.status();
        assert!(status.enabled);
        assert!(status.model_dir.ends_with("models"));
        assert_eq!(status.reasoning_model_id, LOCAL_REASONING_MODEL_ID);
        assert_eq!(status.embedding_model_id, LOCAL_EMBEDDING_MODEL_ID);
        assert!(status.deterministic_fallback_enabled);
    }

    #[test]
    fn b1_curated_embedding_catalog_is_wellformed() {
        assert!(CURATED_EMBEDDING_MODEL_URL.starts_with("https://"));
        assert!(CURATED_EMBEDDING_MODEL_URL.ends_with(".gguf"));
        assert_eq!(CURATED_EMBEDDING_MODEL_SHA256.len(), 64);
        assert!(CURATED_EMBEDDING_MODEL_SHA256
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        assert!(CURATED_EMBEDDING_MODEL_BYTES > 0);
    }

    #[test]
    fn b1_semantic_embedding_unavailable_without_model_file() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::new(dir.path());
        assert!(!runtime.semantic_embedding_available());
        let status = runtime.status();
        assert_eq!(status.embedding_mode, "lexical_fallback");
        assert!(!status.semantic_embedding_available);
        assert_eq!(status.embedding_model_id, LOCAL_EMBEDDING_MODEL_ID);
    }

    #[test]
    fn a3_model_kind_parser_accepts_expected_names() {
        assert_eq!(
            LocalModelKind::parse("reasoning").unwrap(),
            LocalModelKind::Reasoning
        );
        assert_eq!(
            LocalModelKind::parse("embeddings").unwrap(),
            LocalModelKind::Embedding
        );
        assert!(LocalModelKind::parse("voice").is_err());
    }
}
