mod ambient;
mod artifact_parser;
mod chat;
mod chat_streaming;
mod chunking;
mod classifier;
mod commands;
mod coworking;
mod embedding;
mod flow;
mod message_kind;
mod models;
mod proactive;
mod providers;
mod reasoning;
mod retrieval;
mod revision;
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
            let embeddings = Arc::new(default_embeddings_provider());
            let reasoning = Arc::new(OpenAiReasoningProvider::from_env());
            let voice = Arc::new(OpenAiVoiceProvider::from_env());

            app.manage(JeffState::new(store, embeddings, reasoning, voice));
            app.manage(AmbientState::new());

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
            ambient::install_tray(&handle)
                .map_err(|error| format!("failed to install tray icon: {error}"))?;

            // register the global hotkey. registration may fail if the combo
            // is already owned by another app; we surface the conflict via an
            // event and continue — the tray remains a working entry point.
            let _ = ambient::register_global_hotkey(&handle);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_task,
            commands::list_tasks,
            commands::get_active_task,
            commands::set_active_task,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Jeff desktop app");
}
