//! Tauri commands invoked by the UI. Thin wrappers over Model ops + side effects.

use crate::app::{self, emit_state, AppState};
use crate::profiles::Profile;
use hivetty_core::layout::{Dir, PaneId};
use hivetty_core::model::{SurfaceId, WorkspaceId};
use std::sync::atomic::Ordering;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, State};

type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

#[tauri::command(async)]
pub fn get_state(st: State<'_, AppState>) -> serde_json::Value {
    app::wait_restored();
    serde_json::to_value(&*st.model.lock()).unwrap_or_default()
}

#[derive(serde::Serialize, Clone)]
pub struct Info {
    profiles: Vec<Profile>,
    default_profile: String,
    pipe: String,
    version: &'static str,
    /// HIVETTY_DEBUG_PERF: the UI reports terminal throughput through `debug_log`.
    debug_perf: bool,
}

pub fn info(st: &AppState) -> Info {
    Info { profiles: st.profiles.lock().clone(), default_profile: st.default_profile.lock().clone(), pipe: st.pipe_name.clone(), version: env!("CARGO_PKG_VERSION"), debug_perf: std::env::var_os("HIVETTY_DEBUG_PERF").is_some() }
}

#[tauri::command(async)]
pub fn get_info(st: State<'_, AppState>) -> Info {
    info(&st)
}

#[tauri::command(async)]
pub fn new_workspace(app: AppHandle, name: Option<String>, profile: Option<String>, cwd: Option<String>) -> R<(WorkspaceId, PaneId)> {
    app::act_new_workspace(&app, name, profile, cwd, None).map_err(e)
}

#[tauri::command(async)]
pub fn new_surface(app: AppHandle, st: State<'_, AppState>, workspace: Option<WorkspaceId>, profile: Option<String>) -> R<PaneId> {
    let ws = workspace.or(st.model.lock().active_workspace).ok_or("no workspace")?;
    let near = {
        let m = st.model.lock();
        m.ws_index(ws).and_then(|i| {
            let w = &m.workspaces[i];
            w.surfaces.iter().find(|s| s.id == w.active_surface).map(|s| s.focused)
        })
    };
    let kind = app::kind_near(&app, near, profile, None, None);
    let (_, pane) = st.model.lock().new_surface(ws, kind).ok_or("workspace not found")?;
    app::start_pane(&app, pane).map_err(e)?;
    emit_state(&app);
    Ok(pane)
}

#[tauri::command(async)]
pub fn split(app: AppHandle, st: State<'_, AppState>, dir: Dir, pane: Option<PaneId>, profile: Option<String>) -> R<PaneId> {
    let kind = profile.map(|p| {
        let target = pane.or(st.model.lock().focused_pane());
        app::kind_near(&app, target, Some(p), None, None)
    });
    app::act_split(&app, pane, dir, kind).map_err(e)
}

#[tauri::command(async)]
pub fn close_pane(app: AppHandle, pane: PaneId) {
    app::act_close_pane(&app, pane);
}

#[tauri::command(async)]
pub fn close_surface(app: AppHandle, st: State<'_, AppState>, workspace: WorkspaceId, surface: SurfaceId) {
    let closed = st.model.lock().close_surface(workspace, surface);
    app::teardown(&app, &closed);
    emit_state(&app);
}

#[tauri::command(async)]
pub fn close_workspace(app: AppHandle, st: State<'_, AppState>, workspace: WorkspaceId) {
    let closed = st.model.lock().close_workspace(workspace);
    app::teardown(&app, &closed);
    emit_state(&app);
}

#[tauri::command(async)]
pub fn focus_pane(app: AppHandle, st: State<'_, AppState>, pane: PaneId) {
    if st.model.lock().focus_pane(pane) {
        emit_state(&app);
    }
}

#[tauri::command(async)]
pub fn focus_dir(app: AppHandle, st: State<'_, AppState>, dir: Dir) {
    if st.model.lock().focus_dir(dir).is_some() {
        emit_state(&app);
    }
}

#[tauri::command(async)]
pub fn resize_pane(app: AppHandle, st: State<'_, AppState>, dir: Dir, delta: f32) {
    if st.model.lock().resize_focused(dir, delta) {
        emit_state(&app);
    }
}

/// Set an explicit split ratio (drag handle). `path` is a list of 0/1 child choices from the root.
#[tauri::command(async)]
pub fn set_ratio(app: AppHandle, st: State<'_, AppState>, surface: SurfaceId, path: Vec<u8>, ratio: f32) {
    use hivetty_core::layout::Node;
    let mut m = st.model.lock();
    let Some(s) = m.workspaces.iter_mut().flat_map(|w| w.surfaces.iter_mut()).find(|s| s.id == surface) else { return };
    let mut node = &mut s.root;
    for step in path {
        match node {
            Node::Split { a, b, .. } => node = if step == 0 { a } else { b },
            Node::Leaf { .. } => return,
        }
    }
    if let Node::Split { ratio: r, .. } = node {
        *r = ratio.clamp(0.05, 0.95);
    }
    drop(m);
    emit_state(&app);
}

#[tauri::command(async)]
pub fn equalize(app: AppHandle, st: State<'_, AppState>) {
    {
        let mut m = st.model.lock();
        let Some(p) = m.focused_pane() else { return };
        let Some(loc) = m.locate(p) else { return };
        m.workspaces[loc.ws].surfaces[loc.surface].root.equalize();
    }
    emit_state(&app);
}

#[tauri::command(async)]
pub fn toggle_zoom(app: AppHandle, st: State<'_, AppState>) {
    st.model.lock().toggle_zoom();
    emit_state(&app);
}

#[tauri::command(async)]
pub fn activate_workspace(app: AppHandle, st: State<'_, AppState>, workspace: WorkspaceId) {
    if st.model.lock().activate_workspace(workspace) {
        emit_state(&app);
    }
}

#[tauri::command(async)]
pub fn activate_workspace_index(app: AppHandle, st: State<'_, AppState>, index: isize) {
    let id = {
        let m = st.model.lock();
        let n = m.workspaces.len();
        if n == 0 {
            return;
        }
        let i = if index < 0 { n - 1 } else { (index as usize).min(n - 1) };
        m.workspaces[i].id
    };
    st.model.lock().activate_workspace(id);
    emit_state(&app);
}

#[tauri::command(async)]
pub fn activate_surface(app: AppHandle, st: State<'_, AppState>, workspace: WorkspaceId, surface: SurfaceId) {
    if st.model.lock().activate_surface(workspace, surface) {
        emit_state(&app);
    }
}

#[tauri::command(async)]
pub fn cycle_workspace(app: AppHandle, st: State<'_, AppState>, step: isize) {
    st.model.lock().cycle_workspace(step);
    emit_state(&app);
}

#[tauri::command(async)]
pub fn cycle_surface(app: AppHandle, st: State<'_, AppState>, step: isize) {
    st.model.lock().cycle_surface(step);
    emit_state(&app);
}

#[tauri::command(async)]
pub fn move_workspace(app: AppHandle, st: State<'_, AppState>, workspace: WorkspaceId, to: usize) {
    st.model.lock().move_workspace(workspace, to);
    emit_state(&app);
}

#[tauri::command(async)]
pub fn update_workspace(app: AppHandle, st: State<'_, AppState>, workspace: WorkspaceId, name: Option<String>, description: Option<String>, color: Option<String>) {
    {
        let mut m = st.model.lock();
        let Some(i) = m.ws_index(workspace) else { return };
        let w = &mut m.workspaces[i];
        if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
            w.name = n;
        }
        if let Some(d) = description {
            w.description = Some(d).filter(|d| !d.is_empty());
        }
        if let Some(c) = color {
            w.color = Some(c).filter(|c| !c.is_empty());
        }
    }
    emit_state(&app);
}

#[tauri::command(async)]
pub fn rename_surface(app: AppHandle, st: State<'_, AppState>, surface: SurfaceId, title: String) {
    {
        let mut m = st.model.lock();
        if let Some(s) = m.workspaces.iter_mut().flat_map(|w| w.surfaces.iter_mut()).find(|s| s.id == surface) {
            s.title = Some(title).filter(|t| !t.trim().is_empty());
        }
    }
    emit_state(&app);
}

// ---- notifications ----

#[tauri::command(async)]
pub fn mark_all_read(app: AppHandle, st: State<'_, AppState>) {
    st.model.lock().mark_all_read();
    emit_state(&app);
}

#[tauri::command(async)]
pub fn clear_notifications(app: AppHandle, st: State<'_, AppState>) {
    st.model.lock().clear_notifications();
    emit_state(&app);
}

#[tauri::command(async)]
pub fn jump_latest_unread(app: AppHandle, st: State<'_, AppState>) -> Option<PaneId> {
    let pane = st.model.lock().latest_unread()?;
    st.model.lock().focus_pane(pane);
    emit_state(&app);
    Some(pane)
}

#[tauri::command(async)]
pub fn set_app_focused(app: AppHandle, st: State<'_, AppState>, focused: bool) {
    st.focused.store(focused, Ordering::Relaxed);
    if focused {
        let mut m = st.model.lock();
        if let Some(p) = m.focused_pane() {
            m.mark_pane_read(p);
        }
        drop(m);
        emit_state(&app);
    }
}

// ---- pty ----

#[tauri::command(async)]
pub fn pty_attach(st: State<'_, AppState>, pane: PaneId, channel: Channel<InvokeResponseBody>) -> bool {
    st.ptys.attach(pane, channel)
}

#[tauri::command(async)]
pub fn pty_write(st: State<'_, AppState>, pane: PaneId, data: String) {
    st.ptys.write(pane, data.as_bytes());
}

#[tauri::command(async)]
pub fn pty_resize(st: State<'_, AppState>, pane: PaneId, cols: u16, rows: u16) {
    st.ptys.resize(pane, cols, rows);
}

#[tauri::command(async)]
pub fn restart_pane(app: AppHandle, st: State<'_, AppState>, pane: PaneId) -> R<()> {
    if st.browsers.has(pane) {
        return st.browsers.eval(&app, pane, "location.reload()").map(|_| ());
    }
    st.ptys.kill(pane);
    if let Some(p) = st.model.lock().panes.get_mut(&pane) {
        p.exited = false;
        p.last_exit = None;
    }
    app::start_pane(&app, pane).map_err(e)?;
    emit_state(&app);
    Ok(())
}

// ---- browser panes ----

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn browser_set_bounds(app: AppHandle, st: State<'_, AppState>, pane: PaneId, x: f64, y: f64, w: f64, h: f64, visible: bool) {
    st.browsers.set_bounds(&app, pane, x, y, w, h, visible);
}

#[tauri::command(async)]
pub fn browser_navigate(app: AppHandle, st: State<'_, AppState>, pane: PaneId, url: String) -> R<String> {
    st.browsers.navigate(&app, pane, &url)
}

/// action: "back" | "forward" | "reload"
#[tauri::command(async)]
pub fn browser_history(app: AppHandle, st: State<'_, AppState>, pane: PaneId, action: String) -> R<()> {
    let expr = match action.as_str() {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "reload" => "location.reload()",
        _ => return Err("unknown action".into()),
    };
    st.browsers.eval(&app, pane, expr).map(|_| ())
}

#[tauri::command(async)]
pub fn browser_devtools(app: AppHandle, st: State<'_, AppState>, pane: PaneId) -> R<()> {
    st.browsers.open_devtools(&app, pane)
}

#[tauri::command(async)]
pub fn browser_open(app: AppHandle, url: String, near: Option<PaneId>) -> R<PaneId> {
    app::open_in_browser(&app, &url, near).map_err(e)
}

#[tauri::command(async)]
pub fn browser_focus(app: AppHandle, st: State<'_, AppState>, pane: PaneId) {
    st.browsers.focus(&app, pane);
}

/// The UI's active shortcut chords, so browser webviews can forward them instead of eating them.
#[tauri::command]
pub fn set_app_shortcuts(chords: Vec<String>) {
    crate::browser::set_app_chords(chords);
}

// ---- config ----

#[tauri::command(async)]
pub fn get_config(st: State<'_, AppState>) -> crate::config::ConfigState {
    crate::config::state(&st)
}

/// Set one setting. `remove: true` deletes the key; otherwise `value` (null allowed) is stored.
#[tauri::command(async)]
pub fn config_set(app: AppHandle, path: Vec<String>, value: Option<serde_json::Value>, remove: Option<bool>) -> R<crate::config::ConfigState> {
    let v = if remove.unwrap_or(false) { None } else { Some(value.unwrap_or(serde_json::Value::Null)) };
    crate::config::set(&app, &path, v)
}

#[tauri::command(async)]
pub fn config_open(app: AppHandle) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let p = crate::config::config_path();
    // .json may have no handler; fall back to Notepad
    if app.opener().open_path(p.display().to_string(), None::<&str>).is_err() {
        std::process::Command::new("notepad.exe").arg(&p).spawn().map_err(e)?;
    }
    Ok(())
}

#[tauri::command(async)]
pub fn config_import_wt(path: Option<String>, text: Option<String>) -> R<Vec<hivetty_core::config::Theme>> {
    crate::config::wt_schemes(path, text)
}

// ---- session ----

#[tauri::command(async)]
pub fn session_put_scrollback(st: State<'_, AppState>, pane: PaneId, data: String) {
    if st.model.lock().panes.contains_key(&pane) {
        st.session.put_scrollback(pane, data);
    }
}

#[tauri::command(async)]
pub fn session_take_restored(st: State<'_, AppState>, pane: PaneId) -> Option<String> {
    st.session.take_restored(pane)
}

#[tauri::command(async)]
pub fn session_save(app: AppHandle) -> R<String> {
    crate::session::save(&app).map(|p| p.display().to_string()).map_err(e)
}

#[tauri::command(async)]
pub fn session_flushed(app: AppHandle) {
    crate::session::flushed(&app);
}

#[tauri::command(async)]
pub fn pane_resume_agent(app: AppHandle, pane: PaneId) -> R<()> {
    app::resume_agent(&app, pane).map_err(e)
}

#[tauri::command(async)]
pub fn pane_dismiss_resume(app: AppHandle, st: State<'_, AppState>, pane: PaneId) {
    if let Some(p) = st.model.lock().panes.get_mut(&pane) {
        p.resumable = false;
    }
    emit_state(&app);
}

// ---- find in files ----

#[tauri::command(async)]
pub fn find_start(st: State<'_, AppState>, opts: crate::find::FindOpts, channel: Channel<serde_json::Value>) -> R<u64> {
    st.find.start(opts, channel)
}

#[tauri::command(async)]
pub fn find_cancel(st: State<'_, AppState>) {
    st.find.cancel();
}

#[tauri::command(async)]
pub fn open_file_at(app: AppHandle, path: String, line: Option<u64>, col: Option<u64>, near: Option<PaneId>) -> R<()> {
    crate::project::open_file_at(&app, &path, line.unwrap_or(1), col.unwrap_or(1), near).map_err(e)
}

// ---- custom commands ----

#[tauri::command(async)]
pub fn project_commands(app: AppHandle, pane: Option<PaneId>) -> crate::project::Listing {
    crate::project::list(&app, pane)
}

#[tauri::command(async)]
pub fn run_command(app: AppHandle, command: crate::project::Resolved, pane: Option<PaneId>) -> R<PaneId> {
    crate::project::run(&app, &command, pane).map_err(e)
}

// ---- import from Chrome / Edge (opt-in) ----

#[tauri::command(async)]
pub fn browser_profiles() -> Vec<crate::chromium::BrowserProfile> {
    crate::chromium::list_profiles()
}

/// Import history and/or cookies from a local Chromium profile into browser pane `pane`.
#[tauri::command(async)]
pub fn browser_import(app: AppHandle, pane: PaneId, profile: String, cookies: bool, history: bool, domains: Vec<String>) -> R<serde_json::Value> {
    crate::chromium::import(&app, pane, &profile, cookies, history, &domains).map_err(e)
}

#[tauri::command(async)]
pub fn browser_suggest(app: AppHandle, query: String) -> Vec<serde_json::Value> {
    crate::chromium::suggest(&app, &query, 8)
}

// ---- remote workspaces, file drops, session host ----

/// Open a workspace on `host` over ssh (`user@host[:port]`).
#[tauri::command(async)]
pub fn new_ssh_workspace(app: AppHandle, host: String, command: Option<String>) -> R<(WorkspaceId, PaneId)> {
    let r = hivetty_core::remote::Remote::parse(&host)?;
    app::act_ssh_workspace(&app, r, None, None, command.filter(|c| !c.trim().is_empty())).map_err(e)
}

/// Files dropped on a terminal: paste their paths (ssh panes upload them first).
#[tauri::command(async)]
pub fn pane_drop_files(app: AppHandle, pane: PaneId, paths: Vec<String>) -> R<()> {
    crate::remote::drop_files(&app, pane, paths).map_err(e)
}

/// Session host status for Settings: running, its shells, and how many belong to this window.
#[tauri::command(async)]
pub fn host_status(st: State<'_, AppState>) -> serde_json::Value {
    match st.ptys.existing_host() {
        Some(h) => {
            let sessions = h.list().unwrap_or_default();
            let live = sessions.iter().filter(|s| s["exited"] != true).count();
            serde_json::json!({ "running": true, "sessions": sessions.len(), "live": live, "exe": crate::host::host_exe().map(|p| p.display().to_string()) })
        }
        None => serde_json::json!({ "running": false, "sessions": 0, "live": 0, "exe": crate::host::host_exe().map(|p| p.display().to_string()) }),
    }
}

/// Stop the session host, ending every shell it holds (those panes show as exited).
#[tauri::command(async)]
pub fn host_stop(st: State<'_, AppState>) -> R<()> {
    if let Some(h) = st.ptys.existing_host() {
        let _ = h.call("shutdown", serde_json::json!({}));
    }
    Ok(())
}

#[tauri::command(async)]
pub fn update_status(app: AppHandle) -> crate::updater::Status {
    crate::updater::status(&app)
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> R<crate::updater::Status> {
    crate::updater::check(&app).await
}

#[tauri::command]
pub async fn update_install(app: AppHandle, progress: Channel<crate::updater::Progress>) -> R<()> {
    crate::updater::install(&app, progress).await
}

/// The UI has drawn its first frame (startup timing; also shows the window if it isn't yet).
#[tauri::command]
pub fn ui_ready(app: AppHandle, script_ms: Option<f64>, frame_ms: Option<f64>, bundle_ms: Option<f64>) {
    if let (Some(s), Some(f)) = (script_ms, frame_ms) {
        let b = bundle_ms.unwrap_or(-1.0);
        crate::perf::mark(&format!("ui first frame (after navigation: bundle loaded {b:.0} ms, app code ran {s:.0} ms, first frame {f:.0} ms)"));
    } else {
        crate::perf::mark("ui first frame");
    }
    crate::app::show_main_window(&app);
}

/// Diagnostics from the UI (only sent when HIVETTY_DEBUG_PERF is set).
#[tauri::command]
pub fn debug_log(msg: String) {
    eprintln!("[ui] {msg}");
}
