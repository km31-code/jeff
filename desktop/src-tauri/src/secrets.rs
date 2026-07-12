use anyhow::{anyhow, Context, Result};
use keyring::{Entry, Error as KeyringError};

use crate::onboarding::{API_KEY_SOURCE_ENV, API_KEY_SOURCE_KEYCHAIN, API_KEY_SOURCE_NONE};

pub const OPENAI_KEYCHAIN_SERVICE: &str = "com.jeff.desktop";
pub const OPENAI_KEYCHAIN_ACCOUNT: &str = "openai_api_key";
pub const ANTHROPIC_KEYCHAIN_ACCOUNT: &str = "anthropic_api_key";
pub const BUNDLED_TOKEN_KEYCHAIN_ACCOUNT: &str = "bundled_inference_token";
pub const BUNDLED_TOKEN_ENV_VAR: &str = "JEFF_BUNDLED_INFERENCE_TOKEN";
pub const PREFER_ENV_OPENAI_KEY_VAR: &str = "JEFF_PREFER_ENV_OPENAI_API_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiKeyResolution {
    pub api_key: Option<String>,
    pub source: &'static str,
}

pub trait OpenAiKeyStore {
    fn get_openai_api_key(&self) -> Result<Option<String>>;
    fn set_openai_api_key(&self, api_key: &str) -> Result<()>;
    fn delete_openai_api_key(&self) -> Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemOpenAiKeyStore;

impl SystemOpenAiKeyStore {
    fn entry() -> Result<Entry> {
        Entry::new(OPENAI_KEYCHAIN_SERVICE, OPENAI_KEYCHAIN_ACCOUNT)
            .context("failed to initialize OpenAI API key keychain entry")
    }
}

impl OpenAiKeyStore for SystemOpenAiKeyStore {
    fn get_openai_api_key(&self) -> Result<Option<String>> {
        let entry = Self::entry()?;
        match entry.get_password() {
            Ok(value) => {
                let trimmed = value.trim().to_string();
                if trimmed.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(trimmed))
                }
            }
            Err(KeyringError::NoEntry) => Ok(None),
            Err(err) => Err(anyhow!(
                "failed to read OpenAI API key from keychain: {err}"
            )),
        }
    }

    fn set_openai_api_key(&self, api_key: &str) -> Result<()> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("API key cannot be empty"));
        }

        let entry = Self::entry()?;
        entry
            .set_password(trimmed)
            .context("failed to store OpenAI API key in keychain")?;
        Ok(())
    }

    fn delete_openai_api_key(&self) -> Result<()> {
        let entry = Self::entry()?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(err) => Err(anyhow!(
                "failed to delete OpenAI API key from keychain: {err}"
            )),
        }
    }
}

pub fn openai_api_key_from_env() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_or_none_resolution(env_value: Option<String>) -> OpenAiKeyResolution {
    if let Some(value) = env_value {
        return OpenAiKeyResolution {
            api_key: Some(value),
            source: API_KEY_SOURCE_ENV,
        };
    }

    OpenAiKeyResolution {
        api_key: None,
        source: API_KEY_SOURCE_NONE,
    }
}

#[allow(dead_code)]
pub fn resolve_openai_api_key_with_store<S: OpenAiKeyStore>(
    store: &S,
    env_value: Option<String>,
) -> OpenAiKeyResolution {
    resolve_openai_api_key_with_preference(store, env_value, false)
}

pub fn resolve_openai_api_key_with_preference<S: OpenAiKeyStore>(
    store: &S,
    env_value: Option<String>,
    prefer_env: bool,
) -> OpenAiKeyResolution {
    if prefer_env && env_value.is_some() {
        return env_or_none_resolution(env_value);
    }

    match store.get_openai_api_key() {
        Ok(Some(api_key)) => OpenAiKeyResolution {
            api_key: Some(api_key),
            source: API_KEY_SOURCE_KEYCHAIN,
        },
        Ok(None) => env_or_none_resolution(env_value),
        Err(err) => {
            eprintln!("[jeff secrets] keychain read failed, falling back to env: {err}");
            env_or_none_resolution(env_value)
        }
    }
}

pub fn resolve_openai_api_key() -> OpenAiKeyResolution {
    resolve_openai_api_key_with_preference(
        &SystemOpenAiKeyStore,
        openai_api_key_from_env(),
        prefer_env_openai_api_key(),
    )
}

fn prefer_env_openai_api_key() -> bool {
    matches!(
        std::env::var(PREFER_ENV_OPENAI_KEY_VAR)
            .unwrap_or_default()
            .trim(),
        "1" | "true" | "TRUE" | "yes" | "YES"
    )
}

pub fn resolve_openai_api_key_required() -> Result<String> {
    resolve_openai_api_key()
        .api_key
        .ok_or_else(|| anyhow!("OPENAI_API_KEY is not configured"))
}

pub fn store_openai_api_key(api_key: &str) -> Result<()> {
    SystemOpenAiKeyStore.set_openai_api_key(api_key)
}

pub fn delete_openai_api_key() -> Result<()> {
    SystemOpenAiKeyStore.delete_openai_api_key()
}

// ---- apex a1: anthropic api key ----------------------------------------------
// mirrors the openai key path: keychain first, env var fallback. the anthropic
// key is optional — the model router falls back to openai when it is absent.

fn anthropic_entry() -> Result<Entry> {
    Entry::new(OPENAI_KEYCHAIN_SERVICE, ANTHROPIC_KEYCHAIN_ACCOUNT)
        .context("failed to initialize Anthropic API key keychain entry")
}

pub fn anthropic_api_key_from_env() -> Option<String> {
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn resolve_anthropic_api_key() -> Option<String> {
    match anthropic_entry().and_then(|entry| match entry.get_password() {
        Ok(value) => Ok(Some(value.trim().to_string()).filter(|v| !v.is_empty())),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(err) => Err(anyhow!(
            "failed to read Anthropic API key from keychain: {err}"
        )),
    }) {
        Ok(Some(key)) => Some(key),
        Ok(None) => anthropic_api_key_from_env(),
        Err(err) => {
            eprintln!("[jeff secrets] anthropic keychain read failed, falling back to env: {err}");
            anthropic_api_key_from_env()
        }
    }
}

pub fn resolve_anthropic_api_key_required() -> Result<String> {
    resolve_anthropic_api_key().ok_or_else(|| anyhow!("ANTHROPIC_API_KEY is not configured"))
}

pub fn store_anthropic_api_key(api_key: &str) -> Result<()> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("API key cannot be empty"));
    }
    anthropic_entry()?
        .set_password(trimmed)
        .context("failed to store Anthropic API key in keychain")
}

pub fn delete_anthropic_api_key() -> Result<()> {
    match anthropic_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(err) => Err(anyhow!(
            "failed to delete Anthropic API key from keychain: {err}"
        )),
    }
}

fn bundled_token_entry() -> Result<Entry> {
    Entry::new(OPENAI_KEYCHAIN_SERVICE, BUNDLED_TOKEN_KEYCHAIN_ACCOUNT)
        .context("failed to initialize bundled inference token keychain entry")
}

pub fn resolve_bundled_inference_token() -> Option<String> {
    match bundled_token_entry().and_then(|entry| match entry.get_password() {
        Ok(value) => Ok(Some(value.trim().to_string()).filter(|v| !v.is_empty())),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(err) => Err(anyhow!(
            "failed to read bundled inference token from keychain: {err}"
        )),
    }) {
        Ok(Some(token)) => Some(token),
        Ok(None) | Err(_) => std::env::var(BUNDLED_TOKEN_ENV_VAR)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    }
}

pub fn store_bundled_inference_token(token: &str) -> Result<()> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("bundled inference token cannot be empty"));
    }
    bundled_token_entry()?
        .set_password(trimmed)
        .context("failed to store bundled inference token in keychain")
}

#[allow(dead_code)]
pub fn delete_bundled_inference_token() -> Result<()> {
    match bundled_token_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(err) => Err(anyhow!(
            "failed to delete bundled inference token from keychain: {err}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockStore {
        key: Option<String>,
        fail: bool,
    }

    impl OpenAiKeyStore for MockStore {
        fn get_openai_api_key(&self) -> Result<Option<String>> {
            if self.fail {
                return Err(anyhow!("keychain unavailable"));
            }
            Ok(self.key.clone())
        }

        fn set_openai_api_key(&self, _api_key: &str) -> Result<()> {
            Ok(())
        }

        fn delete_openai_api_key(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn resolve_prefers_keychain_over_env() {
        let store = MockStore {
            key: Some("sk-keychain".to_string()),
            fail: false,
        };
        let resolved = resolve_openai_api_key_with_store(&store, Some("sk-env".to_string()));
        assert_eq!(resolved.source, API_KEY_SOURCE_KEYCHAIN);
        assert_eq!(resolved.api_key.as_deref(), Some("sk-keychain"));
    }

    #[test]
    fn resolve_can_prefer_env_for_eval_runs() {
        let store = MockStore {
            key: Some("sk-keychain".to_string()),
            fail: false,
        };
        let resolved =
            resolve_openai_api_key_with_preference(&store, Some("sk-env".to_string()), true);
        assert_eq!(resolved.source, API_KEY_SOURCE_ENV);
        assert_eq!(resolved.api_key.as_deref(), Some("sk-env"));
    }

    #[test]
    fn resolve_falls_back_to_env_when_keychain_empty() {
        let store = MockStore {
            key: None,
            fail: false,
        };
        let resolved = resolve_openai_api_key_with_store(&store, Some("sk-env".to_string()));
        assert_eq!(resolved.source, API_KEY_SOURCE_ENV);
        assert_eq!(resolved.api_key.as_deref(), Some("sk-env"));
    }

    #[test]
    fn resolve_falls_back_to_none_when_no_sources() {
        let store = MockStore {
            key: None,
            fail: false,
        };
        let resolved = resolve_openai_api_key_with_store(&store, None);
        assert_eq!(resolved.source, API_KEY_SOURCE_NONE);
        assert_eq!(resolved.api_key, None);
    }

    #[test]
    fn resolve_uses_env_when_keychain_read_fails() {
        let store = MockStore {
            key: None,
            fail: true,
        };
        let resolved = resolve_openai_api_key_with_store(&store, Some("sk-env".to_string()));
        assert_eq!(resolved.source, API_KEY_SOURCE_ENV);
        assert_eq!(resolved.api_key.as_deref(), Some("sk-env"));
    }
}
