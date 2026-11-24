//! Browser panes: child WebView2 instances layered over the pane's rect, plus an agent-facing
//! automation API built on the Chrome DevTools Protocol (CallDevToolsProtocolMethod).

use crate::app::{emit_state, AppState};
use hivetty_core::layout::PaneId;
use hivetty_core::model::PaneKind;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Rect, Webview, WebviewUrl};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    COREWEBVIEW2_PHYSICAL_KEY_STATUS,
};
use webview2_com::{AcceleratorKeyPressedEventHandler, CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR, DevToolsProtocolEventReceivedEventHandler, FocusChangedEventHandler};
use windows::core::{HSTRING, PWSTR};

const LOG_CAP: usize = 500;

/// Chords bound to app actions (same format as the UI's `chordOf`, e.g. "ctrl+shift+w").
/// A focused browser webview swallows keys, so these are intercepted natively and forwarded.
static APP_CHORDS: LazyLock<parking_lot::RwLock<HashSet<String>>> = LazyLock::new(Default::default);

pub fn set_app_chords(chords: Vec<String>) {
    *APP_CHORDS.write() = chords.into_iter().collect();
}

fn vk_key_name(vk: u32) -> Option<String> {
    let s = match vk {
        0x41..=0x5A => return Some(((vk as u8) as char).to_ascii_lowercase().to_string()),
        0x30..=0x39 => return Some(((vk as u8) as char).to_string()),
        0x70..=0x87 => return Some(format!("f{}", vk - 0x6F)),
        0x08 => "backspace",
        0x09 => "tab",
        0x0D => "enter",
        0x1B => "escape",
        0x20 => "space",
        0x21 => "pageup",
        0x22 => "pagedown",
        0x23 => "end",
        0x24 => "home",
        0x25 => "arrowleft",
        0x26 => "arrowup",
        0x27 => "arrowright",
        0x28 => "arrowdown",
        0x2D => "insert",
        0x2E => "delete",
        0xBA => ";",
        0xBB => "=",
        0xBC => ",",
        0xBD => "-",
        0xBE => ".",
        0xBF => "/",
        0xC0 => "`",
        0xDB => "[",
        0xDD => "]",
        _ => return None,
    };
    Some(s.to_string())
}

/// Build a chord from the current modifier state + virtual key, or None for keys we never take.
fn chord_from_vk(vk: u32) -> Option<String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    let down = |k: u16| unsafe { GetKeyState(k as i32) } < 0;
    let key = vk_key_name(vk)?;
    let (ctrl, alt, shift, meta) = (down(VK_CONTROL), down(VK_MENU), down(VK_SHIFT), down(VK_LWIN) || down(VK_RWIN));
    // Never steal plain typing from the page: require a non-Shift modifier unless it's an F-key.
    if !(ctrl || alt || meta) && !(key.starts_with('f') && key.len() > 1) {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("ctrl");
    }
    if alt {
        parts.push("alt");
    }
    if shift {
        parts.push("shift");
    }
    if meta {
        parts.push("meta");
    }
    parts.push(&key);
    Some(parts.join("+"))
}

/// Mark the browser pane focused in the model (clicks inside the native webview never reach the UI).
fn focus_in_model(app: &AppHandle, pane: PaneId) {
    let st = app.state::<AppState>();
    let changed = st.model.lock().focus_pane(pane);
    if changed {
        emit_state(app);
    }
}

/// Run an app shortcut pressed while a browser webview had keyboard focus.
fn forward_shortcut(app: &AppHandle, pane: PaneId, chord: String) {
    focus_in_model(app, pane);
    // Give keyboard focus back to the app UI so overlays (palette, rename…) receive typing.
    if let Some(main) = app.get_webview("main") {
        let _ = main.set_focus();
    }
    let _ = app.emit_to("main", "app-shortcut", chord);
}

/// Intercept app shortcuts and track focus on a browser pane's WebView2 controller.
fn hook_shortcuts(wv: &Webview, app: AppHandle, pane: PaneId) {
    let _ = wv.with_webview(move |pw| {
        let ctl = pw.controller();
        let a1 = app.clone();
        let keys = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
            unsafe { args.KeyEventKind(&mut kind)? };
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
                return Ok(());
            }
            let mut vk = 0u32;
            unsafe { args.VirtualKey(&mut vk)? };
            let Some(chord) = chord_from_vk(vk) else { return Ok(()) };
            if !APP_CHORDS.read().contains(&chord) {
                return Ok(());
            }
            unsafe { args.SetHandled(true)? };
            // Swallow auto-repeat, but only act on the first press (holding Ctrl+Shift+W
            // must not close pane after pane).
            let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
            unsafe { args.PhysicalKeyStatus(&mut status)? };
            if status.WasKeyDown.as_bool() {
                return Ok(());
            }
            // Defer: closing this webview from inside its own event handler isn't safe.
            let app = a1.clone();
            let _ = a1.run_on_main_thread(move || forward_shortcut(&app, pane, chord));
            Ok(())
        }));
        let a2 = app.clone();
        let focus = FocusChangedEventHandler::create(Box::new(move |_, _| {
            let app = a2.clone();
            let _ = a2.run_on_main_thread(move || focus_in_model(&app, pane));
            Ok(())
        }));
        let (mut t1, mut t2) = (0i64, 0i64);
        unsafe {
            let _ = ctl.add_AcceleratorKeyPressed(&keys, &mut t1);
            let _ = ctl.add_GotFocus(&focus, &mut t2);
        }
    });
}

#[derive(Default)]
struct Logs {
    console: VecDeque<Value>,
    network: VecDeque<Value>,
}

struct Entry {
    label: String,
    /// snapshot ref ("e12") -> backendDOMNodeId
    refs: HashMap<String, i64>,
    logs: Arc<Mutex<Logs>>,
    visible: bool,
    bounds: (f64, f64, f64, f64),
}

#[derive(Default)]
pub struct BrowserManager {
    entries: Mutex<HashMap<PaneId, Entry>>,
}

pub type BResult<T> = Result<T, String>;

/// A separate WebView2 profile with its own browser arguments (remote workspaces' SOCKS proxy).
pub struct BrowserProxy {
    pub data_dir: std::path::PathBuf,
    pub args: String,
}

pub fn label_for(pane: PaneId) -> String {
    format!("browser-{}", pane.simple())
}

pub fn normalize_url(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return "about:blank".into();
    }
    if s.contains("://") || s.starts_with("about:") || s.starts_with("data:") {
        return s.to_string();
    }
    if let Some(port) = s.strip_prefix(':').filter(|p| p.chars().all(|c| c.is_ascii_digit())) {
        return format!("http://localhost:{port}");
    }
    if s.chars().all(|c| c.is_ascii_digit()) && s.len() <= 5 {
        return format!("http://localhost:{s}");
    }
    let looks_local = s.starts_with("localhost") || s.starts_with("127.") || s.starts_with("0.0.0.0") || s.starts_with("[::1]");
    if looks_local {
        return format!("http://{s}");
    }
    if s.contains('.') && !s.contains(' ') {
        return format!("https://{s}");
    }
    format!("https://www.bing.com/search?q={}", url_encode(s))
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn set_pane_url(app: &AppHandle, pane: PaneId, url: &str) {
    // placeholder/error pages shouldn't replace the URL the pane is meant to show
    if url == "about:blank" || url.starts_with("data:") {
        return;
    }
    let st = app.state::<AppState>();
    let changed = {
        let mut m = st.model.lock();
        match m.panes.get_mut(&pane) {
            Some(p) => match &mut p.kind {
                PaneKind::Browser { url: u } if u != url => {
                    *u = url.to_string();
                    true
                }
                _ => false,
            },
            None => false,
        }
    };
    if changed {
        emit_state(app);
    }
}

impl BrowserManager {
    /// Create the child webview for a browser pane (hidden until the UI positions it).
    pub fn create(&self, app: &AppHandle, pane: PaneId, url: &str, proxy: Option<BrowserProxy>) -> BResult<()> {
        if self.entries.lock().contains_key(&pane) {
            return Ok(());
        }
        let window = app.get_window("main").ok_or("main window missing")?;
        let label = label_for(pane);
        let parsed: tauri::Url = normalize_url(url).parse().map_err(|e| format!("bad url: {e}"))?;
        let (a1, a2, a3) = (app.clone(), app.clone(), app.clone());
        let builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed))
            .on_navigation(move |u| {
                set_pane_url(&a1, pane, u.as_str());
                true
            })
            .on_document_title_changed(move |_, title| {
                let st = a2.state::<AppState>();
                if let Some(p) = st.model.lock().panes.get_mut(&pane) {
                    p.title = Some(title).filter(|t| !t.is_empty());
                }
                emit_state(&a2);
            })
            // target=_blank / window.open: keep it in this pane
            .on_new_window(move |u, _| {
                if let Some(wv) = a3.get_webview(&label_for(pane)) {
                    let _ = wv.navigate(u);
                }
                NewWindowResponse::Deny
            });
        let builder = match proxy {
            Some(p) => {
                let _ = std::fs::create_dir_all(&p.data_dir);
                builder.data_directory(p.data_dir).additional_browser_args(&p.args)
            }
            None => builder,
        };
        let wv = window
            .add_child(builder, LogicalPosition::new(0.0, 0.0), LogicalSize::new(1.0, 1.0))
            .map_err(|e| format!("create webview: {e}"))?;
        let _ = wv.hide();
        let logs = Arc::new(Mutex::new(Logs::default()));
        hook_events(&wv, logs.clone());
        hook_shortcuts(&wv, app.clone(), pane);
        self.entries.lock().insert(pane, Entry { label, refs: HashMap::new(), logs, visible: false, bounds: (0.0, 0.0, 0.0, 0.0) });
        Ok(())
    }

    pub fn close(&self, app: &AppHandle, pane: PaneId) {
        if let Some(e) = self.entries.lock().remove(&pane) {
            if let Some(wv) = app.get_webview(&e.label) {
                let _ = wv.close();
            }
        }
    }

    /// Re-apply every visible webview's bounds. Logical bounds don't change when the window
    /// moves to a monitor with a different scale, but the physical rect does.
    pub fn reapply_bounds(&self, app: &AppHandle) {
        let list: Vec<(String, (f64, f64, f64, f64))> = self.entries.lock().values().filter(|e| e.visible).map(|e| (e.label.clone(), e.bounds)).collect();
        for (label, (x, y, w, h)) in list {
            if let Some(wv) = app.get_webview(&label) {
                let _ = wv.set_bounds(Rect { position: LogicalPosition::new(x, y).into(), size: LogicalSize::new(w, h).into() });
            }
        }
    }

    pub fn close_all(&self, app: &AppHandle) {
        let panes: Vec<_> = self.entries.lock().keys().copied().collect();
        for p in panes {
            self.close(app, p);
        }
    }

    pub fn has(&self, pane: PaneId) -> bool {
        self.entries.lock().contains_key(&pane)
    }

    fn webview(&self, app: &AppHandle, pane: PaneId) -> BResult<Webview> {
        let label = self.entries.lock().get(&pane).map(|e| e.label.clone()).ok_or("not a browser pane")?;
        app.get_webview(&label).ok_or_else(|| "browser webview missing".to_string())
    }

    /// Position/show/hide from the UI layout (logical px relative to the window client area).
    pub fn set_bounds(&self, app: &AppHandle, pane: PaneId, x: f64, y: f64, w: f64, h: f64, visible: bool) {
        let visible = visible && w >= 2.0 && h >= 2.0;
        let (label, need_bounds, need_vis) = {
            let mut entries = self.entries.lock();
            let Some(e) = entries.get_mut(&pane) else { return };
            let nb = visible && e.bounds != (x, y, w, h);
            let nv = e.visible != visible;
            if nb {
                e.bounds = (x, y, w, h);
            }
            e.visible = visible;
            (e.label.clone(), nb, nv)
        };
        let Some(wv) = app.get_webview(&label) else { return };
        if need_bounds {
            let _ = wv.set_bounds(Rect { position: LogicalPosition::new(x, y).into(), size: LogicalSize::new(w, h).into() });
        }
        if need_vis {
            let _ = if visible { wv.show() } else { wv.hide() };
        }
    }

    pub fn navigate(&self, app: &AppHandle, pane: PaneId, url: &str) -> BResult<String> {
        let url = normalize_url(url);
        let parsed: tauri::Url = url.parse().map_err(|e| format!("bad url: {e}"))?;
        self.webview(app, pane)?.navigate(parsed).map_err(|e| e.to_string())?;
        set_pane_url(app, pane, &url);
        Ok(url)
    }

    pub fn focus(&self, app: &AppHandle, pane: PaneId) {
        if let Ok(wv) = self.webview(app, pane) {
            let _ = wv.set_focus();
        }
    }

    pub fn open_devtools(&self, app: &AppHandle, pane: PaneId) -> BResult<()> {
        let wv = self.webview(app, pane)?;
        wv.with_webview(|pw| unsafe {
            if let Ok(core) = pw.controller().CoreWebView2() {
                let _ = core.OpenDevToolsWindow();
            }
        })
        .map_err(|e| e.to_string())
    }

    /// Call a CDP method and wait for its JSON result. Must not be called on the main thread.
    pub fn cdp(&self, app: &AppHandle, pane: PaneId, method: &str, params: Value) -> BResult<Value> {
        let wv = self.webview(app, pane)?;
        cdp_call(&wv, method, params)
    }

    // ---------------- automation ----------------

    pub fn snapshot(&self, app: &AppHandle, pane: PaneId, interactive_only: bool) -> BResult<Value> {
        let tree = self.cdp(app, pane, "Accessibility.getFullAXTree", json!({}))?;
        let (text, refs) = render_ax_tree(&tree, interactive_only);
        let url = self.cdp(app, pane, "Runtime.evaluate", json!({"expression": "location.href", "returnByValue": true})).ok();
        let url = url.and_then(|v| v.pointer("/result/value").and_then(|u| u.as_str()).map(str::to_string));
        let count = refs.len();
        if let Some(e) = self.entries.lock().get_mut(&pane) {
            e.refs = refs;
        }
        Ok(json!({ "url": url, "snapshot": text, "refs": count }))
    }

    /// Resolve "@e12" / "e12" refs or a CSS selector to a backendNodeId.
    fn resolve(&self, app: &AppHandle, pane: PaneId, target: &str) -> BResult<i64> {
        let t = target.trim();
        let r = t.strip_prefix('@').unwrap_or(t);
        if r.starts_with('e') && r[1..].chars().all(|c| c.is_ascii_digit()) && r.len() > 1 {
            return self
                .entries
                .lock()
                .get(&pane)
                .and_then(|e| e.refs.get(r).copied())
                .ok_or_else(|| format!("unknown ref {r}; run `hivetty browser snapshot` first"));
        }
        let doc = self.cdp(app, pane, "DOM.getDocument", json!({"depth": 0}))?;
        let root = doc.pointer("/root/nodeId").and_then(Value::as_i64).ok_or("no document")?;
        let q = self.cdp(app, pane, "DOM.querySelector", json!({"nodeId": root, "selector": t}))?;
        let node = q.get("nodeId").and_then(Value::as_i64).filter(|n| *n != 0).ok_or_else(|| format!("no element matches {t}"))?;
        let d = self.cdp(app, pane, "DOM.describeNode", json!({"nodeId": node}))?;
        d.pointer("/node/backendNodeId").and_then(Value::as_i64).ok_or_else(|| "could not resolve element".into())
    }

    pub fn click(&self, app: &AppHandle, pane: PaneId, target: &str) -> BResult<Value> {
        let id = self.resolve(app, pane, target)?;
        let _ = self.cdp(app, pane, "DOM.scrollIntoViewIfNeeded", json!({"backendNodeId": id}));
        let bm = self.cdp(app, pane, "DOM.getBoxModel", json!({"backendNodeId": id}))?;
        let q: Vec<f64> = bm
            .pointer("/model/content")
            .and_then(Value::as_array)
            .ok_or("element has no box (hidden?)")?
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        if q.len() < 8 {
            return Err("element has no box".into());
        }
        let x = (q[0] + q[2] + q[4] + q[6]) / 4.0;
        let y = (q[1] + q[3] + q[5] + q[7]) / 4.0;
        for (ty, buttons) in [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 0)] {
            let mut p = json!({"type": ty, "x": x, "y": y, "button": "left", "buttons": buttons});
            if ty != "mouseMoved" {
                p["clickCount"] = json!(1);
            }
            self.cdp(app, pane, "Input.dispatchMouseEvent", p)?;
        }
        Ok(json!({"x": x, "y": y}))
    }

    pub fn fill(&self, app: &AppHandle, pane: PaneId, target: &str, text: &str) -> BResult<Value> {
        let id = self.resolve(app, pane, target)?;
        let obj = self.cdp(app, pane, "DOM.resolveNode", json!({"backendNodeId": id}))?;
        let oid = obj.pointer("/object/objectId").and_then(Value::as_str).ok_or("cannot resolve element")?;
        self.cdp(
            app,
            pane,
            "Runtime.callFunctionOn",
            json!({
                "objectId": oid,
                "functionDeclaration": "function(){ this.scrollIntoView({block:'center'}); this.focus(); if (typeof this.select === 'function') this.select(); else { const r = document.createRange(); r.selectNodeContents(this); const s = getSelection(); s.removeAllRanges(); s.addRange(r); } }",
            }),
        )?;
        if text.is_empty() {
            self.cdp(app, pane, "Input.dispatchKeyEvent", json!({"type": "keyDown", "key": "Delete", "code": "Delete", "windowsVirtualKeyCode": 46}))?;
            self.cdp(app, pane, "Input.dispatchKeyEvent", json!({"type": "keyUp", "key": "Delete", "code": "Delete", "windowsVirtualKeyCode": 46}))?;
        } else {
            self.cdp(app, pane, "Input.insertText", json!({"text": text}))?;
        }
        Ok(json!({"ok": true}))
    }

    pub fn type_text(&self, app: &AppHandle, pane: PaneId, text: &str) -> BResult<Value> {
        self.cdp(app, pane, "Input.insertText", json!({"text": text}))?;
        Ok(json!({"ok": true}))
    }

    pub fn press(&self, app: &AppHandle, pane: PaneId, key: &str) -> BResult<Value> {
        let (k, code, vk, text) = key_info(key);
        let mut down = json!({"type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": k, "code": code, "windowsVirtualKeyCode": vk});
        if let Some(t) = text {
            down["text"] = json!(t);
        }
        self.cdp(app, pane, "Input.dispatchKeyEvent", down)?;
        self.cdp(app, pane, "Input.dispatchKeyEvent", json!({"type": "keyUp", "key": k, "code": code, "windowsVirtualKeyCode": vk}))?;
        Ok(json!({"ok": true}))
    }

    pub fn eval(&self, app: &AppHandle, pane: PaneId, expr: &str) -> BResult<Value> {
        let r = self.cdp(app, pane, "Runtime.evaluate", json!({"expression": expr, "returnByValue": true, "awaitPromise": true, "userGesture": true}))?;
        if let Some(ex) = r.get("exceptionDetails") {
            let msg = ex.pointer("/exception/description").or(ex.get("text")).and_then(Value::as_str).unwrap_or("evaluation failed");
            return Err(msg.to_string());
        }
        Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    pub fn screenshot(&self, app: &AppHandle, pane: PaneId) -> BResult<String> {
        let r = self.cdp(app, pane, "Page.captureScreenshot", json!({"format": "png"}))?;
        r.get("data").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "no screenshot data".into())
    }

    pub fn logs(&self, pane: PaneId, network: bool, clear: bool) -> BResult<Value> {
        let logs = self.entries.lock().get(&pane).map(|e| e.logs.clone()).ok_or("not a browser pane")?;
        let mut l = logs.lock();
        let q = if network { &mut l.network } else { &mut l.console };
        let out = json!(q.iter().cloned().collect::<Vec<_>>());
        if clear {
            q.clear();
        }
        Ok(out)
    }
}

fn key_info(key: &str) -> (String, String, i64, Option<String>) {
    let named: &[(&str, &str, i64, Option<&str>)] = &[
        ("Enter", "Enter", 13, Some("\r")),
        ("Tab", "Tab", 9, None),
        ("Escape", "Escape", 27, None),
        ("Backspace", "Backspace", 8, None),
        ("Delete", "Delete", 46, None),
        ("ArrowUp", "ArrowUp", 38, None),
        ("ArrowDown", "ArrowDown", 40, None),
        ("ArrowLeft", "ArrowLeft", 37, None),
        ("ArrowRight", "ArrowRight", 39, None),
        ("Home", "Home", 36, None),
        ("End", "End", 35, None),
        ("PageUp", "PageUp", 33, None),
        ("PageDown", "PageDown", 34, None),
        ("Space", "Space", 32, Some(" ")),
    ];
    for (n, code, vk, text) in named {
        if n.eq_ignore_ascii_case(key) || (key.eq_ignore_ascii_case("esc") && *n == "Escape") {
            let k = if *n == "Space" { " " } else { n };
            return (k.to_string(), code.to_string(), *vk, text.map(str::to_string));
        }
    }
    let c = key.chars().next().unwrap_or(' ');
    let vk = c.to_ascii_uppercase() as i64;
    let code = if c.is_ascii_alphabetic() { format!("Key{}", c.to_ascii_uppercase()) } else if c.is_ascii_digit() { format!("Digit{c}") } else { String::new() };
    (key.to_string(), code, vk, Some(key.to_string()))
}

fn cdp_call(wv: &Webview, method: &str, params: Value) -> BResult<Value> {
    let (tx, rx) = std::sync::mpsc::channel::<BResult<String>>();
    let method = method.to_string();
    let params = params.to_string();
    wv.with_webview(move |pw| {
        let tx_err = tx.clone();
        let r = (|| -> windows::core::Result<()> {
            let core: ICoreWebView2 = unsafe { pw.controller().CoreWebView2()? };
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr, json| {
                let _ = tx.send(hr.map(|_| json).map_err(|e| e.message().to_string()));
                Ok(())
            }));
            unsafe { core.CallDevToolsProtocolMethod(&HSTRING::from(method.as_str()), &HSTRING::from(params.as_str()), &handler) }
        })();
        if let Err(e) = r {
            let _ = tx_err.send(Err(e.message().to_string()));
        }
    })
    .map_err(|e| e.to_string())?;
    let s = rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "browser did not respond (timeout)".to_string())??;
    serde_json::from_str(&s).map_err(|e| format!("bad CDP response: {e}"))
}

/// Subscribe to console and network CDP events into ring buffers.
fn hook_events(wv: &Webview, logs: Arc<Mutex<Logs>>) {
    let wv2 = wv.clone();
    let _ = wv.with_webview(move |pw| {
        let Ok(core) = (unsafe { pw.controller().CoreWebView2() }) else { return };
        let subscribe = |event: &'static str, network: bool| {
            let logs = logs.clone();
            let Ok(recv) = (unsafe { core.GetDevToolsProtocolEventReceiver(&HSTRING::from(event)) }) else { return };
            let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut p = PWSTR::null();
                unsafe { args.ParameterObjectAsJson(&mut p)? };
                let raw = CoTaskMemPWSTR::from(p).to_string();
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    let entry = summarize_event(event, &v);
                    let mut l = logs.lock();
                    let q = if network { &mut l.network } else { &mut l.console };
                    q.push_back(entry);
                    while q.len() > LOG_CAP {
                        q.pop_front();
                    }
                }
                Ok(())
            }));
            let mut token = Default::default();
            let _ = unsafe { recv.add_DevToolsProtocolEventReceived(&handler, &mut token) };
        };
        subscribe("Runtime.consoleAPICalled", false);
        subscribe("Runtime.exceptionThrown", false);
        subscribe("Log.entryAdded", false);
        subscribe("Network.responseReceived", true);
        subscribe("Network.loadingFailed", true);
    });
    // Enabling domains waits for a CDP response, so do it off the main thread.
    std::thread::spawn(move || {
        for m in ["Runtime.enable", "Log.enable", "Network.enable", "Page.enable"] {
            let _ = cdp_call(&wv2, m, json!({}));
        }
    });
}

fn summarize_event(event: &str, v: &Value) -> Value {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
    match event {
        "Runtime.consoleAPICalled" => {
            let args: Vec<String> = v
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| match x.get("value") {
                            Some(Value::String(s)) => s.clone(),
                            Some(other) => other.to_string(),
                            None => x.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            json!({"level": s("/type"), "text": args.join(" ")})
        }
        "Runtime.exceptionThrown" => json!({"level": "error", "text": v.pointer("/exceptionDetails/exception/description").and_then(Value::as_str).unwrap_or(&s("/exceptionDetails/text"))}),
        "Log.entryAdded" => json!({"level": s("/entry/level"), "text": s("/entry/text"), "url": s("/entry/url")}),
        "Network.responseReceived" => json!({
            "url": s("/response/url"),
            "status": v.pointer("/response/status").cloned().unwrap_or(Value::Null),
            "type": s("/type"),
            "mime": s("/response/mimeType"),
        }),
        "Network.loadingFailed" => json!({"error": s("/errorText"), "type": s("/type"), "requestId": s("/requestId")}),
        _ => v.clone(),
    }
}

const INTERACTIVE: &[&str] = &[
    "button", "link", "textbox", "searchbox", "combobox", "checkbox", "radio", "menuitem", "menuitemcheckbox",
    "menuitemradio", "tab", "switch", "slider", "spinbutton", "option", "listbox", "treeitem",
];
const SKIP_ROLES: &[&str] = &["none", "generic", "InlineTextBox", "LineBreak", "presentation", "RootWebArea", "ignored"];

/// Render a CDP AX tree into an indented outline. Interactive elements get refs like [ref=e3].
pub fn render_ax_tree(tree: &Value, interactive_only: bool) -> (String, HashMap<String, i64>) {
    let nodes = tree.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
    let by_id: HashMap<&str, &Value> = nodes.iter().filter_map(|n| Some((n.get("nodeId")?.as_str()?, n))).collect();
    let mut out = String::new();
    let mut refs = HashMap::new();
    let mut next = 1;
    let Some(root) = nodes.first() else { return (out, refs) };

    fn walk(
        n: &Value,
        depth: usize,
        by_id: &HashMap<&str, &Value>,
        out: &mut String,
        refs: &mut HashMap<String, i64>,
        next: &mut usize,
        interactive_only: bool,
        parent_name: &str,
    ) {
        if out.len() > 200_000 {
            return;
        }
        let ignored = n.get("ignored").and_then(Value::as_bool).unwrap_or(false);
        let role = n.pointer("/role/value").and_then(Value::as_str).unwrap_or("");
        let name = n.pointer("/name/value").and_then(Value::as_str).unwrap_or("").trim();
        let name: String = name.chars().take(200).collect();
        let interactive = INTERACTIVE.contains(&role);
        let is_text = role == "StaticText";
        let show = !ignored
            && !SKIP_ROLES.contains(&role)
            && !(is_text && (name.is_empty() || name == parent_name))
            && (!interactive_only || interactive);
        let mut child_depth = depth;
        if show {
            let indent = "  ".repeat(depth);
            let label = if is_text { "text".to_string() } else { role.to_string() };
            out.push_str(&format!("{indent}- {label}"));
            if !name.is_empty() {
                out.push_str(&format!(" \"{}\"", name.replace('"', "'")));
            }
            for prop in n.get("properties").and_then(Value::as_array).into_iter().flatten() {
                let pn = prop.get("name").and_then(Value::as_str).unwrap_or("");
                let pv = prop.pointer("/value/value");
                match (pn, pv) {
                    ("checked" | "selected" | "expanded" | "pressed" | "disabled" | "required", Some(Value::Bool(true))) => out.push_str(&format!(" [{pn}]")),
                    ("checked", Some(Value::String(s))) if s == "true" || s == "mixed" => out.push_str(&format!(" [checked={s}]")),
                    ("level", Some(v)) => out.push_str(&format!(" [level={v}]")),
                    _ => {}
                }
            }
            if let Some(v) = n.pointer("/value/value").and_then(Value::as_str).filter(|v| !v.is_empty()) {
                out.push_str(&format!(" value=\"{}\"", v.chars().take(100).collect::<String>().replace('"', "'")));
            }
            let refable = interactive || role == "heading" || (role == "img" && !name.is_empty());
            if refable {
                if let Some(bid) = n.get("backendDOMNodeId").and_then(Value::as_i64) {
                    let r = format!("e{next}");
                    *next += 1;
                    out.push_str(&format!(" [ref={r}]"));
                    refs.insert(r, bid);
                }
            }
            out.push('\n');
            child_depth = depth + 1;
        }
        let pname = if show && !name.is_empty() { name.as_str() } else { parent_name };
        for cid in n.get("childIds").and_then(Value::as_array).into_iter().flatten() {
            if let Some(c) = cid.as_str().and_then(|id| by_id.get(id)) {
                walk(c, child_depth, by_id, out, refs, next, interactive_only, pname);
            }
        }
    }
    walk(root, 0, &by_id, &mut out, &mut refs, &mut next, interactive_only, "");
    (out, refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url(":5173"), "http://localhost:5173");
        assert_eq!(normalize_url("8080"), "http://localhost:8080");
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("https://x.dev/a"), "https://x.dev/a");
        assert!(normalize_url("rust lifetimes").starts_with("https://www.bing.com/search?q=rust+lifetimes"));
    }

    #[test]
    fn ax_render() {
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Demo"}, "childIds": ["2", "3", "5"]},
            {"nodeId": "2", "role": {"value": "heading"}, "name": {"value": "Login"}, "backendDOMNodeId": 10, "properties": [{"name": "level", "value": {"value": 1}}], "childIds": ["6"]},
            {"nodeId": "6", "role": {"value": "StaticText"}, "name": {"value": "Login"}},
            {"nodeId": "3", "role": {"value": "generic"}, "childIds": ["4"]},
            {"nodeId": "4", "role": {"value": "textbox"}, "name": {"value": "Email"}, "backendDOMNodeId": 11, "value": {"value": "a@b.c"}},
            {"nodeId": "5", "role": {"value": "button"}, "name": {"value": "Sign in"}, "backendDOMNodeId": 12, "ignored": false}
        ]});
        let (text, refs) = render_ax_tree(&tree, false);
        assert_eq!(text, "- heading \"Login\" [level=1] [ref=e1]\n- textbox \"Email\" value=\"a@b.c\" [ref=e2]\n- button \"Sign in\" [ref=e3]\n");
        assert_eq!(refs["e3"], 12);
        let (only, _) = render_ax_tree(&tree, true);
        assert!(!only.contains("heading") && only.contains("button"));
    }

    #[test]
    fn keys() {
        assert_eq!(key_info("enter").2, 13);
        assert_eq!(key_info("a").1, "KeyA");
    }
}
