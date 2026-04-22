use anyhow::{anyhow, Context, Result};
use keyring::{Entry, Error as KeyringError};

use crate::onboarding::{API_KEY_SOURCE_ENV, API_KEY_SOURCE_KEYCHAIN, API_KEY_SOURCE_NONE};

pub const OPENAI_KEYCHAIN_SERVICE: &str = "com.jeff.desktop";
pub const OPENAI_KEYCHAIN_ACCOUNT: &str = "openai_api_key";

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

pub fn resolve_openai_api_key_with_store<S: OpenAiKeyStore>(
    store: &S,
    env_value: Option<String>,
) -> OpenAiKeyResolution {
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
    resolve_openai_api_key_with_store(&SystemOpenAiKeyStore, openai_api_key_from_env())
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
