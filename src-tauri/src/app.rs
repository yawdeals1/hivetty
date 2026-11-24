//! App state: the authoritative Model plus side-effect managers, and the glue that keeps them in sync.

use crate::profiles::{self, Profile};
use hivetty_core::paths::{local_path, shell_host, ShellHost};
use crate::pty::{PtyEvents, PtyManager, SpawnSpec};
use hivetty_core::layout::{Dir, PaneId};
use hivetty_core::model::{Closed, Model, PaneKind, WorkspaceId};
use hivetty_core::osc::TermEvent;
use hivetty_core::protocol;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

pub struct AppState {
    pub model: Mutex<Model>,
    pub ptys: PtyManager,
    pub browsers: crate::browser::BrowserManager,
    /// Shells found on this machine.
    pub detected_profiles: Vec<Profile>,
    /// Detected + custom profiles after applying the config.
    pub profiles: Mutex<Vec<Profile>>,
    pub default_profile: Mutex<String>,
    pub config: Mutex<hivetty_core::config::Loaded>,
    pub session: crate::session::SessionStore,
    pub find: Arc<crate::find::FindManager>,
    /// Address-bar suggestions imported from a Chromium profile (url, title), best first.
    pub history: Mutex<Vec<(String, String)>>,
    pub pipe_name: String,
    /// SOCKS tunnels for remote workspaces' browser panes.
    pub tunnels: crate::remote::Tunnels,
    /// tmux-style numbers (%3, @2, $1) for panes, tabs and workspaces.
    pub nums: Mutex<crate::tmux::Nums>,
    pub tmux_waits: crate::tmux::WaitChannels,
    /// Login shell per WSL distro (decides whether bash integration can be injected).
    pub wsl_shells: Mutex<std::collections::HashMap<String, String>>,
    pub focused: AtomicBool,
    pub last_unread: AtomicUsize,
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl AppState {
    pub fn new() -> Self {
        // Both touch the disk (PATH lookups, settings/schema files); do them side by side.
        let (detected, config) = std::thread::scope(|s| {
            let d = s.spawn(profiles::detect);
            let c = crate::config::initial();
            (d.join().unwrap_or_default(), c)
        });
        let (profiles, default_profile) = crate::config::build_profiles(&detected, &config.config);
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
        Self {
            model: Mutex::new(Model::new()),
            ptys: PtyManager::default(),
            browsers: Default::default(),
            detected_profiles: detected,
            profiles: Mutex::new(profiles),
            default_profile: Mutex::new(default_profile),
            config: Mutex::new(config),
            session: Default::default(),
            find: Default::default(),
            history: Mutex::new(Vec::new()),
            pipe_name: protocol::pipe_name(&user),
            tunnels: Default::default(),
            nums: Default::default(),
            tmux_waits: Default::default(),
            wsl_shells: Default::default(),
            focused: AtomicBool::new(true),
            last_unread: AtomicUsize::new(0),
        }
    }

    pub fn terminal_kind(&self, profile: Option<String>, cwd: Option<String>, command: Option<String>) -> PaneKind {
        PaneKind::Terminal { profile: profile.unwrap_or_else(|| self.default_profile.lock().clone()), cwd, command }
    }
}

/// Bridges PTY thread callbacks back into the app.
pub struct Events(pub AppHandle);

impl PtyEvents for Events {
    fn on_events(&self, pane: PaneId, events: Vec<TermEvent>) {
        if std::env::var_os("HIVETTY_DEBUG_EVENTS").is_some() {
            eprintln!("[events] {pane}: {events:?}");
        }
        let st = self.0.state::<AppState>();
        let focused = st.focused.load(Ordering::Relaxed);
        let mut changed = false;
        let mut toasts = Vec::new();
        {
            let mut m = st.model.lock();
            for ev in events {
                match ev {
                    TermEvent::Title { title } => {
                        if let Some(p) = m.panes.get_mut(&pane) {
                            // Shells on Windows often set the console title to their own exe path; that's noise.
                            let t = title.trim();
                            let exe_path = t.to_ascii_lowercase().ends_with(".exe") && (t.contains('\\') || t.contains('/'));
                            let new = Some(t.to_string()).filter(|t| !t.is_empty() && !exe_path);
                            if p.title != new {
                                p.title = new;
                                changed = true;
                            }
                        }
                    }
                    TermEvent::Cwd { path } => {
                        if let Some(p) = m.panes.get_mut(&pane) {
                            changed |= p.set_cwd(Some(path));
                        }
                    }
                    TermEvent::CommandExecuted => {
                        if let Some(p) = m.panes.get_mut(&pane) {
                            p.running = true;
                            changed = true;
                        }
                    }
                    TermEvent::CommandFinished { exit_code } => {
                        if let Some(p) = m.panes.get_mut(&pane) {
                            p.running = false;
                            p.last_exit = exit_code;
                            changed = true;
                        }
                    }
                    TermEvent::Notify { title, body } => toasts.push((title, body)),
                    TermEvent::Bell => {
                        if st.config.lock().config.terminal.bell == hivetty_core::config::BellMode::None {
                            continue;
                        }
                        let visible = focused && m.focused_pane() == Some(pane);
                        if !visible {
                            if let Some(p) = m.panes.get_mut(&pane) {
                                if !p.unread {
                                    p.unread = true;
                                    changed = true;
                                }
                            }
                        }
                    }
                    TermEvent::PromptStart | TermEvent::CommandStart | TermEvent::Progress { .. } | TermEvent::CursorPositionRequest => {}
                }
            }
        }
        for (title, body) in toasts {
            deliver_notification(&self.0, pane, title, body);
        }
        if changed {
            emit_state(&self.0);
        }
    }

    fn on_exit(&self, pane: PaneId, code: Option<i32>) {
        let st = self.0.state::<AppState>();
        if let Some(p) = st.model.lock().panes.get_mut(&pane) {
            p.exited = true;
            p.running = false;
            p.last_exit = code;
        }
        let _ = self.0.emit("pane-exited", serde_json::json!({ "pane": pane, "code": code }));
        emit_state(&self.0);
    }
}

pub fn emit_state(app: &AppHandle) {
    let st = app.state::<AppState>();
    let (snapshot, unread) = {
        let m = st.model.lock();
        (serde_json::to_value(&*m).unwrap_or_default(), m.unread_count())
    };
    let _ = app.emit("state", snapshot);
    st.session.dirty.store(true, Ordering::Relaxed);
    let prev = st.last_unread.swap(unread, Ordering::Relaxed);
    if (prev == 0) != (unread == 0) {
        crate::notify_os::set_badge(app, unread > 0);
    }
}

/// Record a notification for `pane` and surface it: in-app event, plus a Windows toast and
/// taskbar flash when the user can't currently see that pane.
pub fn deliver_notification(app: &AppHandle, pane: PaneId, title: Option<String>, body: String) -> Option<hivetty_core::model::Notification> {
    let st = app.state::<AppState>();
    let focused = st.focused.load(Ordering::Relaxed);
    let (n, ws_name) = {
        let mut m = st.model.lock();
        let n = m.notify(pane, title, body, now_ms(), focused)?;
        let ws_name = m.ws_index(n.workspace).map(|i| m.workspaces[i].name.clone()).unwrap_or_default();
        (n, ws_name)
    };
    let _ = app.emit("notification", &n);
    if !n.read {
        crate::notify_os::toast(app, &n, &ws_name, focused);
    }
    emit_state(app);
    Some(n)
}

pub fn focus_window(app: &AppHandle) {
    if let Some(w) = app.get_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Start the backing process for a freshly created pane.
pub fn start_pane(app: &AppHandle, pane: PaneId) -> anyhow::Result<()> {
    let st = app.state::<AppState>();
    let (kind, ws, remote) = {
        let m = st.model.lock();
        let p = m.panes.get(&pane).ok_or_else(|| anyhow::anyhow!("no pane"))?;
        let w = m.workspace_of(pane);
        (p.kind.clone(), w.map(|w| w.id), w.and_then(|w| w.remote.clone()))
    };
    let (profile, cwd, command) = match kind {
        PaneKind::Terminal { profile, cwd, command } => (profile, cwd, command),
        PaneKind::Browser { url } => return crate::remote::start_browser(app, pane, &url, remote.as_ref()),
    };
    let mut env = vec![
        (protocol::ENV_PIPE.to_string(), st.pipe_name.clone()),
        (protocol::ENV_PANE.to_string(), pane.to_string()),
    ];
    if let Some(ws) = ws {
        env.push((protocol::ENV_WORKSPACE.to_string(), ws.to_string()));
    }
    // `typed`: a command to type into the shell once it starts (ssh passes it to the remote instead)
    let (exe, args, spawn_cwd, typed) = match shell_host(&profile) {
        ShellHost::Ssh => {
            let r = remote.clone().ok_or_else(|| anyhow::anyhow!("this workspace has no ssh connection"))?;
            (crate::remote::ssh_exe()?, r.terminal_args(cwd.as_deref(), command.as_deref()), None, None)
        }
        ShellHost::Wsl(distro) => {
            let prof = find_profile(&st, &profile).unwrap_or_else(|| profiles::wsl_profile(&distro));
            let args = crate::shell_integration::wsl_args(app, &prof, &distro, cwd.as_deref());
            env.push(("WSLENV".into(), crate::shell_integration::wslenv()));
            env.extend(prof.env.iter().cloned());
            (prof.command, args, None, command)
        }
        ShellHost::Local => {
            let prof = find_profile(&st, &profile)
                .or_else(|| st.profiles.lock().first().cloned())
                .ok_or_else(|| anyhow::anyhow!("no shell profiles"))?;
            let args = crate::shell_integration::final_args(app, &prof, command.as_deref());
            env.extend(crate::shell_integration::env(app, &prof));
            env.push(("PATH".into(), crate::bundled::pane_path()));
            // before the profile's own env, so a custom PATH there still wins
            env.extend(crate::tmux::pane_env(app, pane));
            env.extend(prof.env.iter().cloned());
            let cwd = cwd.clone().filter(|c| local_path(&profile, c).is_some()).or_else(|| prof.cwd.clone());
            (prof.command, args, cwd, command)
        }
    };
    let spec = SpawnSpec { command: exe, args, cwd: spawn_cwd, env, cols: 120, rows: 30 };
    let events: Arc<dyn PtyEvents> = Arc::new(Events(app.clone()));
    let persistent = st.config.lock().config.session.persistent;
    if persistent {
        let meta = serde_json::json!({ "profile": profile, "cwd": cwd, "remote": remote });
        match st.ptys.spawn_hosted(pane, spec.clone(), meta, events.clone()) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("hivetty: session host unavailable ({e:#}); starting the shell locally");
                st.ptys.spawn(pane, spec, events)?;
            }
        }
    } else {
        st.ptys.spawn(pane, spec, events)?;
    }
    if let Some(cmd) = typed.filter(|c| !c.trim().is_empty()) {
        st.ptys.write(pane, format!("{cmd}\r").as_bytes());
    }
    Ok(())
}

fn find_profile(st: &AppState, id: &str) -> Option<Profile> {
    st.profiles.lock().iter().find(|p| p.id == id).cloned()
}

pub fn teardown(app: &AppHandle, closed: &Closed) {
    let st = app.state::<AppState>();
    for p in &closed.panes {
        st.ptys.kill(*p);
        st.session.forget(*p);
        st.browsers.close(app, *p);
    }
    if closed.workspace.is_some() {
        crate::remote::prune_tunnels(app);
    }
}

/// The local (Windows) directory of a reference pane, for things that run on this machine.
pub fn cwd_of(app: &AppHandle, pane: Option<PaneId>) -> Option<String> {
    let st = app.state::<AppState>();
    let m = st.model.lock();
    pane.and_then(|p| m.panes.get(&p)).and_then(|p| p.local_cwd.clone())
}

/// The terminal to open next to `source` (splits, new tabs). WSL and ssh panes keep their shell
/// and directory; other panes get `profile` (default: the default profile) in the source's
/// directory, translated for where the new shell runs.
pub fn kind_near(app: &AppHandle, source: Option<PaneId>, profile: Option<String>, cwd: Option<String>, command: Option<String>) -> PaneKind {
    let st = app.state::<AppState>();
    let (src_profile, src_cwd, src_local, remote_ws) = {
        let m = st.model.lock();
        let p = source.and_then(|s| m.panes.get(&s));
        let remote_ws = source.and_then(|s| m.workspace_of(s)).is_some_and(|w| w.remote.is_some());
        (p.and_then(|p| p.profile().map(str::to_string)), p.and_then(|p| p.cwd.clone()), p.and_then(|p| p.local_cwd.clone()), remote_ws)
    };
    let profile = profile.or_else(|| match src_profile.as_deref() {
        Some(sp) if shell_host(sp) != ShellHost::Local => Some(sp.to_string()),
        // a browser pane in an ssh workspace: new terminals go to the remote too
        None if remote_ws => Some(hivetty_core::paths::SSH_PROFILE.to_string()),
        _ => None,
    });
    let profile = profile.unwrap_or_else(|| st.default_profile.lock().clone());
    let same_host = src_profile.as_deref().is_some_and(|sp| shell_host(sp) == shell_host(&profile));
    let cwd = cwd.or(if same_host { src_cwd } else { src_local });
    PaneKind::Terminal { profile, cwd, command }
}

/// Type the resume command for a pane's recorded agent session (restored panes).
pub fn resume_agent(app: &AppHandle, pane: PaneId) -> anyhow::Result<()> {
    let st = app.state::<AppState>();
    let agent = {
        let mut m = st.model.lock();
        let p = m.panes.get_mut(&pane).ok_or_else(|| anyhow::anyhow!("no pane"))?;
        p.resumable = false;
        p.agent.clone()
    };
    emit_state(app);
    let a = agent.ok_or_else(|| anyhow::anyhow!("no recorded agent session"))?;
    let cmd = crate::session::resume_command(&a.agent, &a.session_id).ok_or_else(|| anyhow::anyhow!("can't resume {} sessions", a.agent))?;
    if !st.ptys.write(pane, format!("{cmd}\r").as_bytes()) {
        anyhow::bail!("pane has no running terminal");
    }
    Ok(())
}

/// Reconnect a pane to its shell in the session host. Returns false if the host doesn't have it.
fn reattach_pane(app: &AppHandle, host: &Arc<crate::host::HostClient>, pane: PaneId) -> bool {
    let st = app.state::<AppState>();
    match st.ptys.reattach(host, pane, 30, 120, Arc::new(Events(app.clone()))) {
        Ok(exit) => {
            // The live replay is more current than the saved scrollback.
            st.session.drop_restored(pane);
            let mut m = st.model.lock();
            if let Some(p) = m.panes.get_mut(&pane) {
                // still running: whatever it was doing continues, nothing to resume
                p.resumable = false;
                if let Some(code) = exit {
                    p.exited = true;
                    p.last_exit = code;
                }
            }
            true
        }
        Err(e) => {
            eprintln!("hivetty: couldn't reattach {pane}: {e:#}");
            false
        }
    }
}

/// Reopen the last session (if enabled) and reattach shells still running in the session host;
/// hosted shells the session doesn't know about land in a "Recovered" workspace.
/// Returns true when something was restored.
pub fn restore_session(app: &AppHandle) -> bool {
    let st = app.state::<AppState>();
    let sc = st.config.lock().config.session.clone();
    let host = st.ptys.existing_host();
    let mut live: std::collections::HashMap<PaneId, serde_json::Value> = host
        .as_ref()
        .and_then(|h| h.list().ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| Some((uuid::Uuid::parse_str(s["pane"].as_str()?).ok()?, s)))
        .collect();
    let mut restored = false;
    if let Some((model, scrollback)) = sc.restore.then(crate::session::load).flatten().filter(|(m, _)| !m.workspaces.is_empty()) {
        let panes: Vec<PaneId> = model.workspaces.iter().flat_map(|w| w.surfaces.iter()).flat_map(|s| s.root.panes()).collect();
        *st.model.lock() = model;
        if sc.scrollback {
            crate::session::set_restored(app, scrollback);
        }
        for p in panes {
            if live.remove(&p).is_some() && reattach_pane(app, host.as_ref().unwrap(), p) {
                continue;
            }
            if let Err(e) = start_pane(app, p) {
                eprintln!("hivetty: couldn't restore pane {p}: {e:#}");
            }
            let resumable = st.model.lock().panes.get(&p).is_some_and(|p| p.resumable);
            if resumable && sc.resume_agents {
                let _ = resume_agent(app, p);
            }
        }
        restored = true;
    }
    if let (Some(host), false) = (host.as_ref(), live.is_empty()) {
        // group orphans by connection so ssh shells get a workspace with that remote
        let mut groups: std::collections::BTreeMap<String, (Option<hivetty_core::remote::Remote>, Vec<(PaneId, PaneKind)>)> = Default::default();
        let mut orphans: Vec<_> = live.into_iter().collect();
        orphans.sort_by_key(|(_, s)| s["created"].as_u64().unwrap_or(0));
        for (id, s) in orphans {
            let meta = &s["meta"];
            let remote: Option<hivetty_core::remote::Remote> = serde_json::from_value(meta["remote"].clone()).ok();
            let profile = meta["profile"].as_str().map(str::to_string).unwrap_or_else(|| st.default_profile.lock().clone());
            let kind = PaneKind::Terminal { profile, cwd: meta["cwd"].as_str().map(str::to_string), command: None };
            let key = remote.as_ref().map(|r| r.key()).unwrap_or_default();
            groups.entry(key).or_insert_with(|| (remote.clone(), Vec::new())).1.push((id, kind));
        }
        for (_, (remote, panes)) in groups {
            let ids: Vec<PaneId> = panes.iter().map(|(id, _)| *id).collect();
            let name = match &remote {
                Some(r) => format!("Recovered ({})", r.hostname()),
                None => "Recovered".to_string(),
            };
            let ws = {
                let mut m = st.model.lock();
                let ws = m.adopt_workspace(name, panes);
                if let Some(i) = ws.and_then(|w| m.ws_index(w)) {
                    m.workspaces[i].remote = remote;
                }
                ws
            };
            if ws.is_none() {
                continue;
            }
            for id in ids {
                if !reattach_pane(app, host, id) {
                    let closed = st.model.lock().close_pane(id);
                    teardown(app, &closed);
                }
            }
            restored = true;
        }
    }
    if restored {
        emit_state(app);
    }
    restored
}

// ---- high-level actions shared by UI commands and the pipe API ----

pub fn act_new_workspace(app: &AppHandle, name: Option<String>, profile: Option<String>, cwd: Option<String>, command: Option<String>) -> anyhow::Result<(WorkspaceId, PaneId)> {
    let st = app.state::<AppState>();
    let kind = st.terminal_kind(profile, cwd, command);
    let (ws, pane) = st.model.lock().new_workspace(name, kind);
    start_pane(app, pane)?;
    emit_state(app);
    Ok((ws, pane))
}

/// Open a workspace whose terminals run on `remote` over ssh.
pub fn act_ssh_workspace(app: &AppHandle, remote: hivetty_core::remote::Remote, name: Option<String>, cwd: Option<String>, command: Option<String>) -> anyhow::Result<(WorkspaceId, PaneId)> {
    crate::remote::ssh_exe()?;
    let st = app.state::<AppState>();
    let name = name.unwrap_or_else(|| remote.hostname().to_string());
    let kind = PaneKind::Terminal { profile: hivetty_core::paths::SSH_PROFILE.into(), cwd, command };
    let (ws, pane) = {
        let mut m = st.model.lock();
        let (ws, pane) = m.new_workspace(Some(name), kind);
        let i = m.ws_index(ws).expect("just created");
        m.workspaces[i].remote = Some(remote);
        (ws, pane)
    };
    start_pane(app, pane)?;
    emit_state(app);
    Ok((ws, pane))
}

pub fn act_split(app: &AppHandle, target: Option<PaneId>, dir: Dir, kind: Option<PaneKind>) -> anyhow::Result<PaneId> {
    let st = app.state::<AppState>();
    let target = target.or_else(|| st.model.lock().focused_pane()).ok_or_else(|| anyhow::anyhow!("no pane to split"))?;
    let kind = kind.unwrap_or_else(|| kind_near(app, Some(target), None, None, None));
    let new = st.model.lock().split(target, dir, kind).ok_or_else(|| anyhow::anyhow!("pane not found"))?;
    start_pane(app, new)?;
    emit_state(app);
    Ok(new)
}

pub fn act_close_pane(app: &AppHandle, pane: PaneId) {
    let st = app.state::<AppState>();
    let closed = st.model.lock().close_pane(pane);
    teardown(app, &closed);
    emit_state(app);
}

/// Find the browser pane an automation call should target: explicit, focused, or the most
/// relevant one in the active workspace.
pub fn find_browser_pane(app: &AppHandle, explicit: Option<PaneId>) -> Option<PaneId> {
    let st = app.state::<AppState>();
    let m = st.model.lock();
    let is_browser = |p: &PaneId| matches!(m.panes.get(p).map(|p| &p.kind), Some(PaneKind::Browser { .. }));
    if let Some(p) = explicit {
        return is_browser(&p).then_some(p);
    }
    if let Some(f) = m.focused_pane().filter(is_browser) {
        return Some(f);
    }
    let ws = m.active_workspace.and_then(|w| m.ws_index(w))?;
    let w = &m.workspaces[ws];
    let mut surfaces: Vec<_> = w.surfaces.iter().filter(|s| s.id == w.active_surface).collect();
    surfaces.extend(w.surfaces.iter().filter(|s| s.id != w.active_surface));
    surfaces.into_iter().flat_map(|s| s.root.panes()).find(is_browser)
}

/// Open a URL in a browser pane: navigate the one in the current tab, or split one beside `near`.
pub fn open_in_browser(app: &AppHandle, url: &str, near: Option<PaneId>) -> anyhow::Result<PaneId> {
    let st = app.state::<AppState>();
    let target = {
        let m = st.model.lock();
        let cur = near.or(m.focused_pane()).and_then(|p| m.surface_of(p)).map(|s| s.root.panes()).unwrap_or_default();
        cur.into_iter().find(|p| matches!(m.panes.get(p).map(|p| &p.kind), Some(PaneKind::Browser { .. })))
    };
    if let Some(p) = target {
        st.browsers.navigate(app, p, url).map_err(|e| anyhow::anyhow!(e))?;
        st.model.lock().focus_pane(p);
        emit_state(app);
        return Ok(p);
    }
    let url = crate::browser::normalize_url(url);
    act_split(app, near, Dir::Right, Some(PaneKind::Browser { url }))
}

static WINDOW_SHOWN: AtomicBool = AtomicBool::new(false);

/// Show the main window (once). It's created hidden so the saved size and position (window-state
/// plugin) apply before it appears.
pub fn show_main_window(app: &AppHandle) {
    if WINDOW_SHOWN.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(w) = app.get_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    crate::perf::mark("window shown");
}

static RESTORED: parking_lot::Mutex<bool> = parking_lot::Mutex::new(false);
static RESTORED_CV: parking_lot::Condvar = parking_lot::Condvar::new();

/// Startup restore (and the first workspace) is done.
pub fn mark_restored() {
    *RESTORED.lock() = true;
    RESTORED_CV.notify_all();
}

/// Block until startup restore is done (bounded, so a stuck restore can't hang the UI).
pub fn wait_restored() {
    let mut done = RESTORED.lock();
    if !*done {
        let _ = RESTORED_CV.wait_while_for(&mut done, |d| !*d, std::time::Duration::from_secs(10));
    }
}
