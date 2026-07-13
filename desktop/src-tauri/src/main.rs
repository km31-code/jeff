use std::sync::{mpsc, Arc};

use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::ShortcutState;

use jeff_desktop::ambient::{self, AmbientState};
use jeff_desktop::providers::VoiceProvider;
use jeff_desktop::retrieval::default_embeddings_provider;
use jeff_desktop::selection_capture::{self, SelectionCaptureState};
use jeff_desktop::state::{CalendarState, ContextState, JeffState};
use jeff_desktop::store::TaskStore;
use jeff_desktop::typing_activity::{self, TypingActivityState};
use jeff_desktop::voice::OpenAiVoiceProvider;
use jeff_desktop::{
    daemon_ipc, daemon_supervisor,
    awareness_core, commands, core_runtime, crisis, local_runtime, login_item, model_router,
    subtask, wake_word,
};

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
            //
            // apex f1b-3: if a daemon is up and hosting the core, it owns the
            // mutating background schedulers (standing jobs, job resume,
            // speculation) and this app runs as a client -- perception, world
            // model, and UI loops only -- so the schedulers never double-run
            // against the shared store. if the daemon is absent, unreachable, or
            // speaks a different protocol, the app runs the full core exactly as
            // it did before, and nothing is lost.
            // apex f1b-3b: the app owns the daemon's lifecycle. if the user
            // enabled the background daemon in the Privacy Center, start it (or
            // adopt the one already running). it defaults to off.
            let daemon_socket = daemon_ipc::default_socket_path(&app_local_data_dir);
            let daemon = daemon_supervisor::ensure_running(
                &app.state::<JeffState>().store,
                &daemon_socket,
            );
            let profile = if daemon.owns_background_schedulers() {
                eprintln!("[jeff] daemon is hosting the core; running as client");
                core_runtime::CoreProfile::AppClient
            } else {
                core_runtime::CoreProfile::Full
            };
            let _core = core_runtime::start(
                Arc::new(core_runtime::TauriHost::new(handle.clone())),
                profile,
            );

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
            // apex f1b-3b: background daemon control
            commands::get_background_daemon,
            commands::set_background_daemon_enabled,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Jeff desktop app");
}
