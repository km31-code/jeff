mod ambient;
mod artifact_parser;
mod calendar;
mod chat;
mod chat_streaming;
mod chunking;
mod classifier;
mod commands;
mod context_observer;
mod coworking;
mod embedding;
mod errors;
mod flow;
mod latency;
mod login_item;
mod message_kind;
mod models;
mod onboarding;
mod proactive;
mod providers;
mod reasoning;
mod retrieval;
mod revision;
mod secrets;
mod selection_capture;
mod similarity;
mod state;
mod store;
mod streaming;
mod subtask;
mod typing_activity;
mod user_model;
mod voice;
mod voice_naturalness;
mod watcher;
mod workload;
mod workspace;

use std::sync::Arc;

use ambient::AmbientState;
use providers::VoiceProvider;
use reasoning::OpenAiReasoningProvider;
use retrieval::default_embeddings_provider;
use selection_capture::SelectionCaptureState;
use state::{CalendarState, ContextState, JeffState};
use store::TaskStore;
use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;
use typing_activity::TypingActivityState;
use voice::OpenAiVoiceProvider;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
fn main() {
    dotenvy::dotenv().ok();

    tauri::Builder::default()
        // single-instance plugin must be registered before any window work so
        // second-launch invocations are redirected to the already-running app.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let _ = ambient::show_overlay_interactive(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    // toggle on key press; ignore release so we do not
                    // double-fire. pressed-only keeps toggle latency tight.
                    if event.state == ShortcutState::Pressed {
                        if ambient::shortcut_matches(shortcut, ambient::DEFAULT_HOTKEY) {
                            let _ = ambient::toggle_overlay_interactive(app);
                        } else if ambient::shortcut_matches(
                            shortcut,
                            selection_capture::SELECTION_CAPTURE_HOTKEY,
                        ) {
                            selection_capture::capture_selection_from_hotkey(app);
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            let app_local_data_dir = app
                .path()
                .app_local_data_dir()
                .map_err(|error| format!("failed to resolve app local data directory: {error}"))?;

            let store = TaskStore::initialize(&app_local_data_dir)
                .map_err(|error| format!("failed to initialize local task store: {error}"))?;

            // phase 19: read persisted session settings before constructing
            // managed state so AmbientState starts in the correct mode without
            // a round-trip through the frontend after the window is shown.
            let quiet_mode = store.get_quiet_mode().unwrap_or(false);
            let overlay_expanded = store.get_overlay_expanded().unwrap_or(false);
            let launch_at_login = store.get_launch_at_login().unwrap_or(false);
            // detect first session: session_restored_at absent means jeff has
            // never completed a session on this install.
            let is_first_session = !store.get_session_restored_at().unwrap_or(false);
            if is_first_session {
                // mark immediately so a crash before the notification fires
                // does not cause a duplicate on the next launch.
                let _ = store.mark_session_restored();
            }

            let embeddings = Arc::new(default_embeddings_provider());
            let reasoning = Arc::new(OpenAiReasoningProvider::from_env());
            let voice: Arc<dyn VoiceProvider> = Arc::new(OpenAiVoiceProvider::from_env());

            // build AmbientState with restored quiet_mode and overlay_mode
            // applied so the overlay opens in the correct state immediately.
            let ambient_state = {
                let s = AmbientState::new();
                if quiet_mode {
                    s.set_quiet_mode(true);
                }
                if overlay_expanded {
                    s.set_overlay_mode(ambient::OverlayMode::Expanded);
                }
                s
            };

            app.manage(JeffState::new(store, embeddings, reasoning, voice));
            app.manage(ambient_state);
            // phase 20: manage context state for active-window polling.
            app.manage(ContextState::new());
            app.manage(SelectionCaptureState::new());
            let typing_enabled = app
                .state::<JeffState>()
                .store
                .get_privacy_typing_activity_enabled()
                .unwrap_or(true);
            app.manage(TypingActivityState::new(typing_enabled));
            // phase 23: manage calendar state for EventKit polling.
            app.manage(CalendarState::new());

            // phase 19: sync the macos SMAppService login-item registry with
            // the persisted preference. if registration fails, clear the
            // persisted setting so the tray checkmark cannot lie about state.
            if launch_at_login {
                match login_item::set_login_item_enabled(true) {
                    Ok(status) if status.is_enabled_or_pending() => {}
                    Ok(_) => {
                        let _ = app.state::<JeffState>().store.set_launch_at_login(false);
                    }
                    Err(err) => {
                        eprintln!("[jeff login-item] failed to sync launch at login: {err}");
                        let _ = app.state::<JeffState>().store.set_launch_at_login(false);
                    }
                }
            }

            {
                let state = app.state::<JeffState>();
                if let Err(err) = commands::restore_workspace_awareness_for_active_task(&state) {
                    eprintln!("[jeff watcher] failed to restore startup watcher state: {err}");
                }
            }

            let handle = app.handle().clone();
            selection_capture::start_browser_bridge(handle.clone());
            if let Some(typing_state) = handle.try_state::<TypingActivityState>() {
                typing_activity::start_global_typing_monitor(typing_state.clone_state());
            }

            // phase 11: main window starts hidden. jeff is tray-resident.
            // the full workspace is only shown on explicit user action.
            if let Some(main_window) = handle.get_webview_window(ambient::MAIN_WINDOW_LABEL) {
                let _ = main_window.hide();
                let hide_handle = handle.clone();
                main_window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(window) =
                            hide_handle.get_webview_window(ambient::MAIN_WINDOW_LABEL)
                        {
                            let _ = window.hide();
                        }
                    }
                });
            }

            ambient::build_overlay_window(&handle)
                .map_err(|error| format!("failed to build overlay window: {error}"))?;

            // phase 19: if overlay was restored as expanded, resize the window
            // now so it opens at the right height without a frontend round-trip.
            if overlay_expanded {
                let _ = ambient::resize_overlay_for_mode(&handle, ambient::OverlayMode::Expanded);
            }

            // pass initial toggle state so tray checkmarks are correct from
            // the first paint rather than waiting for a user interaction.
            ambient::install_tray(&handle, launch_at_login, quiet_mode)
                .map_err(|error| format!("failed to install tray icon: {error}"))?;

            // register the global hotkey. registration may fail if the combo
            // is already owned by another app; we surface the conflict via an
            // event and continue — the tray remains a working entry point.
            let _ = ambient::register_global_hotkey(&handle);

            // phase 20: spawn the active-window context polling loop.
            // polls NSWorkspace every 3 seconds (first poll is immediate).
            // emits context://context-updated after every poll so the frontend
            // can subscribe to the event instead of maintaining its own interval.
            // emits context://document-switch when the frontmost document title
            // changes to one not yet nudged this session, and the document is
            // off-task relative to the active task title.
            {
                use tauri::Emitter;
                let poll_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        // skip polling when quiet mode is active.
                        let quiet = poll_handle
                            .try_state::<AmbientState>()
                            .map(|s| s.is_quiet_mode())
                            .unwrap_or(false);

                        if quiet {
                            // still emit a null context so the frontend clears stale display.
                            let _ = poll_handle
                                .emit("context://context-updated", serde_json::Value::Null);
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            continue;
                        }

                        let active_context_allowed = poll_handle
                            .try_state::<JeffState>()
                            .map(|s| {
                                let onboarding_complete =
                                    s.store.get_onboarding_complete().unwrap_or(false);
                                let privacy_enabled = s
                                    .store
                                    .get_privacy_active_window_context_enabled()
                                    .unwrap_or(true);
                                onboarding_complete && privacy_enabled
                            })
                            .unwrap_or(false);

                        if !active_context_allowed {
                            if let Some(ctx_state) = poll_handle.try_state::<ContextState>() {
                                ctx_state.update(None);
                            }
                            let _ = poll_handle
                                .emit("context://context-updated", serde_json::Value::Null);
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            continue;
                        }

                        let new_ctx = context_observer::poll_active_window();

                        let Some(ctx_state) = poll_handle.try_state::<ContextState>() else {
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            continue;
                        };

                        // fire the document-switch nudge before updating state so
                        // we compare the incoming title against the last-known one.
                        if context_observer::is_accessibility_trusted() {
                            if let Some(ref ctx) = new_ctx {
                                let title = &ctx.document_title;
                                if !title.is_empty() && ctx_state.should_nudge_for_switch(title) {
                                    // suppress nudge when the document title matches
                                    // the active task title (user is on-task).
                                    let task_title = poll_handle
                                        .try_state::<JeffState>()
                                        .and_then(|s| s.store.get_active_task().ok().flatten())
                                        .map(|t| t.title);
                                    let off_task = task_title
                                        .as_deref()
                                        .map_or(true, |t| document_is_off_task(title, t));
                                    if off_task {
                                        let _ = poll_handle.emit(
                                            "context://document-switch",
                                            serde_json::json!({
                                                "app_name": ctx.app_name,
                                                "document_title": ctx.document_title,
                                            }),
                                        );
                                        ctx_state.mark_nudged(title.clone());
                                    }
                                }
                            }
                        }

                        ctx_state.update(new_ctx);

                        // emit context-updated so the frontend tracks current state
                        // without needing its own polling interval.
                        let context_payload = ctx_state.current().map(|ctx| {
                            serde_json::json!({
                                "app_name": ctx.app_name,
                                "document_title": ctx.document_title,
                                "captured_at": ctx.captured_at,
                            })
                        });
                        let _ = poll_handle.emit(
                            "context://context-updated",
                            context_payload.unwrap_or(serde_json::Value::Null),
                        );

                        // sleep at end so the first poll runs immediately on startup.
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                });
            }

            // phase 22: keep rate-only typing state in sync with privacy and
            // expose only a boolean event for the frontend tts queue.
            {
                use tauri::Emitter;
                let typing_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let mut last_typing: Option<bool> = None;
                    loop {
                        let Some(typing_state) = typing_handle.try_state::<TypingActivityState>()
                        else {
                            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                            continue;
                        };

                        let enabled = typing_handle
                            .try_state::<JeffState>()
                            .and_then(|state| {
                                state.store.get_privacy_typing_activity_enabled().ok()
                            })
                            .unwrap_or(true);
                        typing_state.set_enabled(enabled);
                        let is_typing = enabled && typing_state.is_typing();

                        if last_typing != Some(is_typing) {
                            last_typing = Some(is_typing);
                            let _ = typing_handle.emit(
                                "typing://activity-changed",
                                serde_json::json!({
                                    "is_typing": is_typing,
                                    "rate_only": true,
                                    "monitor_available": typing_state.monitor_available(),
                                    "last_error": typing_state.last_error(),
                                }),
                            );
                            if let Some(state) = typing_handle.try_state::<JeffState>() {
                                if let Ok(now) = coworking::unix_now_seconds() {
                                    if let Ok(mut runtime) = state.coworking.lock() {
                                        let _ = runtime.set_user_typing(is_typing, now);
                                    }
                                }
                            }
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    }
                });
            }

            // phase 23: calendar poll task (60-second interval).
            // polls EventKit for the next upcoming event when the privacy gate
            // is enabled and the OS permission has been granted.
            {
                let cal_poll_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        let quiet = cal_poll_handle
                            .try_state::<AmbientState>()
                            .map(|s| s.is_quiet_mode())
                            .unwrap_or(false);
                        if quiet {
                            if let Some(cs) = cal_poll_handle.try_state::<CalendarState>() {
                                cs.update(None);
                            }
                            use tauri::Emitter;
                            let _ = cal_poll_handle
                                .emit("calendar://event-updated", serde_json::Value::Null);
                            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                            continue;
                        }

                        let cal_enabled = cal_poll_handle
                            .try_state::<JeffState>()
                            .and_then(|s| s.store.get_privacy_calendar_context_enabled().ok())
                            .unwrap_or(false);

                        if !cal_enabled {
                            // clear stale event if feature is turned off mid-session
                            if let Some(cs) = cal_poll_handle.try_state::<CalendarState>() {
                                cs.update(None);
                            }
                            use tauri::Emitter;
                            let _ = cal_poll_handle
                                .emit("calendar://event-updated", serde_json::Value::Null);
                            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                            continue;
                        }

                        let next_event = calendar::fetch_next_event(8);

                        if let Some(cs) = cal_poll_handle.try_state::<CalendarState>() {
                            cs.update(next_event.clone());
                        }

                        use tauri::Emitter;
                        let _ = cal_poll_handle.emit(
                            "calendar://event-updated",
                            serde_json::to_value(&next_event).unwrap_or(serde_json::Value::Null),
                        );

                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }
                });
            }

            // phase 23: stale-task notification check at startup
            {
                let stale_check_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    // give the app a few seconds to finish startup before checking
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    if let Some(jeff_state) = stale_check_handle.try_state::<JeffState>() {
                        let quiet = stale_check_handle
                            .try_state::<AmbientState>()
                            .map(|s| s.is_quiet_mode())
                            .unwrap_or(false);
                        let _ = workload::check_stale_task_notifications(
                            &jeff_state.store,
                            &stale_check_handle,
                            quiet,
                        );
                    }
                });
            }

            // phase 24: background update check — delayed 2 seconds so it
            // does not compete with tray-ready or session-restore on startup.
            {
                let update_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    perform_update_check(update_handle).await;
                });
            }

            // phase 19: fire a one-time native notification on the very first
            // session so users who enabled launch-at-login know jeff is running
            // in the tray without needing to look for it.
            // set_focus is never called here; jeff must not steal focus on
            // automatic startup (phase 11 + phase 19 constraint).
            if is_first_session {
                let _ = ambient::dispatch_notification(
                    &handle,
                    ambient::NotificationPayload {
                        title: "Jeff is ready".to_string(),
                        body: format!("Press {} to bring it up.", ambient::DEFAULT_HOTKEY),
                        context_kind: None,
                        context_id: None,
                    },
                );
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_task,
            commands::list_tasks,
            commands::get_active_task,
            commands::set_active_task,
            commands::get_onboarding_status,
            commands::complete_onboarding,
            commands::set_preferred_workspace_folder,
            commands::clear_preferred_workspace_folder,
            commands::validate_openai_api_key,
            commands::store_openai_api_key,
            commands::delete_openai_api_key,
            commands::get_workspace_prompt_dismissed,
            commands::set_workspace_prompt_dismissed,
            commands::get_task_workspace,
            commands::get_task_summary,
            commands::list_open_resources,
            commands::import_artifact,
            commands::list_artifacts,
            commands::retrieve_context,
            commands::build_context_pack,
            commands::list_messages,
            commands::send_message,
            commands::cancel_interaction,
            commands::transcribe_audio,
            commands::synthesize_speech,
            commands::get_coworking_status,
            commands::set_proactive_mode,
            commands::set_user_typing,
            commands::set_user_speaking,
            commands::set_assistant_speaking,
            commands::evaluate_proactive_nudge,
            commands::get_artifact_content,
            commands::propose_artifact_revision,
            commands::list_pending_revisions,
            commands::list_task_pending_revisions,
            commands::apply_revision,
            commands::reject_revision,
            commands::list_artifact_versions,
            commands::revert_artifact_to_version,
            commands::create_subtask,
            commands::list_subtasks,
            commands::cancel_subtask,
            commands::accept_subtask_result,
            commands::reject_subtask_result,
            commands::suggest_subtask,
            commands::refine_subtask,
            commands::convert_subtask_to_revision,
            commands::evaluate_next_suggestions,
            commands::list_suggestions,
            commands::accept_suggestion,
            commands::dismiss_suggestion,
            commands::explain_suggestion,
            commands::get_session_mode_state,
            commands::list_recent_events,
            commands::get_active_artifact_selection,
            commands::set_active_artifact_selection,
            ambient::ambient_toggle_overlay,
            ambient::ambient_show_overlay,
            ambient::ambient_hide_overlay,
            ambient::ambient_show_workspace,
            ambient::ambient_open_privacy_center,
            ambient::ambient_open_onboarding,
            ambient::ambient_open_onboarding_at_step,
            ambient::ambient_hide_workspace,
            ambient::ambient_set_overlay_mode,
            ambient::ambient_set_tray_status,
            ambient::ambient_set_quiet_mode,
            ambient::ambient_get_state,
            ambient::ambient_quit_app,
            ambient::ambient_notify,
            ambient::ambient_notification_clicked,
            ambient::ambient_mark_notification_permission,
            commands::send_message_streaming,
            commands::cancel_streaming_turn,
            commands::start_workspace_watcher,
            commands::stop_workspace_watcher,
            commands::get_watcher_status,
            commands::ensure_workspace_watcher,
            commands::list_recently_learned,
            commands::set_clipboard_capture,
            commands::get_clipboard_capture_setting,
            commands::classify_message_intent,
            commands::trigger_task_resume,
            commands::check_task_drift,
            commands::trigger_speculative_subtask,
            commands::dismiss_proactive_trigger,
            commands::record_task_focus,
            // phase 16
            commands::list_subtask_steps,
            commands::list_file_write_proposals,
            commands::approve_subtask_file_write,
            commands::reject_subtask_file_write,
            commands::list_write_audit_log,
            commands::start_subtask_chain,
            // phase 19
            commands::get_launch_at_login,
            commands::set_launch_at_login,
            commands::get_session_restore_state,
            // phase 20
            commands::get_active_window_context,
            commands::get_accessibility_permission_status,
            commands::request_accessibility_permission,
            // phase 21
            commands::get_privacy_center_dashboard,
            commands::set_privacy_surface_enabled,
            commands::clear_user_profile_memory,
            commands::list_proactive_trigger_audit_log,
            commands::clear_active_task_data,
            commands::clear_all_jeff_data,
            // phase 22
            commands::get_selection_capture_indicator,
            commands::dismiss_selection_capture,
            commands::get_selection_bridge_status,
            commands::capture_browser_selection,
            commands::set_tts_voice,
            // phase 23: user profile
            commands::get_user_profile_signals,
            commands::add_quality_rubric,
            commands::delete_quality_rubric,
            commands::delete_user_profile_signal,
            // phase 23: workload
            commands::get_workload_summary,
            commands::switch_active_task_from_companion,
            // phase 23: calendar
            commands::request_calendar_permission,
            commands::get_calendar_permission_status,
            commands::get_calendar_next_event,
            // phase 23: live app actions
            commands::approve_live_edit,
            commands::reject_live_edit,
            commands::list_live_edit_receipts,
            commands::get_pending_live_edits,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Jeff desktop app");
}

// phase 24: silent background update check. runs after a short startup
// delay so tray-ready and session-restore finish first. shows a native
// dialog with Install / Later buttons; user-dismissed means next launch.
async fn perform_update_check(app: tauri::AppHandle) {
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

fn document_is_off_task(document_title: &str, task_title: &str) -> bool {
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
