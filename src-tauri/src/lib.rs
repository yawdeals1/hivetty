mod app;
mod browser;
mod bundled;
mod chromium;
mod commands;
mod config;
mod find;
mod host;
mod metadata;
mod notify_os;
mod perf;
mod profiles;
mod project;
mod pty;
mod remote;
mod rpc;
mod session;
mod shell_integration;
mod tmux;
mod updater;

use app::AppState;
use tauri::Manager;

pub fn run() {
    perf::mark("process start → run()");
    let state = AppState::new();
    perf::mark("profiles detected, config loaded");
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Window size/position/maximized across launches; positions on monitors that are no
        // longer connected are ignored. The window starts hidden and is shown by the UI.
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(tauri_plugin_window_state::StateFlags::all() - tauri_plugin_window_state::StateFlags::VISIBLE)
                .build(),
        )
        .manage(updater::UpdaterState::default())
        .manage(state)
        .setup(|app| {
            perf::mark("setup");
            let handle = app.handle().clone();
            // Created hidden so the saved size/position applies before it appears; show it now,
            // on the app background color, while the UI loads.
            app::show_main_window(&handle);
            rpc::start(handle.clone());
            metadata::start(handle.clone());
            config::watch(handle.clone());
            updater::start(handle.clone());
            // Restoring spawns shells (and maybe browser webviews), which takes a while. Do it off
            // the main thread: the UI's page is served from the main thread, so blocking it here
            // delays the first frame. get_state waits for this to finish.
            let h = handle.clone();
            std::thread::Builder::new()
                .name("restore".into())
                .spawn(move || {
                    // install the tmux shim before any terminal starts
                    tmux::shim_dir();
                    app::restore_session(&h);
                    perf::mark("session restored");
                    if h.state::<AppState>().model.lock().workspaces.is_empty() {
                        if let Err(err) = app::act_new_workspace(&h, None, None, None, None) {
                            eprintln!("failed to start initial workspace: {err:#}");
                        }
                    }
                    // only after restore, so an early autosave can't overwrite the saved session
                    session::autosave(h.clone());
                    app::mark_restored();
                    perf::mark("first terminal started");
                })
                .expect("spawn restore thread");
            perf::mark("setup done");
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::ScaleFactorChanged { .. } = event {
                let app = window.app_handle().clone();
                // after the window has settled at its new scale
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    app.state::<AppState>().browsers.reapply_bounds(&app);
                });
            }
            // Closing: give the UI a moment to hand over the latest scrollback, then close for real.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && session::begin_close(window.app_handle()) {
                    api.prevent_close();
                }
            }
            if let tauri::WindowEvent::Destroyed = event {
                if window.label() == "main" {
                    let app = window.app_handle();
                    if app.state::<AppState>().config.lock().config.session.restore {
                        if let Err(e) = session::save(app) {
                            eprintln!("hivetty: saving session failed: {e:#}");
                        }
                    }
                    let st = app.state::<AppState>();
                    // persistent sessions: hosted shells keep running for the next launch
                    let keep = st.config.lock().config.session.persistent;
                    st.ptys.shutdown(!keep);
                    st.browsers.close_all(app);
                    st.tunnels.stop_all();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_info,
            commands::new_workspace,
            commands::new_surface,
            commands::split,
            commands::close_pane,
            commands::close_surface,
            commands::close_workspace,
            commands::focus_pane,
            commands::focus_dir,
            commands::resize_pane,
            commands::set_ratio,
            commands::equalize,
            commands::toggle_zoom,
            commands::activate_workspace,
            commands::activate_workspace_index,
            commands::activate_surface,
            commands::cycle_workspace,
            commands::cycle_surface,
            commands::move_workspace,
            commands::update_workspace,
            commands::rename_surface,
            commands::mark_all_read,
            commands::clear_notifications,
            commands::jump_latest_unread,
            commands::set_app_focused,
            commands::pty_attach,
            commands::pty_write,
            commands::pty_resize,
            commands::restart_pane,
            commands::browser_set_bounds,
            commands::browser_navigate,
            commands::browser_history,
            commands::browser_devtools,
            commands::browser_open,
            commands::browser_focus,
            commands::get_config,
            commands::config_set,
            commands::config_open,
            commands::config_import_wt,
            commands::session_put_scrollback,
            commands::session_take_restored,
            commands::session_save,
            commands::session_flushed,
            commands::pane_resume_agent,
            commands::pane_dismiss_resume,
            commands::find_start,
            commands::find_cancel,
            commands::open_file_at,
            commands::project_commands,
            commands::run_command,
            commands::browser_profiles,
            commands::browser_import,
            commands::browser_suggest,
            commands::new_ssh_workspace,
            commands::pane_drop_files,
            commands::host_status,
            commands::host_stop,
            commands::update_status,
            commands::ui_ready,
            commands::debug_log,
            commands::update_check,
            commands::update_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Hivetty");
}
