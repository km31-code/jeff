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

use crate::store::TaskStore;
use anyhow::{anyhow, Result};

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

// the bundled relay is the one non-local component (opt-in, metered). Its
// endpoint/token issuance is env-gated; this reports whether it is configured.
pub fn bundled_inference_configured(store: &TaskStore) -> bool {
    store
        .get_app_setting(BUNDLED_RELAY_ENDPOINT_KEY)
        .ok()
        .flatten()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

// onboarding can complete without a key when bundled inference is chosen -- a
// sophomore never sees the words "API key". byok still requires a key.
pub fn onboarding_ready(store: &TaskStore, has_stored_api_key: bool) -> bool {
    match get_inference_mode(store).as_str() {
        INFERENCE_MODE_BUNDLED => true,
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

        // choosing bundled lets onboarding finish with no key entry.
        set_inference_mode(&store, INFERENCE_MODE_BUNDLED).unwrap();
        assert_eq!(get_inference_mode(&store), INFERENCE_MODE_BUNDLED);
        assert!(onboarding_ready(&store, false));

        // an invalid mode is rejected.
        assert!(set_inference_mode(&store, "free-lunch").is_err());
    }
}
