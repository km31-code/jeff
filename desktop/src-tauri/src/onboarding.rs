pub const APP_SETTING_ONBOARDING_COMPLETE: &str = "onboarding_complete";
pub const APP_SETTING_PREFERRED_WORKSPACE_FOLDER: &str = "preferred_workspace_folder";
pub const APP_SETTING_ONBOARDING_LAST_COMPLETED_AT: &str = "onboarding_last_completed_at";
pub const APP_SETTING_WORKSPACE_PROMPT_DISMISSED: &str = "workspace_prompt_dismissed";

pub const API_KEY_SOURCE_KEYCHAIN: &str = "keychain";
pub const API_KEY_SOURCE_ENV: &str = "env";
pub const API_KEY_SOURCE_NONE: &str = "none";

// apex e6: inference choice. Bundled = Jeff-provided metered relay (no key
// entry); byok = bring-your-own-key. Default byok preserves existing behavior.
pub const INFERENCE_MODE_KEY: &str = "inference_mode";
pub const INFERENCE_MODE_BUNDLED: &str = "bundled";
pub const INFERENCE_MODE_BYOK: &str = "byok";
pub const BUNDLED_RELAY_ENDPOINT_KEY: &str = "bundled_inference_relay_endpoint";
pub const BUNDLED_TOKEN_EXPIRES_AT_KEY: &str = "bundled_inference_token_expires_at";
pub const BUNDLED_TOKEN_SCOPE_KEY: &str = "bundled_inference_token_scope";
pub const BUNDLED_INSTALLATION_ID_KEY: &str = "bundled_inference_installation_id";
pub const BUNDLED_RELAY_URL_ENV: &str = "JEFF_BUNDLED_RELAY_URL";

use crate::store::TaskStore;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn get_inference_mode(store: &TaskStore) -> String {
    store
        .get_app_setting(INFERENCE_MODE_KEY)
        .ok()
        .flatten()
        .filter(|value| value == INFERENCE_MODE_BUNDLED || value == INFERENCE_MODE_BYOK)
        .unwrap_or_else(|| INFERENCE_MODE_BYOK.to_string())
}

pub fn set_inference_mode(store: &TaskStore, mode: &str) -> Result<()> {
    if mode != INFERENCE_MODE_BUNDLED && mode != INFERENCE_MODE_BYOK {
        return Err(anyhow!("inference mode must be 'bundled' or 'byok'"));
    }
    store.set_app_setting(INFERENCE_MODE_KEY, mode)
}

pub fn bundled_inference_configured(store: &TaskStore) -> bool {
    bundled_inference_ready(
        store,
        crate::secrets::resolve_bundled_inference_token().is_some(),
    )
}

pub fn set_bundled_relay_endpoint(store: &TaskStore, endpoint: &str) -> Result<()> {
    let endpoint = validate_bundled_relay_endpoint(endpoint)?;
    store.set_app_setting(BUNDLED_RELAY_ENDPOINT_KEY, endpoint.as_str())
}

pub fn get_bundled_relay_endpoint(store: &TaskStore) -> Option<String> {
    store
        .get_app_setting(BUNDLED_RELAY_ENDPOINT_KEY)
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
}

fn validate_bundled_relay_endpoint(endpoint: &str) -> Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(endpoint.trim()).context("invalid bundled relay endpoint")?;
    let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if parsed.scheme() != "https" && !(local && parsed.scheme() == "http") {
        return Err(anyhow!(
            "bundled relay endpoint must use HTTPS except on localhost"
        ));
    }
    if parsed.username() != "" || parsed.password().is_some() || parsed.query().is_some() {
        return Err(anyhow!(
            "bundled relay endpoint cannot contain credentials or a query"
        ));
    }
    Ok(parsed)
}

fn bundled_inference_ready(store: &TaskStore, has_token: bool) -> bool {
    if !has_token || get_bundled_relay_endpoint(store).is_none() {
        return false;
    }
    let scope_ok = store
        .get_app_setting(BUNDLED_TOKEN_SCOPE_KEY)
        .ok()
        .flatten()
        .map(|scope| {
            scope
                .split_whitespace()
                .any(|item| item == "inference:chat")
        })
        .unwrap_or(false);
    let expiry_ok = store
        .get_app_setting(BUNDLED_TOKEN_EXPIRES_AT_KEY)
        .ok()
        .flatten()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
        .map(|expiry| expiry > chrono::Utc::now())
        .unwrap_or(false);
    scope_ok && expiry_ok
}

#[derive(Debug, Serialize)]
struct TokenRequest<'a> {
    installation_id: &'a str,
    scope: &'static str,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    token: String,
    expires_at: String,
    scope: String,
}

fn installation_id(store: &TaskStore) -> Result<String> {
    if let Some(existing) = store.get_app_setting(BUNDLED_INSTALLATION_ID_KEY)? {
        if !existing.trim().is_empty() {
            return Ok(existing);
        }
    }
    let seed = format!(
        "{}:{}:{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        std::thread::current().name().unwrap_or("jeff")
    );
    let id = format!("jeff-{:x}", Sha256::digest(seed.as_bytes()));
    store.set_app_setting(BUNDLED_INSTALLATION_ID_KEY, &id)?;
    Ok(id)
}

pub fn configure_bundled_inference(
    store: &TaskStore,
    endpoint_override: Option<&str>,
) -> Result<()> {
    let endpoint = endpoint_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| get_bundled_relay_endpoint(store))
        .or_else(|| std::env::var(BUNDLED_RELAY_URL_ENV).ok())
        .or_else(|| option_env!("JEFF_BUNDLED_RELAY_URL").map(str::to_string))
        .ok_or_else(|| anyhow!("bundled inference relay is not configured in this build"))?;
    let endpoint = validate_bundled_relay_endpoint(&endpoint)?;
    let token_url = endpoint
        .join("v1/tokens")
        .context("failed to build bundled token endpoint")?;
    let install_id = installation_id(store)?;
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("failed to build bundled relay client")?
        .post(token_url)
        .json(&TokenRequest {
            installation_id: &install_id,
            scope: "inference:chat",
        })
        .send()
        .context("failed to request bundled inference token")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(anyhow!(
            "bundled token request failed with status {status}: {body}"
        ));
    }
    let issued: TokenResponse = response
        .json()
        .context("failed to parse bundled token response")?;
    if !issued
        .scope
        .split_whitespace()
        .any(|scope| scope == "inference:chat")
    {
        return Err(anyhow!("bundled token is missing inference:chat scope"));
    }
    let expiry = chrono::DateTime::parse_from_rfc3339(&issued.expires_at)
        .context("bundled token expiry is invalid")?;
    if expiry <= chrono::Utc::now() {
        return Err(anyhow!("bundled token is already expired"));
    }
    crate::secrets::store_bundled_inference_token(&issued.token)?;
    set_bundled_relay_endpoint(store, endpoint.as_str())?;
    store.set_app_setting(BUNDLED_TOKEN_EXPIRES_AT_KEY, &issued.expires_at)?;
    store.set_app_setting(BUNDLED_TOKEN_SCOPE_KEY, &issued.scope)?;
    set_inference_mode(store, INFERENCE_MODE_BUNDLED)
}

// onboarding can complete without a key when bundled inference is chosen -- a
// sophomore never sees the words "API key". byok still requires a key. Fail
// closed: bundled is only "ready" once its relay is actually configured, so we
// never mark onboarding complete into a nonfunctional inference path.
pub fn onboarding_ready(store: &TaskStore, has_stored_api_key: bool) -> bool {
    onboarding_ready_with_token(
        store,
        has_stored_api_key,
        crate::secrets::resolve_bundled_inference_token().is_some(),
    )
}

fn onboarding_ready_with_token(
    store: &TaskStore,
    has_stored_api_key: bool,
    has_bundled_token: bool,
) -> bool {
    match get_inference_mode(store).as_str() {
        INFERENCE_MODE_BUNDLED => bundled_inference_ready(store, has_bundled_token),
        _ => has_stored_api_key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn e6_bundled_inference_completes_onboarding_without_a_key() {
        let (_dir, store) = store();
        // default is byok, which needs a key.
        assert_eq!(get_inference_mode(&store), INFERENCE_MODE_BYOK);
        assert!(!onboarding_ready(&store, false));
        assert!(onboarding_ready(&store, true));

        // choosing bundled does not by itself complete onboarding: the relay must
        // be configured first, or we would mark a nonfunctional config as ready.
        set_inference_mode(&store, INFERENCE_MODE_BUNDLED).unwrap();
        assert_eq!(get_inference_mode(&store), INFERENCE_MODE_BUNDLED);
        assert!(!onboarding_ready(&store, false));
        assert!(!bundled_inference_configured(&store));

        // once the relay endpoint is issued, bundled finishes with no key entry.
        set_bundled_relay_endpoint(&store, "https://relay.example/jeff/").unwrap();
        store
            .set_app_setting(BUNDLED_TOKEN_SCOPE_KEY, "inference:chat")
            .unwrap();
        store
            .set_app_setting(
                BUNDLED_TOKEN_EXPIRES_AT_KEY,
                &(chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            )
            .unwrap();
        assert!(bundled_inference_ready(&store, true));
        assert!(!bundled_inference_ready(&store, false));
        assert!(onboarding_ready_with_token(&store, false, true));

        // clearing the endpoint returns bundled to unconfigured (fail closed).
        assert!(set_bundled_relay_endpoint(&store, "http://relay.example/jeff").is_err());

        // an invalid mode is rejected.
        assert!(set_inference_mode(&store, "free-lunch").is_err());
    }
}
