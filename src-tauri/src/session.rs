//! Session persistence: `%APPDATA%\Hivetty\sessions\last.json` plus one scrollback file per
//! terminal in `sessions\scrollback\`. Saved every few seconds when something changed, and on exit.

use crate::app::{now_ms, AppState};
use crate::config::{data_dir, write_atomic};
use hivetty_core::layout::PaneId;
use hivetty_core::model::{Model, PaneKind};
use hivetty_core::redact::redact;
use hivetty_core::session::{self, SessionFile};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Raw-output fallback for terminals the UI never rendered (bytes).
const RAW_TAIL: usize = 256 * 1024;
const AUTOSAVE: Duration = Duration::from_secs(8);

#[derive(Default)]
pub struct SessionStore {
    pub dirty: AtomicBool,
    /// Close handshake: 0 = open, 1 = waiting for the UI to flush, 2 = flushed / closing.
    closing: std::sync::atomic::AtomicU8,
    /// Latest serialized buffer the UI sent for each terminal.
    scrollback: Mutex<HashMap<PaneId, String>>,
    /// Scrollback loaded from disk that the UI hasn't picked up yet.
    restored: Mutex<HashMap<PaneId, String>>,
    save_lock: Mutex<()>,
}

fn debug() -> bool {
    std::env::var_os("HIVETTY_DEBUG_SESSION").is_some()
}

pub fn dir() -> PathBuf {
    data_dir().join("sessions")
}

fn file() -> PathBuf {
    dir().join("last.json")
}

fn scrollback_dir() -> PathBuf {
    dir().join("scrollback")
}

fn scrollback_file(p: PaneId) -> PathBuf {
    scrollback_dir().join(format!("{p}.txt"))
}

impl SessionStore {
    pub fn put_scrollback(&self, pane: PaneId, data: String) {
        if debug() {
            eprintln!("[session] put {pane} len={} tail={:?}", data.len(), data.chars().rev().take(40).collect::<String>().chars().rev().collect::<String>());
        }
        self.restored.lock().remove(&pane);
        self.scrollback.lock().insert(pane, data);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn take_restored(&self, pane: PaneId) -> Option<String> {
        // Keep a copy as the pane's scrollback until the UI sends a fresh one.
        let s = self.restored.lock().remove(&pane)?;
        self.scrollback.lock().entry(pane).or_insert_with(|| s.clone());
        Some(s)
    }

    /// Discard saved scrollback for a pane that reattached to its still-running shell.
    pub fn drop_restored(&self, pane: PaneId) {
        self.restored.lock().remove(&pane);
    }

    pub fn forget(&self, pane: PaneId) {
        self.scrollback.lock().remove(&pane);
        self.restored.lock().remove(&pane);
    }
}

/// Load the last session. Returns the model and saved scrollback per pane.
pub fn load() -> Option<(Model, HashMap<PaneId, String>)> {
    let text = std::fs::read_to_string(file()).ok()?;
    let parsed: SessionFile = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("hivetty: ignoring damaged session file: {e}");
            let _ = std::fs::copy(file(), dir().join("last.damaged.json"));
            return None;
        }
    };
    let model = match session::restore(parsed) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("hivetty: can't restore session: {e}");
            return None;
        }
    };
    let mut sb = HashMap::new();
    for (id, p) in &model.panes {
        if matches!(p.kind, PaneKind::Terminal { .. }) {
            if let Ok(s) = std::fs::read_to_string(scrollback_file(*id)) {
                if !s.is_empty() {
                    sb.insert(*id, s);
                }
            }
        }
    }
    Some((model, sb))
}

/// Called on the main window's close request. Returns true if the close should be held while
/// the UI flushes scrollback (a second request, or the UI's ack, lets it through).
pub fn begin_close(app: &AppHandle) -> bool {
    if debug() {
        eprintln!("[session] close requested");
    }
    let st = app.state::<AppState>();
    let cfg = st.config.lock().config.session.clone();
    if !cfg.restore || !cfg.scrollback {
        return false;
    }
    if st.session.closing.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return false; // already waiting: let this one through
    }
    let _ = app.emit("session-flush", ());
    let app = app.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let deadline = std::time::Instant::now() + Duration::from_millis(1500);
        while st.session.closing.load(Ordering::SeqCst) == 1 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        st.session.closing.store(2, Ordering::SeqCst);
        if let Some(w) = app.get_window("main") {
            let _ = w.destroy();
        }
    });
    true
}

/// The UI finished sending scrollback after a `session-flush` request.
pub fn flushed(app: &AppHandle) {
    let _ = app.state::<AppState>().session.closing.compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst);
}

pub fn set_restored(app: &AppHandle, sb: HashMap<PaneId, String>) {
    *app.state::<AppState>().session.restored.lock() = sb;
}

/// Last `max` bytes of raw output, starting at a line boundary.
fn raw_tail(bytes: Vec<u8>, max: usize) -> String {
    let start = bytes.len().saturating_sub(max);
    let mut s = &bytes[start..];
    if start > 0 {
        if let Some(nl) = s.iter().position(|b| *b == b'\n') {
            s = &s[nl + 1..];
        }
    }
    String::from_utf8_lossy(s).into_owned()
}

/// Keep at most `lines` trailing lines.
fn cap_lines(s: &str, lines: usize) -> &str {
    let mut count = 0;
    for (i, b) in s.bytes().enumerate().rev() {
        if b == b'\n' {
            count += 1;
            if count >= lines {
                return &s[i + 1..];
            }
        }
    }
    s
}

/// Save the current session. Safe to call from any thread; concurrent calls serialize.
pub fn save(app: &AppHandle) -> anyhow::Result<PathBuf> {
    let st = app.state::<AppState>();
    let _guard = st.session.save_lock.lock();
    let cfg = st.config.lock().config.session.clone();
    st.session.dirty.store(false, Ordering::Relaxed);
    let snap = session::snapshot(&st.model.lock(), now_ms());
    let path = file();
    std::fs::create_dir_all(dir())?;

    let terminals: Vec<PaneId> = snap.panes.iter().filter(|p| matches!(p.kind, PaneKind::Terminal { .. })).map(|p| p.id).collect();
    let sbdir = scrollback_dir();
    if cfg.scrollback {
        std::fs::create_dir_all(&sbdir)?;
        let ui = st.session.scrollback.lock().clone();
        let restored = st.session.restored.lock().clone();
        for id in &terminals {
            let text = match (ui.get(id), restored.get(id)) {
                (Some(s), _) => s.clone(),
                // never rendered: what we restored plus whatever it printed since
                (None, r) => {
                    let raw = st.ptys.replay(*id).map(|b| raw_tail(b, RAW_TAIL)).unwrap_or_default();
                    match r {
                        Some(r) if raw.is_empty() => r.clone(),
                        Some(r) => format!("{r}\r\n{raw}"),
                        None => raw,
                    }
                }
            };
            let text = redact(cap_lines(&text, cfg.scrollback_lines as usize));
            if debug() {
                eprintln!("[session] save {id} from={} len={}", if ui.contains_key(id) { "ui" } else { "raw" }, text.len());
            }
            let f = scrollback_file(*id);
            if text.trim().is_empty() {
                let _ = std::fs::remove_file(f);
            } else if std::fs::read_to_string(&f).ok().as_deref() != Some(text.as_str()) {
                write_atomic(&f, text.as_bytes())?;
            }
        }
    }
    // Drop scrollback of panes that are gone (or all of it when disabled).
    let keep: HashSet<String> = if cfg.scrollback { terminals.iter().map(|p| format!("{p}.txt")).collect() } else { HashSet::new() };
    if let Ok(rd) = std::fs::read_dir(&sbdir) {
        for e in rd.flatten() {
            if !keep.contains(&e.file_name().to_string_lossy().to_string()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    let json = serde_json::to_vec_pretty(&snap)?;
    if path.exists() {
        let _ = std::fs::copy(&path, dir().join("last.prev.json"));
    }
    write_atomic(&path, &json)?;
    Ok(path)
}

/// Periodic autosave so a crash or forced shutdown loses at most a few seconds.
pub fn autosave(app: AppHandle) {
    std::thread::Builder::new()
        .name("session-autosave".into())
        .spawn(move || loop {
            std::thread::sleep(AUTOSAVE);
            let st = app.state::<AppState>();
            if !st.session.dirty.load(Ordering::Relaxed) || !st.config.lock().config.session.restore {
                continue;
            }
            if let Err(e) = save(&app) {
                eprintln!("hivetty: session autosave failed: {e:#}");
            }
        })
        .expect("spawn autosave");
}

/// The command that resumes a recorded agent session, if the agent and id look sane.
pub fn resume_command(agent: &str, session_id: &str) -> Option<String> {
    let ok = !session_id.is_empty() && session_id.len() <= 128 && session_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return None;
    }
    match agent {
        "claude" => Some(format!("claude --resume {session_id}")),
        "codex" => Some(format!("codex resume {session_id}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_and_caps() {
        assert_eq!(raw_tail(b"aaaa
bbbb
cc".to_vec(), 9), "bbbb
cc");
        assert_eq!(raw_tail(b"one\ntwo".to_vec(), 100), "one\ntwo");
        assert_eq!(cap_lines("1\n2\n3\n4", 2), "3\n4");
        assert_eq!(cap_lines("1\n2", 5), "1\n2");
    }

    #[test]
    fn resume_commands() {
        assert_eq!(resume_command("claude", "4f1c-9a_b").as_deref(), Some("claude --resume 4f1c-9a_b"));
        assert_eq!(resume_command("codex", "abc").as_deref(), Some("codex resume abc"));
        assert_eq!(resume_command("claude", "x; rm -rf /"), None);
        assert_eq!(resume_command("other", "abc"), None);
    }
}
