mod action_bus;
mod agent_runtime;
mod ambient;
mod artifact_parser;
mod awareness_core;
mod briefing;
mod calendar;
mod calendar_core;
mod character;
mod chat;
mod chat_streaming;
mod chunking;
mod classifier;
mod commands;
mod consolidation;
mod context_observer;
mod core_runtime;
mod cost_governor;
mod coworking;
mod crisis;
mod crisis_core;
mod document_model;
mod drive_core;
mod embedding;
mod errors;
mod flow;
mod gmail_core;
mod goal_extraction;
mod judgment_eval_core;
mod latency;
mod local_runtime;
mod login_item;
mod memory;
mod message_kind;
mod model_router;
mod models;
mod native_docs;
mod onboarding;
mod proactive;
mod providers;
mod reasoning;
mod relational_model;
mod retrieval;
mod revision;
mod secrets;
mod selection_capture;
mod self_extend;
mod similarity;
mod speculation;
mod state;
mod store;
mod streaming;
mod subtask;
mod synthesis;
mod tool_bus;
mod trust;
mod typing_activity;
mod user_model;
mod voice;
mod voice_naturalness;
mod voice_session;
mod wake_word;
mod watcher;
mod web_tools;
mod work_understanding;
mod workload;
mod workspace;

use std::sync::{mpsc, Arc};

use ambient::AmbientState;
use providers::VoiceProvider;
use retrieval::default_embeddings_provider;
use selection_capture::SelectionCaptureState;
use state::{CalendarState, ContextState, JeffState};
use store::TaskStore;
use tauri::{Emitter, Manager};
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
                            // d2: when overlay is already visible, emit a hotkey event
                            // so the frontend can handle barge-in vs hide.
                            // when hidden, show the overlay as before.
                            let overlay_visible = app
                                .get_webview_window(ambient::OVERLAY_WINDOW_LABEL)
                                .and_then(|w| w.is_visible().ok())
                                .unwrap_or(false);
                            if overlay_visible {
                                let _ = app.emit(
                                    "ambient://hotkey-pressed",
                                    serde_json::json!({ "overlay_visible": true }),
                                );
                            } else {
                                let _ = ambient::show_overlay_interactive(app);
                            }
                        } else if ambient::shortcut_matches(shortcut, ambient::MIC_SHORTCUT) {
                            // d3: mic shortcut — frontend toggles mic on/off.
                            let _ = app.emit("ambient://mic-shortcut", serde_json::json!({}));
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

            let local_runtime = Arc::new(local_runtime::LocalRuntime::new(&app_local_data_dir));
            let embeddings = Arc::new(default_embeddings_provider(local_runtime.clone()));
            // apex a1: all reasoning paths route through the model router.
            // tier config is loaded from app_settings with spec defaults.
            let model_router = Arc::new(model_router::ModelRouter::from_store_with_local_runtime(
                &store,
                Some(local_runtime.clone()),
            ));
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

            let jeff_state = JeffState::new(store, embeddings, local_runtime, model_router, voice);
            // register the file-indexed emit callback on the watcher so it can
            // fire workspace://file-indexed without holding an AppHandle directly.
            {
                let emit_app = app.handle().clone();
                let mut watcher = jeff_state
                    .watcher
                    .lock()
                    .expect("watcher state lock poisoned");
                watcher.set_file_indexed_notify(std::sync::Arc::new(
                    move |task_id: i64, file_name: String| {
                        let _ = emit_app.emit(
                            "workspace://file-indexed",
                            serde_json::json!({ "task_id": task_id, "file_name": file_name }),
                        );
                    },
                ));
            }
            {
                let emit_app = app.handle().clone();
                let mut watcher = jeff_state
                    .watcher
                    .lock()
                    .expect("watcher state lock poisoned");
                watcher.set_mass_deletion_notify(std::sync::Arc::new(
                    move |task_id: i64, removed_count: usize, known_file_count: usize| {
                        crisis::fire_data_loss_risk(
                            &emit_app,
                            task_id,
                            removed_count,
                            known_file_count,
                        );
                    },
                ));
            }
            {
                let (companion_tx, companion_rx) =
                    mpsc::sync_channel::<subtask::CompanionEvent>(64);
                jeff_state.subtasks.set_companion_notify(companion_tx);
                let emit_app = app.handle().clone();
                std::thread::spawn(move || {
                    while let Ok(event) = companion_rx.recv() {
                        match event {
                            subtask::CompanionEvent::Started {
                                subtask_id,
                                task_id,
                                title,
                            } => {
                                let _ = emit_app.emit(
                                    "subtask://companion-started",
                                    serde_json::json!({
                                        "subtask_id": subtask_id,
                                        "task_id": task_id,
                                        "title": title,
                                    }),
                                );
                            }
                            subtask::CompanionEvent::Complete {
                                subtask_id,
                                task_id,
                                final_status,
                            } => {
                                awareness_core::spawn_awareness_update(
                                    &emit_app,
                                    awareness_core::SnapshotTrigger::SubtaskCompleted,
                                    task_id,
                                );
                                let _ = emit_app.emit(
                                    "subtask://companion-complete",
                                    serde_json::json!({
                                        "subtask_id": subtask_id,
                                        "task_id": task_id,
                                        "final_status": final_status,
                                    }),
                                );
                            }
                            subtask::CompanionEvent::WriteProposal(proposal) => {
                                let _ =
                                    emit_app.emit("subtask://companion-write-proposal", proposal);
                            }
                        }
                    }
                });
            }
            app.manage(jeff_state);
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

            // phase 11: the hidden workspace window also closes to hide.
            if let Some(main_window) = app.get_webview_window("main") {
                let main_window_for_close = main_window.clone();
                main_window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = main_window_for_close.hide();
                    }
                });
            }

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

            // single-window design: the overlay is the only window.
            // workspace mode resizes it; there is no separate main window.
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

            if let Some(state) = handle.try_state::<JeffState>() {
                wake_word::maybe_start_from_settings(&state.wake_word, &state.store, &handle);
                let armed = state.wake_word.status(&state.store).armed;
                ambient::update_wake_word_armed(&handle, armed);
            }

            // register the global hotkey. registration may fail if the combo
            // is already owned by another app; we surface the conflict via an
            // event and continue — the tray remains a working entry point.
            let _ = ambient::register_global_hotkey(&handle);

            // apex f1a: every recurring background scheduler and startup task a
            // headless daemon must run lives in core_runtime; the tauri setup
            // closure only wires the shell and then starts the core.
            // apex f1b-1: the core runs against a CoreHost seam, not the raw
            // AppHandle. in-process that seam is TauriHost.
            let _core = core_runtime::start(Arc::new(core_runtime::TauriHost::new(
                handle.clone(),
            )));

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
            commands::store_anthropic_api_key,
            commands::delete_anthropic_api_key,
            commands::get_anthropic_key_configured,
            commands::get_tier_model_map,
            commands::set_tier_model_map,
            commands::debug_llm_cache_metrics,
            commands::get_local_runtime_status,
            commands::start_local_runtime,
            commands::stop_local_runtime,
            commands::download_local_model,
            commands::download_curated_embedding_model,
            commands::delete_local_model,
            commands::get_cost_governor_status,
            commands::get_interruption_audit,
            commands::get_debrief_enabled,
            commands::set_debrief_enabled,
            commands::get_voice_config,
            commands::set_voice_config,
            commands::start_voice_session,
            commands::get_wake_word_status,
            commands::set_wake_word_enabled,
            commands::set_crisis_class_enabled,
            commands::record_crisis_feedback,
            commands::persist_voice_transcript,
            commands::handle_voice_tool_call,
            commands::set_llm_daily_budget,
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
            commands::generate_revision_alternative,
            commands::list_revision_alternatives,
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
            commands::create_agent_job,
            commands::list_agent_jobs,
            commands::get_agent_job_detail,
            commands::run_agent_job,
            commands::send_job_steering,
            commands::cancel_agent_job,
            commands::resume_agent_jobs,
            commands::create_standing_job,
            commands::list_standing_jobs,
            commands::run_due_standing_jobs,
            commands::set_standing_job_enabled,
            commands::get_speculation_status,
            commands::set_speculation_enabled,
            commands::list_speculation_cache,
            commands::discard_speculation_cache_entry,
            commands::serve_speculation,
            commands::list_capability_gaps,
            commands::list_custom_tools,
            commands::propose_custom_tool,
            commands::approve_custom_tool,
            commands::kill_custom_tool,
            commands::run_custom_tool,
            commands::approve_custom_tool_run,
            commands::reject_custom_tool_run,
            commands::list_tool_connections,
            commands::add_tool_connection,
            commands::set_tool_connection_enabled,
            commands::remove_tool_connection,
            commands::list_connection_tools,
            commands::list_tool_call_log,
            commands::discover_connection_tools,
            commands::invoke_tool,
            commands::list_web_query_log,
            commands::set_web_user_name_guard,
            commands::set_web_corpus_dir,
            commands::web_search,
            commands::web_fetch,
            commands::triage_inbox,
            commands::summarize_email_thread,
            commands::register_email_reply_watch,
            commands::list_email_reply_watches,
            commands::draft_email_reply,
            commands::propose_email_labels,
            commands::approve_connected_action,
            commands::reject_connected_action,
            commands::poll_email_reply_watches,
            commands::full_day_calendar,
            commands::pre_meeting_prep,
            commands::propose_calendar_event,
            commands::pull_remote_doc,
            commands::list_remote_docs,
            commands::remove_remote_doc,
            commands::set_inference_mode,
            commands::configure_bundled_inference,
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
            ambient::ambient_set_workspace_mode,
            ambient::ambient_open_privacy_center,
            ambient::ambient_open_onboarding,
            ambient::ambient_open_onboarding_at_step,
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
            commands::get_situational_snapshot,
            // phase 16
            commands::list_subtask_steps,
            commands::list_file_write_proposals,
            commands::approve_subtask_file_write,
            commands::reject_subtask_file_write,
            commands::list_write_audit_log,
            commands::list_action_receipts,
            commands::revert_action_receipt,
            commands::request_google_docs_write,
            commands::get_native_docs_status,
            commands::request_native_doc_write,
            commands::list_trust_ladder,
            commands::set_trust_level,
            commands::demote_trust_class,
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
            commands::list_episodes,
            commands::search_episodes,
            commands::delete_episode,
            commands::clear_memory_episodes,
            commands::list_facts,
            commands::delete_fact,
            commands::clear_memory_facts,
            commands::run_memory_consolidation,
            commands::preview_memory_prompt_context,
            // phase 30
            commands::get_relational_profile,
            commands::delete_stated_goal,
            commands::delete_struggle_pattern,
            commands::clear_relational_profile,
            commands::list_proactive_trigger_audit_log,
            commands::get_synthesis_log,
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
            // phase 31: content observation
            commands::set_content_observation_enabled,
            commands::get_content_observation_enabled,
            commands::clear_content_observation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Jeff desktop app");
}

fn crisis_event_matches_context(event_title: &str, document_title: &str) -> bool {
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

// phase 31: content observation polling loop.
async fn spawn_content_observation_poll(handle: tauri::AppHandle) {
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

async fn spawn_goal_extraction_poll(handle: tauri::AppHandle) {
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

const MEMORY_SESSION_SUMMARY_TICK_SECONDS: u64 = 60;
const MEMORY_SESSION_IDLE_SECONDS: i64 = 30 * 60;
const MEMORY_CONSOLIDATION_TICK_SECONDS: u64 = 60;
const MEMORY_CONSOLIDATION_IDLE_SECONDS: i64 = 10 * 60;
const MEMORY_CONSOLIDATION_LAST_2AM_KEY: &str = "memory_consolidation:last_2am_run";

async fn spawn_memory_session_summary_poll(handle: tauri::AppHandle) {
    if std::env::var("JEFF_DISABLE_MEMORY_SUMMARY").is_ok() {
        return;
    }
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(
            MEMORY_SESSION_SUMMARY_TICK_SECONDS,
        ))
        .await;

        let Some(jeff_state) = handle.try_state::<state::JeffState>() else {
            continue;
        };
        if !jeff_state
            .store
            .get_privacy_user_profile_memory_enabled()
            .unwrap_or(false)
        {
            continue;
        }

        let tasks = jeff_state.store.list_tasks().unwrap_or_default();
        for task in tasks {
            let store = jeff_state.store.clone();
            let embeddings = jeff_state.embeddings.clone();
            let router = jeff_state.model_router.clone();
            let task_id = task.id;
            match tokio::task::spawn_blocking(move || {
                memory::record_idle_session_summary_if_due(
                    &store,
                    embeddings.as_ref(),
                    &router,
                    task_id,
                    MEMORY_SESSION_IDLE_SECONDS,
                )
            })
            .await
            {
                Ok(Ok(Some(_episode))) => {
                    eprintln!("[jeff] memory_session_summary_recorded task={task_id}");
                }
                Ok(Ok(None)) => {}
                Ok(Err(err)) => eprintln!("[jeff] memory_session_summary_failed: {err}"),
                Err(err) => eprintln!("[jeff] memory_session_summary_join_failed: {err}"),
            }
        }
    }
}

async fn spawn_memory_consolidation_poll(handle: tauri::AppHandle) {
    if std::env::var("JEFF_DISABLE_MEMORY_CONSOLIDATION").is_ok() {
        return;
    }
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(
            MEMORY_CONSOLIDATION_TICK_SECONDS,
        ))
        .await;

        let Some(jeff_state) = handle.try_state::<state::JeffState>() else {
            continue;
        };
        if !jeff_state
            .store
            .get_privacy_user_profile_memory_enabled()
            .unwrap_or(false)
        {
            continue;
        }
        if consolidation::unconsolidated_episode_count(&jeff_state.store).unwrap_or(0) == 0 {
            continue;
        }

        let now = chrono::Utc::now().timestamp();
        let idle_due = jeff_state
            .store
            .list_tasks()
            .unwrap_or_default()
            .into_iter()
            .any(|task| {
                let recent = jeff_state
                    .store
                    .list_recent_chat_messages(task.id, 1)
                    .unwrap_or_default();
                let Some(message) = recent.last() else {
                    return true;
                };
                let Some(last_at) =
                    awareness_core::parse_sqlite_datetime_to_unix(&message.created_at)
                else {
                    return false;
                };
                now.saturating_sub(last_at) >= MEMORY_CONSOLIDATION_IDLE_SECONDS
            });
        let local_now = chrono::Local::now();
        let today = local_now.format("%Y-%m-%d").to_string();
        let two_am_due = local_now.format("%H").to_string() == "02"
            && jeff_state
                .store
                .get_app_setting(MEMORY_CONSOLIDATION_LAST_2AM_KEY)
                .ok()
                .flatten()
                .as_deref()
                != Some(today.as_str());

        if !idle_due && !two_am_due {
            continue;
        }

        let store = jeff_state.store.clone();
        let embeddings = jeff_state.embeddings.clone();
        let router = jeff_state.model_router.clone();
        match tokio::task::spawn_blocking(move || {
            consolidation::run_consolidation(&store, embeddings.as_ref(), &router)
        })
        .await
        {
            Ok(Ok(report)) => {
                eprintln!(
                    "[jeff] memory_consolidation_complete processed={} upserted={} merged={} dropped={}",
                    report.processed_episode_count,
                    report.upserted_fact_count,
                    report.merged_fact_count,
                    report.dropped_fact_count
                );
                if two_am_due {
                    let _ = jeff_state
                        .store
                        .set_app_setting(MEMORY_CONSOLIDATION_LAST_2AM_KEY, &today);
                }
            }
            Ok(Err(err)) => eprintln!("[jeff] memory_consolidation_failed: {err}"),
            Err(err) => eprintln!("[jeff] memory_consolidation_join_failed: {err}"),
        }
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
