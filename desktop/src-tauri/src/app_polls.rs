// apex f1b-2b: the tauri-coupled poll implementations and small helpers, moved
// out of main.rs into the shared lib. core_runtime (also in the lib) drives
// them, so the headless daemon binary can link the whole core rather than the
// core living inside the app binary.

use tauri::Manager;

use crate::ambient::AmbientState;
use crate::{
    awareness_core, context_observer, goal_extraction, memory, relational_model, speculation, state,
    work_understanding,
};

pub fn crisis_event_matches_context(event_title: &str, document_title: &str) -> bool {
    let document = document_title.to_ascii_lowercase();
    event_title
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 4)
        .any(|token| document.contains(&token))
}

// phase 24: silent background update check. runs after a short startup
// delay so tray-ready and session-restore finish first. shows a native
// dialog with Install / Later buttons; user-dismissed means next launch.
pub async fn perform_update_check(app: tauri::AppHandle) {
    use tauri_plugin_updater::UpdaterExt;
    let updater = match app.updater() {
        Ok(u) => u,
        Err(_) => return,
    };
    let update = match updater.check().await {
        Ok(Some(u)) => u,
        _ => return,
    };
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
    let install = app
        .dialog()
        .message(format!(
            "Jeff {} is available — install now?",
            update.version
        ))
        .title("Jeff Update Available")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install".into(),
            "Later".into(),
        ))
        .blocking_show();
    if install {
        let _ = update
            .download_and_install(|_chunk, _total| {}, || {})
            .await;
        app.restart();
    }
}

// phase 31: content observation polling loop.
pub async fn spawn_content_observation_poll(handle: tauri::AppHandle) {
    if std::env::var("JEFF_DISABLE_CONTENT_OBSERVATION").is_ok() {
        return;
    }
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(
            context_observer::CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS,
        ))
        .await;

        let quiet = handle
            .try_state::<AmbientState>()
            .map(|s| s.is_quiet_mode())
            .unwrap_or(false);
        if quiet {
            continue;
        }

        let Some(jeff_state) = handle.try_state::<state::JeffState>() else {
            continue;
        };
        let Some(task) = jeff_state.store.get_active_task().ok().flatten() else {
            continue;
        };
        let task_id = task.id;

        let enabled = jeff_state
            .store
            .get_content_observation_enabled(task_id)
            .unwrap_or(false);
        if !enabled {
            continue;
        }

        let failed_count = jeff_state
            .content_observation
            .lock()
            .ok()
            .map(|g| g.capture_failed_count)
            .unwrap_or(0);
        if failed_count >= 3 {
            continue;
        }

        let Some(pid) = context_observer::get_frontmost_pid() else {
            continue;
        };

        let text_opt = context_observer::read_ax_document_text(pid);

        // apex b1: drive the semantic document model outside the content
        // observation lock so per-paragraph embedding never blocks snapshot
        // assembly. raw text stays inside document_model; only the counts-only
        // summary crosses back out.
        let doc_summary = match text_opt.as_ref() {
            Some(text) => jeff_state.document_model.lock().ok().and_then(|mut dm| {
                let delta = dm.observe(task_id, text, jeff_state.embeddings.as_ref());
                // apex d8: a meaningful document delta invalidates precomputed
                // speculation for this task -- the cached answer may no longer
                // match the document.
                if delta.structure_changed
                    || !delta.added.is_empty()
                    || !delta.removed.is_empty()
                    || !delta.rewritten.is_empty()
                {
                    let _ = speculation::invalidate_for_task(&jeff_state.store, task_id);
                }
                dm.state(task_id)
            }),
            None => None,
        };

        let mut should_update_awareness = false;
        let mut work_understanding_text: Option<String> = None;
        if let Ok(mut guard) = jeff_state.content_observation.lock() {
            guard.capture_attempt_count += 1;
            match text_opt {
                None => {
                    guard.capture_failed_count += 1;
                }
                Some(text) => {
                    guard.capture_failed_count = 0;
                    let prior_wc = guard
                        .observation
                        .as_ref()
                        .map(|o| o.word_count)
                        .unwrap_or(0);
                    let prior_stable = guard
                        .observation
                        .as_ref()
                        .map(|o| o.stable_for_ticks)
                        .unwrap_or(0);
                    let prior_text_ref = guard.raw_text.clone();
                    let observation = context_observer::summarize_content_observation(
                        &text,
                        prior_text_ref.as_deref(),
                        prior_wc,
                        prior_stable,
                    );
                    if observation.content_changed {
                        work_understanding_text = Some(text.clone());
                    }
                    guard.last_captured_at = Some(observation.captured_at);
                    guard.prior_text = guard.raw_text.take();
                    guard.raw_text = Some(text);
                    guard.observation = Some(observation);
                    guard.source_origin = Some("native_accessibility".to_string());
                    guard.source_title = None;
                    if let Some(summary) = &doc_summary {
                        guard.document_paragraph_count = summary.paragraph_count;
                        guard.document_structure_changed = summary.structure_changed;
                        guard.document_max_churn = summary.max_churn;
                        guard.document_churn_hotspots = summary.churn_hotspot_count;
                    }
                    should_update_awareness = true;
                }
            }
        }

        if should_update_awareness {
            awareness_core::spawn_awareness_update(
                &handle,
                awareness_core::SnapshotTrigger::ContentObservation,
                task_id,
            );
        }
        if let Some(text) = work_understanding_text {
            work_understanding::maybe_spawn_work_understanding(&handle, task_id, text);
        }
    }
}

// apex b2: reflex-tier goal extraction on conversation lulls.
const GOAL_EXTRACTION_TICK_SECONDS: u64 = 15;
const GOAL_LULL_SETTLE_SECONDS: i64 = 30;
const GOAL_ACTIVITY_WINDOW_SECONDS: i64 = 30 * 60;

pub async fn spawn_goal_extraction_poll(handle: tauri::AppHandle) {
    if std::env::var("JEFF_DISABLE_GOAL_EXTRACTION").is_ok() {
        return;
    }
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(GOAL_EXTRACTION_TICK_SECONDS)).await;

        let Some(jeff_state) = handle.try_state::<state::JeffState>() else {
            continue;
        };
        let Some(task) = jeff_state.store.get_active_task().ok().flatten() else {
            continue;
        };
        if !jeff_state
            .store
            .get_privacy_user_profile_memory_enabled()
            .unwrap_or(false)
        {
            continue;
        }
        let task_id = task.id;

        // list_recent_chat_messages returns oldest -> newest, which is the
        // transcript order the extractor expects.
        let recent = jeff_state
            .store
            .list_recent_chat_messages(task_id, 20)
            .unwrap_or_default();
        let last_user_message = recent.iter().rev().find(|m| m.role == "user");
        let Some(last_user_message) = last_user_message else {
            continue;
        };
        let Some(last_user_turn) =
            awareness_core::parse_sqlite_datetime_to_unix(&last_user_message.created_at)
        else {
            continue;
        };

        // only extract on a settled lull after a recent user turn, and at most
        // once per (task, latest user message).
        let now = chrono::Utc::now().timestamp();
        let settled = now - last_user_turn;
        if !(GOAL_LULL_SETTLE_SECONDS..=GOAL_ACTIVITY_WINDOW_SECONDS).contains(&settled) {
            continue;
        }
        if !jeff_state
            .goal_extraction
            .should_extract(task_id, last_user_message.id)
        {
            continue;
        }

        // the reflex extractor is a blocking http call; keep it off the async
        // worker thread.
        let router = jeff_state.model_router.clone();
        let recent_for_worker = recent.clone();
        let extraction = match tokio::task::spawn_blocking(move || {
            (
                goal_extraction::extract_goal_with_fallback(&router, &recent_for_worker),
                memory::extract_memory_tags_with_fallback(&router, &recent_for_worker),
            )
        })
        .await
        {
            Ok((extraction, memory_tags)) => {
                if !memory_tags.is_empty() {
                    let store = jeff_state.store.clone();
                    let embeddings = jeff_state.embeddings.clone();
                    match tokio::task::spawn_blocking(move || {
                        memory::record_memory_tags_for_turn(
                            &store,
                            embeddings.as_ref(),
                            task_id,
                            &memory_tags,
                        )
                    })
                    .await
                    {
                        Ok(Ok(written)) if written > 0 => {
                            eprintln!("[jeff] memory_tags_recorded task={task_id} count={written}");
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => eprintln!("[jeff] memory_tag_record_failed: {err}"),
                        Err(err) => eprintln!("[jeff] memory_tag_join_failed: {err}"),
                    }
                }
                extraction
            }
            Err(err) => {
                eprintln!("[jeff] goal_extraction_join_failed: {err}");
                continue;
            }
        };

        if extraction.is_recordable() {
            if let Some(goal) = extraction.goal.as_deref() {
                if let Err(err) =
                    relational_model::record_goal_stated(&jeff_state.store, task_id, goal)
                {
                    eprintln!("[jeff] goal_extraction_record_failed: {err}");
                } else {
                    eprintln!(
                        "[jeff] goal_extracted task={} confidence={:.2}",
                        task_id, extraction.confidence
                    );
                }
            }
        }
    }
}

pub fn document_is_off_task(document_title: &str, task_title: &str) -> bool {
    let doc = document_title.trim().to_ascii_lowercase();
    let task = task_title.trim().to_ascii_lowercase();
    if doc.is_empty() || task.is_empty() {
        return true;
    }
    // require task title to be at least 5 chars before attempting suppression.
    // short names like "a", "my", or "doc" would match too broadly via
    // substring containment and suppress nudges the user should see.
    if task.len() < 5 {
        return true;
    }
    !(doc.contains(&task) || task.contains(&doc))
}

#[cfg(test)]
mod tests {
    use super::document_is_off_task;

    #[test]
    fn document_task_match_is_case_insensitive_and_substring_tolerant() {
        assert!(!document_is_off_task(
            "History Storymap Draft",
            "history storymap"
        ));
        assert!(!document_is_off_task(
            "history storymap",
            "History Storymap Draft"
        ));
        assert!(document_is_off_task("Chemistry Notes", "history storymap"));
    }

    #[test]
    fn short_task_titles_are_always_off_task_to_avoid_broad_suppression() {
        // "a", "my", "doc" are too short to meaningfully match document titles.
        assert!(document_is_off_task("a quick note", "a"));
        assert!(document_is_off_task("my documents", "my"));
        assert!(document_is_off_task("document.txt", "doc"));
        // exactly 5 chars is long enough to participate in matching.
        assert!(!document_is_off_task("notes on taxes", "notes"));
    }

    #[test]
    fn empty_inputs_are_always_off_task() {
        assert!(document_is_off_task("", "history notes"));
        assert!(document_is_off_task("history notes", ""));
        assert!(document_is_off_task("", ""));
    }
}
