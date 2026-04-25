// user_model.rs — user profile signal storage and injection
//
// signals are local, never transmitted. all writes are async-safe via the
// TaskStore connection pool.

use anyhow::Result;

use crate::store::TaskStore;

// -------------------------------------------------------------------------
// core get/set (thin wrappers around store)
// -------------------------------------------------------------------------

#[allow(dead_code)]
pub fn get_profile_value(store: &TaskStore, key: &str) -> Result<Option<String>> {
    store.get_profile_value(key)
}

#[allow(dead_code)]
pub fn set_profile_value(store: &TaskStore, key: &str, value: &str) -> Result<()> {
    store.set_profile_value(key, value)
}

#[allow(dead_code)]
pub fn clear_all_profile(store: &TaskStore) -> Result<()> {
    store.clear_user_profile()
}

// -------------------------------------------------------------------------
// profile injection (prepended to system prompts when signals exist)
// -------------------------------------------------------------------------

/// returns a compact (< 100 token) profile context block, or None when the
/// table is empty or the privacy gate is off.
pub fn build_profile_injection(store: &TaskStore) -> Option<String> {
    let signals = store.get_all_profile_signals().ok()?;
    if signals.is_empty() {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();

    let style_len = signals
        .iter()
        .find(|(k, _, _)| k == "style_avg_sentence_length")
        .map(|(_, v, _)| v.parse::<f64>().ok())
        .flatten();
    let formality = signals
        .iter()
        .find(|(k, _, _)| k == "style_formality_score")
        .map(|(_, v, _)| v.parse::<f64>().ok())
        .flatten();
    if let Some(len) = style_len {
        let tone = match formality {
            Some(f) if f > 0.6 => "formal",
            _ => "concise",
        };
        parts.push(format!(
            "User prefers {tone} writing (~{:.0} words/sentence).",
            len
        ));
    }

    let pref_len = signals
        .iter()
        .find(|(k, _, _)| k == "response_length_preference")
        .map(|(_, v, _)| v.parse::<f64>().ok())
        .flatten();
    if let Some(len) = pref_len {
        parts.push(format!("Preferred response length: ~{:.0} words.", len));
    }

    let peak_hour = signals
        .iter()
        .find(|(k, _, _)| k == "work_rhythm_peak_hour")
        .map(|(_, v, _)| v.clone());
    if let Some(h) = peak_hour {
        if let Ok(hour) = h.parse::<u8>() {
            let period = if hour < 12 {
                "morning"
            } else if hour < 17 {
                "afternoon"
            } else {
                "evening"
            };
            parts.push(format!("User tends to focus work in the {period}."));
        }
    }

    // inject quality rubrics verbatim
    for (k, v, _) in &signals {
        if k.starts_with("rubric_") {
            parts.push(v.clone());
        }
    }

    if parts.is_empty() {
        return None;
    }

    Some(format!(
        "[User preferences — apply these when drafting or revising]\n{}",
        parts.join("\n")
    ))
}

// -------------------------------------------------------------------------
// signal writers
// -------------------------------------------------------------------------

/// called after a revision is accepted; updates style signals from the text.
pub fn record_revision_accepted(store: &TaskStore, accepted_text: &str) -> Result<()> {
    let sentence_count = count_sentences(accepted_text).max(1);
    let word_count = accepted_text.split_whitespace().count();
    let avg_len = word_count as f64 / sentence_count as f64;

    // running average with existing value
    let current_avg = store
        .get_profile_value("style_avg_sentence_length")?
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(avg_len);
    let new_avg = (current_avg * 0.7) + (avg_len * 0.3);
    store.set_profile_value("style_avg_sentence_length", &format!("{:.2}", new_avg))?;

    // formality: low contraction ratio = formal
    let contractions = count_contractions(accepted_text);
    let formality = if word_count == 0 {
        0.5
    } else {
        1.0 - (contractions as f64 / word_count as f64).min(1.0)
    };
    let current_f = store
        .get_profile_value("style_formality_score")?
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(formality);
    let new_f = (current_f * 0.7) + (formality * 0.3);
    store.set_profile_value("style_formality_score", &format!("{:.3}", new_f))?;

    Ok(())
}

/// called when a revision is accepted after the user significantly rewrites it.
pub fn record_revision_rewrite(store: &TaskStore, edited_text: &str) -> Result<()> {
    // significant rewrite → nudge toward shorter sentences and lower formality
    let sentence_count = count_sentences(edited_text).max(1);
    let word_count = edited_text.split_whitespace().count();
    let shorter = (word_count as f64 / sentence_count as f64) * 0.85;
    let current_avg = store
        .get_profile_value("style_avg_sentence_length")?
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(shorter);
    let new_avg = (current_avg * 0.7) + (shorter * 0.3);
    store.set_profile_value("style_avg_sentence_length", &format!("{:.2}", new_avg))?;

    let current_f = store
        .get_profile_value("style_formality_score")?
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.5);
    let new_f = (current_f * 0.7) + (0.4_f64 * 0.3); // pull toward lower formality
    store.set_profile_value("style_formality_score", &format!("{:.3}", new_f))?;

    Ok(())
}

/// called when a subtask result is accepted.
pub fn record_subtask_accepted(store: &TaskStore, execution_type: &str) -> Result<()> {
    let key = format!("delegation_accepted_{}", execution_type);
    let count = store
        .get_profile_value(&key)?
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    store.set_profile_value(&key, &(count + 1).to_string())?;
    Ok(())
}

/// called when a subtask result is rejected.
pub fn record_subtask_rejected(store: &TaskStore, execution_type: &str) -> Result<()> {
    let key = format!("delegation_rejected_{}", execution_type);
    let count = store
        .get_profile_value(&key)?
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    store.set_profile_value(&key, &(count + 1).to_string())?;
    Ok(())
}

/// called when task_focus_log gains a new entry; updates peak hour.
pub fn record_focus_hour(store: &TaskStore) -> Result<()> {
    use chrono::Timelike;
    let hour = chrono::Local::now().hour();
    // store a comma-separated history and pick the mode
    let key = "work_rhythm_focus_hours";
    let existing = store.get_profile_value(key)?.unwrap_or_default();
    let mut hours: Vec<u8> = existing.split(',').filter_map(|s| s.parse().ok()).collect();
    hours.push(hour as u8);
    // keep last 100 entries to bound storage
    if hours.len() > 100 {
        hours.drain(0..hours.len() - 100);
    }
    store.set_profile_value(
        key,
        &hours
            .iter()
            .map(|h| h.to_string())
            .collect::<Vec<_>>()
            .join(","),
    )?;

    // compute mode
    let mut counts = [0u32; 24];
    for h in &hours {
        counts[*h as usize] += 1;
    }
    let peak = counts
        .iter()
        .enumerate()
        .max_by_key(|(_, &c)| c)
        .map(|(i, _)| i)
        .unwrap_or(9);
    store.set_profile_value("work_rhythm_peak_hour", &peak.to_string())?;

    Ok(())
}

/// called after a non-dismissed Jeff response completes; updates preferred length.
pub fn record_response_length(store: &TaskStore, word_count: usize) -> Result<()> {
    let current = store
        .get_profile_value("response_length_preference")?
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(word_count as f64);
    let new_val = (current * 0.8) + (word_count as f64 * 0.2);
    store.set_profile_value("response_length_preference", &format!("{:.1}", new_val))?;
    Ok(())
}

/// word-level rewrite ratio based on longest common subsequence distance.
/// 0.0 means effectively unchanged; 1.0 means no word overlap in order.
pub fn word_level_diff_ratio(original: &str, edited: &str) -> f64 {
    let original_words = normalized_words(original);
    let edited_words = normalized_words(edited);
    if original_words.is_empty() && edited_words.is_empty() {
        return 0.0;
    }
    if original_words.is_empty() || edited_words.is_empty() {
        return 1.0;
    }
    let lcs = lcs_len(&original_words, &edited_words);
    1.0 - (lcs as f64 / original_words.len().max(edited_words.len()) as f64)
}

/// called when a proactive trigger is dismissed; down-weights the trigger type.
pub fn record_trigger_dismissed(store: &TaskStore, trigger_type: &str) -> Result<()> {
    let key = format!("trigger_weight_{}", trigger_type);
    let current = store
        .get_profile_value(&key)?
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.0);
    let new_val = (current - 0.1).max(0.1);
    store.set_profile_value(&key, &format!("{:.2}", new_val))?;
    Ok(())
}

/// adds a quality rubric; returns the key used.
pub fn add_quality_rubric(store: &TaskStore, text: &str) -> Result<String> {
    let existing = store.get_all_profile_signals()?;
    let next_n = existing
        .iter()
        .filter(|(k, _, _)| k.starts_with("rubric_"))
        .count();
    let key = format!("rubric_{}", next_n);
    store.set_profile_value(&key, text)?;
    Ok(key)
}

// -------------------------------------------------------------------------
// plain-language summaries for "Jeff remembers" panel
// -------------------------------------------------------------------------

pub struct SignalSummary {
    pub key: String,
    pub label: String,
    pub value: String,
    pub updated_at: String,
}

pub fn get_readable_signals(store: &TaskStore) -> Result<Vec<SignalSummary>> {
    let signals = store.get_all_profile_signals()?;
    let mut out = Vec::new();
    for (key, value, updated_at) in signals {
        // skip internal focus-hours accumulator; only show the peak
        if key == "work_rhythm_focus_hours" {
            continue;
        }
        let label = readable_label(&key, &value);
        out.push(SignalSummary {
            key: key.clone(),
            label,
            value,
            updated_at,
        });
    }
    Ok(out)
}

fn readable_label(key: &str, value: &str) -> String {
    if key == "style_avg_sentence_length" {
        let n: f64 = value.parse().unwrap_or(0.0);
        return format!("You write in sentences of about {:.0} words.", n);
    }
    if key == "style_formality_score" {
        let f: f64 = value.parse().unwrap_or(0.5);
        let adj = if f > 0.6 { "formal" } else { "conversational" };
        return format!("Your writing style is {}.", adj);
    }
    if key == "response_length_preference" {
        let n: f64 = value.parse().unwrap_or(0.0);
        return format!("You prefer responses of about {:.0} words.", n);
    }
    if key == "work_rhythm_peak_hour" {
        let h: u8 = value.parse().unwrap_or(9);
        let period = if h < 12 {
            "morning"
        } else if h < 17 {
            "afternoon"
        } else {
            "evening"
        };
        return format!("You tend to focus work in the {}.", period);
    }
    if key.starts_with("delegation_accepted_") {
        let et = key
            .trim_start_matches("delegation_accepted_")
            .replace('_', " ");
        return format!("You often accept {} subtasks.", et);
    }
    if key.starts_with("delegation_rejected_") {
        let et = key
            .trim_start_matches("delegation_rejected_")
            .replace('_', " ");
        return format!("You often decline {} subtasks.", et);
    }
    if key.starts_with("rubric_") {
        return format!("Quality note: {}", value);
    }
    if key.starts_with("trigger_weight_") {
        let t = key.trim_start_matches("trigger_weight_").replace('_', " ");
        let w: f64 = value.parse().unwrap_or(1.0);
        if w < 0.5 {
            return format!("You prefer fewer {} suggestions.", t);
        }
    }
    format!("{}: {}", key, value)
}

// -------------------------------------------------------------------------
// helpers
// -------------------------------------------------------------------------

fn count_sentences(text: &str) -> usize {
    text.chars()
        .filter(|&c| c == '.' || c == '!' || c == '?')
        .count()
        .max(1)
}

fn count_contractions(text: &str) -> usize {
    // simple: count apostrophes between alphabetic chars as contraction indicators
    let bytes = text.as_bytes();
    let mut count = 0usize;
    for i in 1..bytes.len().saturating_sub(1) {
        if bytes[i] == b'\''
            && bytes[i - 1].is_ascii_alphabetic()
            && bytes[i + 1].is_ascii_alphabetic()
        {
            count += 1;
        }
    }
    count
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_ascii_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn lcs_len(left: &[String], right: &[String]) -> usize {
    let mut prev = vec![0usize; right.len() + 1];
    let mut curr = vec![0usize; right.len() + 1];
    for left_word in left {
        for (j, right_word) in right.iter().enumerate() {
            curr[j + 1] = if left_word == right_word {
                prev[j] + 1
            } else {
                curr[j].max(prev[j + 1])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.fill(0);
    }
    prev[right.len()]
}

// -------------------------------------------------------------------------
// tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TaskStore;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, TaskStore) {
        let dir = TempDir::new().expect("tempdir");
        let store = TaskStore::initialize(dir.path()).expect("store");
        (dir, store)
    }

    #[test]
    fn profile_set_get_roundtrip() {
        let (_dir, store) = test_store();
        set_profile_value(&store, "style_avg_sentence_length", "12.50").unwrap();
        let v = get_profile_value(&store, "style_avg_sentence_length").unwrap();
        assert_eq!(v.as_deref(), Some("12.50"));
    }

    #[test]
    fn build_profile_injection_empty_returns_none() {
        let (_dir, store) = test_store();
        assert!(build_profile_injection(&store).is_none());
    }

    #[test]
    fn build_profile_injection_with_signals_returns_some() {
        let (_dir, store) = test_store();
        set_profile_value(&store, "style_avg_sentence_length", "12.0").unwrap();
        let injection = build_profile_injection(&store);
        assert!(injection.is_some());
        assert!(injection.unwrap().contains("words/sentence"));
    }

    #[test]
    fn clear_all_profile_empties_table() {
        let (_dir, store) = test_store();
        set_profile_value(&store, "foo", "bar").unwrap();
        clear_all_profile(&store).unwrap();
        assert!(build_profile_injection(&store).is_none());
    }

    #[test]
    fn record_revision_accepted_updates_style_signals() {
        let (_dir, store) = test_store();
        let text = "This is a sentence. Here is another one. And one more.";
        record_revision_accepted(&store, text).unwrap();
        let v = get_profile_value(&store, "style_avg_sentence_length").unwrap();
        assert!(v.is_some());
    }

    #[test]
    fn add_quality_rubric_stores_and_is_injected() {
        let (_dir, store) = test_store();
        add_quality_rubric(&store, "Always cite sources.").unwrap();
        let injection = build_profile_injection(&store).unwrap();
        assert!(injection.contains("Always cite sources."));
    }

    #[test]
    fn record_trigger_dismissed_decrements_weight() {
        let (_dir, store) = test_store();
        record_trigger_dismissed(&store, "reorientation").unwrap();
        let v = get_profile_value(&store, "trigger_weight_reorientation").unwrap();
        let weight: f64 = v.unwrap().parse().unwrap();
        assert!((weight - 0.90).abs() < 0.01);
    }

    #[test]
    fn word_level_diff_ratio_detects_same_length_rewrite() {
        let original = "This claim describes the source but does not analyze citizenship.";
        let edited = "Evidence connects citizenship debates to policy choices and legal power.";
        assert!(word_level_diff_ratio(original, edited) > 0.30);
        assert!(word_level_diff_ratio(original, original) < 0.01);
    }
}
