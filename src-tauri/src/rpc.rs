//! Named-pipe JSON-RPC server: every UI action is scriptable by the `hivetty` CLI and agents.

use crate::app::{self, emit_state, AppState};
use hivetty_core::layout::{Dir, PaneId};
use hivetty_core::model::{AgentSession, PaneKind};
use hivetty_core::protocol::{codes, Request, Response};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use uuid::Uuid;

pub fn start(app: AppHandle) {
    let name = app.state::<AppState>().pipe_name.clone();
    tauri::async_runtime::spawn(async move {
        let mut server = match ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(&name) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("hivetty: pipe {name} unavailable ({e}); is another instance running?");
                return;
            }
        };
        loop {
            if server.connect().await.is_err() {
                continue;
            }
            let conn = server;
            server = match ServerOptions::new().reject_remote_clients(true).create(&name) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("hivetty: pipe re-create failed: {e}");
                    return;
                }
            };
            let app = app.clone();
            tauri::async_runtime::spawn(async move { serve(app, conn).await });
        }
    });
}

async fn serve(app: AppHandle, conn: NamedPipeServer) {
    let (r, mut w) = tokio::io::split(conn);
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let id = req.id;
                // Model ops are quick and sync; run them off the async reactor anyway.
                let app2 = app.clone();
                match tauri::async_runtime::spawn_blocking(move || dispatch(&app2, &req.method, req.params)).await {
                    Ok(Ok(v)) => Response::ok(id, v),
                    Ok(Err((code, msg))) => Response::err(id, code, msg),
                    Err(e) => Response::err(id, codes::INTERNAL, e.to_string()),
                }
            }
            Err(e) => Response::err(0, codes::PARSE, e.to_string()),
        };
        let mut out = serde_json::to_vec(&resp).unwrap_or_default();
        out.push(b'\n');
        if w.write_all(&out).await.is_err() {
            break;
        }
    }
}

type RpcResult = Result<Value, (i64, String)>;

fn bad<E: std::fmt::Display>(e: E) -> (i64, String) {
    (codes::INVALID_PARAMS, e.to_string())
}

fn internal<E: std::fmt::Display>(e: E) -> (i64, String) {
    (codes::INTERNAL, e.to_string())
}

fn p<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, (i64, String)> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(bad)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Target {
    pane: Option<PaneId>,
    workspace: Option<String>,
}

/// Resolve a workspace reference: uuid, 1-based index, or name.
fn resolve_ws(st: &AppState, r: Option<&str>) -> Result<Uuid, (i64, String)> {
    let m = st.model.lock();
    let Some(r) = r else { return m.active_workspace.ok_or((codes::NOT_FOUND, "no workspace".into())) };
    if let Ok(id) = Uuid::parse_str(r) {
        if m.ws_index(id).is_some() {
            return Ok(id);
        }
    }
    if let Ok(i) = r.parse::<usize>() {
        if let Some(w) = m.workspaces.get(i.wrapping_sub(1)) {
            return Ok(w.id);
        }
    }
    m.workspaces
        .iter()
        .find(|w| w.name.eq_ignore_ascii_case(r))
        .map(|w| w.id)
        .ok_or((codes::NOT_FOUND, format!("workspace not found: {r}")))
}

fn resolve_pane(st: &AppState, pane: Option<PaneId>) -> Result<PaneId, (i64, String)> {
    let m = st.model.lock();
    match pane {
        Some(p) if m.panes.contains_key(&p) => Ok(p),
        Some(p) => Err((codes::NOT_FOUND, format!("pane not found: {p}"))),
        None => m.focused_pane().ok_or((codes::NOT_FOUND, "no focused pane".into())),
    }
}

pub fn dispatch(app: &AppHandle, method: &str, params: Value) -> RpcResult {
    let st = app.state::<AppState>();
    match method {
        "ping" => Ok(json!({ "version": env!("CARGO_PKG_VERSION") })),

        "state" => Ok(serde_json::to_value(&*st.model.lock()).unwrap_or_default()),

        "list" => {
            let m = st.model.lock();
            let mut nums = st.nums.lock();
            let list: Vec<Value> = m
                .workspaces
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    json!({
                        "index": i + 1,
                        "id": w.id,
                        "name": w.name,
                        "remote": w.remote.as_ref().map(|r| r.label()),
                        "active": m.active_workspace == Some(w.id),
                        "unread": m.workspace_unread(w.id),
                        "surfaces": w.surfaces.iter().map(|s| json!({
                            "id": s.id,
                            "active": s.id == w.active_surface,
                            "focused": s.focused,
                            "panes": s.root.panes().iter().filter_map(|p| m.panes.get(p)).map(|p| json!({
                                "id": p.id, "num": nums.pane(p.id), "title": p.title, "cwd": p.cwd, "kind": p.kind,
                                "unread": p.unread, "running": p.running, "exited": p.exited, "git": p.git, "ports": p.ports, "agent": p.agent,
                            })).collect::<Vec<_>>(),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!(list))
        }

        "notify" => {
            #[derive(Deserialize)]
            struct P {
                pane: Option<PaneId>,
                title: Option<String>,
                body: String,
            }
            let a: P = p(params)?;
            let pane = resolve_pane(&st, a.pane)?;
            let n = app::deliver_notification(app, pane, a.title, a.body).ok_or((codes::NOT_FOUND, "pane not found".to_string()))?;
            Ok(json!({ "id": n.id, "read": n.read }))
        }

        "agent.session" => {
            #[derive(Deserialize)]
            struct P {
                pane: Option<PaneId>,
                agent: String,
                session_id: String,
            }
            let a: P = p(params)?;
            let pane = resolve_pane(&st, a.pane)?;
            if let Some(pn) = st.model.lock().panes.get_mut(&pane) {
                pn.agent = Some(AgentSession { agent: a.agent, session_id: a.session_id });
            }
            emit_state(app);
            Ok(json!({ "ok": true }))
        }

        "workspace.new" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct P {
                name: Option<String>,
                profile: Option<String>,
                cwd: Option<String>,
                command: Option<String>,
                /// false: create it in the background (tmux `new-session -d`)
                focus: Option<bool>,
            }
            let a: P = p(params)?;
            let prev = st.model.lock().active_workspace;
            let (ws, pane) = app::act_new_workspace(app, a.name, a.profile, a.cwd, a.command).map_err(internal)?;
            if a.focus == Some(false) && prev.is_some() {
                st.model.lock().active_workspace = prev;
                emit_state(app);
            }
            Ok(json!({ "workspace": ws, "pane": pane }))
        }

        "workspace.ssh" => {
            #[derive(Deserialize)]
            struct P {
                host: String,
                port: Option<u16>,
                identity: Option<String>,
                #[serde(default)]
                options: Vec<String>,
                name: Option<String>,
                cwd: Option<String>,
                command: Option<String>,
            }
            let a: P = p(params)?;
            let mut r = hivetty_core::remote::Remote::parse(&a.host).map_err(bad)?;
            if a.port.is_some() {
                r.port = a.port;
            }
            r.identity = a.identity;
            r.options = a.options;
            let (ws, pane) = app::act_ssh_workspace(app, r, a.name, a.cwd, a.command).map_err(internal)?;
            Ok(json!({ "workspace": ws, "pane": pane }))
        }

        "workspace.select" => {
            let t: Target = p(params)?;
            let ws = resolve_ws(&st, t.workspace.as_deref())?;
            st.model.lock().activate_workspace(ws);
            emit_state(app);
            Ok(json!({ "workspace": ws }))
        }

        "workspace.rename" => {
            #[derive(Deserialize)]
            struct P {
                workspace: Option<String>,
                name: Option<String>,
                description: Option<String>,
            }
            let a: P = p(params)?;
            let ws = resolve_ws(&st, a.workspace.as_deref())?;
            {
                let mut m = st.model.lock();
                let i = m.ws_index(ws).unwrap();
                if let Some(n) = a.name {
                    m.workspaces[i].name = n;
                }
                if let Some(d) = a.description {
                    m.workspaces[i].description = Some(d).filter(|d| !d.is_empty());
                }
            }
            emit_state(app);
            Ok(json!({ "workspace": ws }))
        }

        "workspace.close" => {
            let t: Target = p(params)?;
            let ws = resolve_ws(&st, t.workspace.as_deref())?;
            let closed = st.model.lock().close_workspace(ws);
            app::teardown(app, &closed);
            emit_state(app);
            Ok(json!({ "closed": closed.panes.len() }))
        }

        "surface.new" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct P {
                workspace: Option<String>,
                profile: Option<String>,
                cwd: Option<String>,
                command: Option<String>,
                title: Option<String>,
                focus: Option<bool>,
            }
            let a: P = p(params)?;
            let ws = resolve_ws(&st, a.workspace.as_deref())?;
            let (near, prev) = {
                let m = st.model.lock();
                let w = &m.workspaces[m.ws_index(ws).unwrap()];
                (w.surfaces.iter().find(|s| s.id == w.active_surface).map(|s| s.focused), (m.active_workspace, w.active_surface))
            };
            let kind = app::kind_near(app, near, a.profile, a.cwd, a.command);
            let (sid, pane) = st.model.lock().new_surface(ws, kind).ok_or((codes::NOT_FOUND, "workspace not found".to_string()))?;
            {
                let mut m = st.model.lock();
                if let Some(i) = m.ws_index(ws) {
                    if let Some(s) = m.workspaces[i].surfaces.iter_mut().find(|s| s.id == sid) {
                        s.title = a.title.filter(|t| !t.trim().is_empty());
                    }
                    if a.focus == Some(false) {
                        m.workspaces[i].active_surface = prev.1;
                        m.active_workspace = prev.0;
                    }
                }
            }
            app::start_pane(app, pane).map_err(internal)?;
            emit_state(app);
            Ok(json!({ "surface": sid, "pane": pane }))
        }

        "pane.split" => {
            #[derive(Deserialize)]
            struct P {
                pane: Option<PaneId>,
                #[serde(default = "default_dir")]
                dir: Dir,
                profile: Option<String>,
                cwd: Option<String>,
                command: Option<String>,
                url: Option<String>,
                /// fraction (0..1) of the space the new pane gets
                size: Option<f32>,
                focus: Option<bool>,
            }
            fn default_dir() -> Dir {
                Dir::Right
            }
            let a: P = p(params)?;
            let target = resolve_pane(&st, a.pane)?;
            let kind = match a.url {
                Some(url) => PaneKind::Browser { url },
                None => app::kind_near(app, Some(target), a.profile, a.cwd, a.command),
            };
            let pane = app::act_split(app, Some(target), a.dir, Some(kind)).map_err(internal)?;
            {
                let mut m = st.model.lock();
                if let Some(loc) = m.locate(pane) {
                    let s = &mut m.workspaces[loc.ws].surfaces[loc.surface];
                    if let Some(f) = a.size {
                        s.root.set_fraction(pane, a.dir.axis(), f);
                    }
                    if a.focus == Some(false) {
                        s.focused = target;
                    }
                }
            }
            emit_state(app);
            Ok(json!({ "pane": pane }))
        }

        "pane.focus" => {
            let t: Target = p(params)?;
            let pane = resolve_pane(&st, t.pane)?;
            st.model.lock().focus_pane(pane);
            emit_state(app);
            Ok(json!({ "pane": pane }))
        }

        "pane.close" => {
            let t: Target = p(params)?;
            let pane = resolve_pane(&st, t.pane)?;
            app::act_close_pane(app, pane);
            Ok(json!({ "pane": pane }))
        }

        "pane.send" => {
            #[derive(Deserialize)]
            struct P {
                pane: Option<PaneId>,
                #[serde(default)]
                text: String,
                #[serde(default)]
                enter: bool,
            }
            let a: P = p(params)?;
            let pane = resolve_pane(&st, a.pane)?;
            let mut data = a.text;
            if a.enter {
                data.push('\r');
            }
            if !st.ptys.write(pane, data.as_bytes()) {
                return Err((codes::NOT_FOUND, "pane has no running terminal".into()));
            }
            Ok(json!({ "pane": pane, "bytes": data.len() }))
        }

        "pane.read" => {
            #[derive(Deserialize)]
            struct P {
                pane: Option<PaneId>,
                #[serde(default)]
                scrollback: usize,
            }
            let a: P = p(params)?;
            let pane = resolve_pane(&st, a.pane)?;
            let text = st.ptys.read_screen(pane, a.scrollback).ok_or((codes::NOT_FOUND, "pane has no running terminal".to_string()))?;
            let (rows, cols) = st.ptys.size(pane).unwrap_or((0, 0));
            Ok(json!({ "pane": pane, "text": text, "rows": rows, "cols": cols }))
        }

        "pane.focus_dir" => {
            #[derive(Deserialize)]
            struct P {
                dir: Dir,
            }
            let a: P = p(params)?;
            let pane = st.model.lock().focus_dir(a.dir);
            emit_state(app);
            Ok(json!({ "pane": pane }))
        }

        "pane.title" => {
            #[derive(Deserialize)]
            struct P {
                pane: Option<PaneId>,
                title: String,
            }
            let a: P = p(params)?;
            let pane = resolve_pane(&st, a.pane)?;
            if let Some(pn) = st.model.lock().panes.get_mut(&pane) {
                pn.title = Some(a.title).filter(|t| !t.is_empty());
            }
            emit_state(app);
            Ok(json!({ "pane": pane }))
        }

        "pane.resize" => {
            #[derive(Deserialize)]
            struct P {
                pane: Option<PaneId>,
                /// fraction (0..1) of the surrounding split along each axis
                width: Option<f32>,
                height: Option<f32>,
                #[serde(default)]
                zoom: bool,
            }
            let a: P = p(params)?;
            let pane = resolve_pane(&st, a.pane)?;
            {
                let mut m = st.model.lock();
                let loc = m.locate(pane).ok_or((codes::NOT_FOUND, "pane not found".to_string()))?;
                let s = &mut m.workspaces[loc.ws].surfaces[loc.surface];
                if let Some(w) = a.width {
                    s.root.set_fraction(pane, hivetty_core::layout::Axis::Row, w);
                }
                if let Some(h) = a.height {
                    s.root.set_fraction(pane, hivetty_core::layout::Axis::Column, h);
                }
                if a.zoom {
                    m.focus_pane(pane);
                    m.toggle_zoom();
                }
            }
            emit_state(app);
            Ok(json!({ "pane": pane }))
        }

        "pane.drop" | "pane.upload" => {
            #[derive(Deserialize)]
            struct P {
                pane: Option<PaneId>,
                paths: Vec<String>,
                /// remote directory (upload only; default: the pane's cwd)
                dir: Option<String>,
            }
            let a: P = p(params)?;
            let pane = resolve_pane(&st, a.pane)?;
            if method == "pane.upload" {
                let (remote, cwd) = {
                    let m = st.model.lock();
                    (m.workspace_of(pane).and_then(|w| w.remote.clone()), m.panes.get(&pane).and_then(|p| p.cwd.clone()))
                };
                let r = remote.ok_or_else(|| bad("not an ssh workspace"))?;
                let dir = a.dir.or(cwd.filter(|c| hivetty_core::paths::is_posix_abs(c))).unwrap_or_default();
                crate::remote::upload(app, pane, r, a.paths, dir);
            } else {
                crate::remote::drop_files(app, pane, a.paths).map_err(internal)?;
            }
            Ok(json!({ "pane": pane }))
        }

        "surface.rename" | "surface.select" | "surface.close" => {
            #[derive(Deserialize)]
            struct P {
                surface: Uuid,
                #[serde(default)]
                title: String,
            }
            let a: P = p(params)?;
            let ws = {
                let m = st.model.lock();
                m.workspaces.iter().find(|w| w.surfaces.iter().any(|s| s.id == a.surface)).map(|w| w.id)
            }
            .ok_or((codes::NOT_FOUND, "tab not found".to_string()))?;
            match method {
                "surface.rename" => {
                    let mut m = st.model.lock();
                    if let Some(s) = m.workspaces.iter_mut().flat_map(|w| w.surfaces.iter_mut()).find(|s| s.id == a.surface) {
                        s.title = Some(a.title).filter(|t| !t.trim().is_empty());
                    }
                }
                "surface.select" => {
                    st.model.lock().activate_surface(ws, a.surface);
                }
                _ => {
                    let closed = st.model.lock().close_surface(ws, a.surface);
                    app::teardown(app, &closed);
                }
            }
            emit_state(app);
            Ok(json!({ "surface": a.surface }))
        }

        "layout.equalize" => {
            let t: Target = p(params)?;
            let pane = resolve_pane(&st, t.pane)?;
            {
                let mut m = st.model.lock();
                if let Some(loc) = m.locate(pane) {
                    m.workspaces[loc.ws].surfaces[loc.surface].root.equalize();
                }
            }
            emit_state(app);
            Ok(json!({ "ok": true }))
        }

        "tmux.state" => Ok(crate::tmux::state(app)),
        "tmux.wait" => {
            #[derive(Deserialize)]
            struct P {
                name: String,
                #[serde(default)]
                mode: String,
            }
            let a: P = p(params)?;
            let w = &st.tmux_waits;
            match a.mode.as_str() {
                "signal" => w.signal(&a.name),
                "lock" => w.lock(&a.name),
                "unlock" => w.unlock(&a.name),
                _ => w.wait(&a.name),
            }
            Ok(json!({ "ok": true }))
        }

        "notifications.list" => Ok(serde_json::to_value(&st.model.lock().notifications).unwrap_or_default()),
        "notifications.clear" => {
            st.model.lock().clear_notifications();
            emit_state(app);
            Ok(json!({ "ok": true }))
        }
        "notifications.jump" => {
            let pane = st.model.lock().latest_unread();
            if let Some(p) = pane {
                st.model.lock().focus_pane(p);
                emit_state(app);
                app::focus_window(app);
            }
            Ok(json!({ "pane": pane }))
        }

        "config.get" => Ok(serde_json::to_value(crate::config::state(&st)).unwrap_or_default()),
        "config.reload" => {
            crate::config::apply(app, crate::config::read());
            let c = crate::config::state(&st);
            Ok(json!({ "path": c.path, "warnings": c.warnings, "error": c.error }))
        }
        "config.set" => {
            #[derive(Deserialize)]
            struct P {
                /// dotted path, e.g. "font.size"
                key: String,
                #[serde(default)]
                value: Value,
                #[serde(default)]
                remove: bool,
            }
            let a: P = p(params)?;
            let path: Vec<String> = a.key.split('.').map(str::to_string).collect();
            let c = crate::config::set(app, &path, (!a.remove).then_some(a.value)).map_err(bad)?;
            Ok(json!({ "path": c.path, "warnings": c.warnings }))
        }

        "update.status" => Ok(serde_json::to_value(crate::updater::status(app)).unwrap_or_default()),
        "update.check" => {
            let st = tauri::async_runtime::block_on(crate::updater::check(app)).map_err(internal)?;
            Ok(serde_json::to_value(st).unwrap_or_default())
        }
        "update.install" => {
            if crate::updater::status(app).state != "available" {
                return Err((codes::NOT_FOUND, "no update available (run update.check first)".into()));
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let ch = tauri::ipc::Channel::new(|_| Ok(()));
                if let Err(e) = crate::updater::install(&app, ch).await {
                    eprintln!("hivetty: update failed: {e}");
                }
            });
            Ok(json!({ "started": true }))
        }

        "session.save" => Ok(json!({ "path": crate::session::save(app).map_err(internal)?.display().to_string() })),

        "browser.profiles" => Ok(serde_json::to_value(crate::chromium::list_profiles()).unwrap_or_default()),
        "browser.import" => {
            #[derive(Deserialize)]
            struct P {
                pane: Option<PaneId>,
                profile: String,
                #[serde(default = "yes")]
                cookies: bool,
                #[serde(default = "yes")]
                history: bool,
                #[serde(default)]
                domains: Vec<String>,
            }
            fn yes() -> bool {
                true
            }
            let a: P = p(params)?;
            let pane = app::find_browser_pane(app, a.pane).ok_or((codes::NOT_FOUND, "no browser pane; open one with `hivetty browser open <url>`".to_string()))?;
            crate::chromium::import(app, pane, &a.profile, a.cookies, a.history, &a.domains).map_err(internal)
        }

        "commands.list" => {
            let t: Target = p(params)?;
            Ok(serde_json::to_value(crate::project::list(app, t.pane)).unwrap_or_default())
        }
        "commands.run" => {
            #[derive(Deserialize)]
            struct P {
                name: String,
                pane: Option<PaneId>,
            }
            let a: P = p(params)?;
            let list = crate::project::list(app, a.pane);
            let c = list
                .commands
                .iter()
                .find(|c| c.name == a.name)
                .or_else(|| list.commands.iter().find(|c| c.name.eq_ignore_ascii_case(&a.name)))
                .ok_or_else(|| (codes::NOT_FOUND, format!("no command named `{}`", a.name)))?;
            let pane = crate::project::run(app, c, a.pane).map_err(internal)?;
            Ok(json!({ "pane": pane, "command": c.command }))
        }

        m if m.starts_with("browser.") => browser_rpc(app, &m["browser.".len()..], params),

        _ => Err((codes::METHOD_NOT_FOUND, format!("unknown method: {method}"))),
    }
}

fn browser_rpc(app: &AppHandle, action: &str, params: Value) -> RpcResult {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct P {
        pane: Option<PaneId>,
        url: Option<String>,
        target: Option<String>,
        text: Option<String>,
        key: Option<String>,
        expr: Option<String>,
        interactive: bool,
        clear: bool,
        path: Option<String>,
        ms: Option<u64>,
        near: Option<PaneId>,
    }
    let a: P = p(params)?;
    let st = app.state::<AppState>();
    let b = &st.browsers;
    let fail = |e: String| (codes::INTERNAL, e);

    if action == "open" {
        let url = a.url.ok_or_else(|| bad("url required"))?;
        let pane = match app::find_browser_pane(app, a.pane) {
            Some(p) if a.pane.is_some() => {
                b.navigate(app, p, &url).map_err(fail)?;
                p
            }
            _ => app::open_in_browser(app, &url, a.near).map_err(internal)?,
        };
        return Ok(json!({ "pane": pane }));
    }

    let pane = app::find_browser_pane(app, a.pane)
        .ok_or((codes::NOT_FOUND, "no browser pane; open one with `hivetty browser open <url>`".to_string()))?;
    let need = |o: Option<String>, what: &str| o.ok_or_else(|| bad(format!("{what} required")));
    let mut v = match action {
        "navigate" => json!({ "url": b.navigate(app, pane, &need(a.url, "url")?).map_err(fail)? }),
        "back" | "forward" | "reload" => {
            let expr = match action {
                "back" => "history.back()",
                "forward" => "history.forward()",
                _ => "location.reload()",
            };
            b.eval(app, pane, expr).map_err(fail)?;
            json!({ "ok": true })
        }
        "snapshot" => b.snapshot(app, pane, a.interactive).map_err(fail)?,
        "click" => b.click(app, pane, &need(a.target, "target")?).map_err(fail)?,
        "fill" => b.fill(app, pane, &need(a.target, "target")?, &a.text.unwrap_or_default()).map_err(fail)?,
        "type" => b.type_text(app, pane, &need(a.text, "text")?).map_err(fail)?,
        "press" => b.press(app, pane, &need(a.key, "key")?).map_err(fail)?,
        "eval" => json!({ "value": b.eval(app, pane, &need(a.expr, "expr")?).map_err(fail)? }),
        "text" => json!({ "text": b.eval(app, pane, "document.body ? document.body.innerText : ''").map_err(fail)? }),
        "url" => json!({
            "url": b.eval(app, pane, "location.href").map_err(fail)?,
            "title": b.eval(app, pane, "document.title").map_err(fail)?,
        }),
        "console" => json!({ "entries": b.logs(pane, false, a.clear).map_err(fail)? }),
        "network" => json!({ "entries": b.logs(pane, true, a.clear).map_err(fail)? }),
        "screenshot" => {
            let data = b.screenshot(app, pane).map_err(fail)?;
            match a.path {
                Some(path) => {
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(internal)?;
                    std::fs::write(&path, bytes).map_err(internal)?;
                    json!({ "path": path })
                }
                None => json!({ "png_base64": data }),
            }
        }
        "wait" => {
            if a.text.is_none() && a.target.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(a.ms.unwrap_or(1000)));
                json!({ "ok": true })
            } else {
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(a.ms.unwrap_or(10_000));
                let expr = match (&a.text, &a.target) {
                    (Some(t), _) => format!("(document.body && document.body.innerText.includes({}))", serde_json::to_string(t).unwrap()),
                    (None, Some(sel)) => format!("!!document.querySelector({})", serde_json::to_string(sel).unwrap()),
                    _ => unreachable!(),
                };
                loop {
                    if b.eval(app, pane, &expr).ok().and_then(|v| v.as_bool()) == Some(true) {
                        break json!({ "ok": true });
                    }
                    if std::time::Instant::now() > deadline {
                        return Err((codes::INTERNAL, "timed out waiting".into()));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
        "devtools" => {
            b.open_devtools(app, pane).map_err(fail)?;
            json!({ "ok": true })
        }
        _ => return Err((codes::METHOD_NOT_FOUND, format!("unknown browser action: {action}"))),
    };
    if let Value::Object(o) = &mut v {
        o.entry("pane").or_insert(json!(pane));
    }
    Ok(v)
}
