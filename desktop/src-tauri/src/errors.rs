use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JeffError {
    ApiTimeout,
    InvalidApiKey,
    MissingOsPermission(String),
    DbLockContention,
}

impl Display for JeffError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            JeffError::ApiTimeout => {
                write!(
                    f,
                    "Jeff couldn't reach OpenAI — check your network connection."
                )
            }
            JeffError::InvalidApiKey => {
                write!(f, "Your API key isn't working. Open settings to update it.")
            }
            JeffError::MissingOsPermission(permission) => {
                write!(
                    f,
                    "Jeff needs {permission} to do this — open System Settings."
                )
            }
            JeffError::DbLockContention => {
                write!(f, "Jeff ran into a save conflict. Try again in a moment.")
            }
        }
    }
}

impl From<JeffError> for String {
    fn from(value: JeffError) -> Self {
        value.to_string()
    }
}

pub fn map_error_message(raw: &str) -> String {
    infer_jeff_error(raw)
        .map(String::from)
        .unwrap_or_else(|| raw.to_string())
}

fn infer_jeff_error(raw: &str) -> Option<JeffError> {
    let lower = raw.to_ascii_lowercase();

    if is_api_timeout(&lower) {
        return Some(JeffError::ApiTimeout);
    }

    if is_invalid_api_key(&lower) {
        return Some(JeffError::InvalidApiKey);
    }

    if is_db_lock_contention(&lower) {
        return Some(JeffError::DbLockContention);
    }

    if let Some(permission) = infer_permission_label(&lower) {
        return Some(JeffError::MissingOsPermission(permission));
    }

    None
}

fn is_api_timeout(lower: &str) -> bool {
    let mentions_timeout = lower.contains("timed out") || lower.contains("timeout");
    let mentions_remote = lower.contains("openai")
        || lower.contains("chat completions")
        || lower.contains("audio transcription")
        || lower.contains("audio speech")
        || lower.contains("embeddings");

    mentions_timeout && mentions_remote
}

fn is_invalid_api_key(lower: &str) -> bool {
    lower.contains("openai_api_key is not configured")
        || lower.contains("status 401")
        || lower.contains("unauthorized")
        || lower.contains("invalid_api_key")
        || lower.contains("incorrect api key")
}

fn is_db_lock_contention(lower: &str) -> bool {
    lower.contains("database is locked")
        || lower.contains("sqlite_busy")
        || lower.contains("sqlite_busy")
        || lower.contains("sqlitedatabasebusy")
        || lower.contains("busy timeout")
}

fn infer_permission_label(lower: &str) -> Option<String> {
    let mentions_permission = lower.contains("permission")
        || lower.contains("not allowed")
        || lower.contains("not authorized")
        || lower.contains("not granted")
        || lower.contains("denied")
        || lower.contains("axisprocesstrusted")
        || lower.contains("axuielement")
        || lower.contains("system settings");

    if !mentions_permission {
        return None;
    }

    if lower.contains("accessibility")
        || lower.contains("axisprocesstrusted")
        || lower.contains("axuielement")
    {
        return Some("Accessibility permission".to_string());
    }

    if lower.contains("notification") {
        return Some("notification permission".to_string());
    }

    if lower.contains("microphone") || lower.contains("record") {
        return Some("microphone permission".to_string());
    }

    if lower.contains("calendar") {
        return Some("calendar permission".to_string());
    }

    Some("required permission".to_string())
}
