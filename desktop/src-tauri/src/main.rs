mod ambient;
mod artifact_parser;
mod chat;
mod chat_streaming;
mod chunking;
mod classifier;
mod commands;
mod coworking;
mod embedding;
mod errors;
mod flow;
mod latency;
mod message_kind;
mod models;
mod onboarding;
mod proactive;
mod providers;
mod reasoning;
mod retrieval;
mod revision;
mod secrets;
mod similarity;
mod state;
mod store;
mod streaming;
mod subtask;
mod voice;
mod watcher;
mod workspace;

use std::sync::Arc;

use ambient::AmbientState;
use reasoning::OpenAiReasoningProvider;
use retrieval::default_embeddings_provider;
use state::JeffState;
use store::TaskStore;
use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;
use voice::OpenAiVoiceProvider;
use providers::VoiceProvider;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
fn main() {
    dotenvy::dotenv().ok();

    tauri::Builder::default()
        // single-instance plugin must be registered before any window work so
        // second-launch invocations are redirected to the already-running app.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let _ = ambient::show_overlay(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        // phase 19: login-item registration via macOS LaunchAgent mechanism.
        // tauri-plugin-autostart wraps the OS registration so set_launch_at_login
        // and get_launch_at_login commands can sync jeff's persisted preference
        // with the actual OS login-item state without shell invocations.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // toggle on key press; ignore release so we do not
                    // double-fire. pressed-only keeps toggle latency tight.
                    if event.state == ShortcutState::Pressed {
                        let _ = ambient::toggle_overlay(app);
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

            // phase 19: sync the OS login-item registry with the persisted
            // preference. only enable; never silently disable (the user might
            // have registered jeff via another path we do not control).
            if launch_at_login {
                use tauri_plugin_autostart::ManagerExt;
                let _ = app.autolaunch().enable();
            }

            {
                let state = app.state::<JeffState>();
                if let Err(err) = commands::restore_workspace_awareness_for_active_task(&state) {
                    eprintln!("[jeff watcher] failed to restore startup watcher state: {err}");
                }
            }

            let handle = app.handle().clone();

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
                        body: format!(
                            "Press {} to bring it up.",
                            ambient::DEFAULT_HOTKEY
                        ),
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
            commands::restore_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Jeff desktop app");
}
