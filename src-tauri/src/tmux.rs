//! Support for the tmux compatibility shim (`tmux.exe`, built from hivetty-cli). Terminals get
//! `$TMUX`/`$TMUX_PANE` and the shim on PATH; the shim translates tmux commands into pipe API
//! calls, using the numbers kept here (`%3` pane, `@2` window = tab, `$1` session = workspace).

use crate::app::AppState;
use hivetty_core::layout::{PaneId, Rect};
use hivetty_core::model::PaneKind;
use parking_lot::{Condvar, Mutex};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

#[derive(Default)]
pub struct Nums {
    panes: HashMap<Uuid, u32>,
    surfaces: HashMap<Uuid, u32>,
    workspaces: HashMap<Uuid, u32>,
    next_pane: u32,
    next_surface: u32,
    next_ws: u32,
}

fn num_of(map: &mut HashMap<Uuid, u32>, next: &mut u32, id: Uuid) -> u32 {
    *map.entry(id).or_insert_with(|| {
        *next += 1;
        *next
    })
}

impl Nums {
    pub fn pane(&mut self, id: Uuid) -> u32 {
        num_of(&mut self.panes, &mut self.next_pane, id)
    }
    pub fn surface(&mut self, id: Uuid) -> u32 {
        num_of(&mut self.surfaces, &mut self.next_surface, id)
    }
    pub fn workspace(&mut self, id: Uuid) -> u32 {
        num_of(&mut self.workspaces, &mut self.next_ws, id)
    }
}

static SHIM_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// A folder holding only a copy of `tmux.exe` (so putting it first on PATH shadows nothing else).
pub fn shim_dir() -> Option<PathBuf> {
    SHIM_DIR
        .get_or_init(|| crate::bundled::installed_copy("tmux.exe", "shim").and_then(|p| p.parent().map(PathBuf::from)))
        .clone()
}

/// Environment that makes a local terminal look like it's inside tmux.
pub fn pane_env(app: &AppHandle, pane: PaneId) -> Vec<(String, String)> {
    let st = app.state::<AppState>();
    if !st.config.lock().config.terminal.tmux_shim {
        return vec![];
    }
    let Some(dir) = shim_dir() else { return vec![] };
    let n = st.nums.lock().pane(pane);
    let path = crate::bundled::pane_path();
    vec![
        ("TMUX".into(), format!("{},{},0", st.pipe_name, std::process::id())),
        ("TMUX_PANE".into(), format!("%{n}")),
        ("PATH".into(), format!("{};{path}", dir.display())),
    ]
}

/// Everything the shim needs to resolve targets and expand formats, in one call.
pub fn state(app: &AppHandle) -> Value {
    let st = app.state::<AppState>();
    let m = st.model.lock();
    let mut nums = st.nums.lock();
    let focused = m.focused_pane();
    let workspaces: Vec<Value> = m
        .workspaces
        .iter()
        .enumerate()
        .map(|(wi, w)| {
            let surfaces: Vec<Value> = w
                .surfaces
                .iter()
                .enumerate()
                .map(|(si, s)| {
                    let rects: HashMap<PaneId, Rect> = s.root.rects(Rect::UNIT).into_iter().collect();
                    let mut win = (0u32, 0u32);
                    let panes: Vec<Value> = s
                        .root
                        .panes()
                        .iter()
                        .enumerate()
                        .filter_map(|(pi, id)| {
                            let p = m.panes.get(id)?;
                            let (rows, cols) = st.ptys.size(*id).unwrap_or((24, 80));
                            if let Some(r) = rects.get(id) {
                                win.0 = win.0.max((cols as f32 / r.w.max(0.05)).round() as u32);
                                win.1 = win.1.max((rows as f32 / r.h.max(0.05)).round() as u32);
                            }
                            let (profile, command) = match &p.kind {
                                PaneKind::Terminal { profile, command, .. } => (profile.clone(), command.clone()),
                                PaneKind::Browser { url } => ("browser".into(), Some(url.clone())),
                            };
                            Some(json!({
                                "id": id,
                                "num": nums.pane(*id),
                                "index": pi,
                                "active": s.focused == *id,
                                "title": p.title,
                                "cwd": p.cwd,
                                "pid": st.ptys.pid(*id),
                                "width": cols,
                                "height": rows,
                                "left": rects.get(id).map(|r| r.x).unwrap_or(0.0),
                                "top": rects.get(id).map(|r| r.y).unwrap_or(0.0),
                                "dead": p.exited,
                                "profile": profile,
                                "command": command,
                                "browser": matches!(p.kind, PaneKind::Browser { .. }),
                            }))
                        })
                        .collect();
                    json!({
                        "id": s.id,
                        "num": nums.surface(s.id),
                        "index": si,
                        "title": s.title,
                        "active": s.id == w.active_surface,
                        "zoomed": s.zoomed.is_some(),
                        "width": win.0.max(1),
                        "height": win.1.max(1),
                        "panes": panes,
                    })
                })
                .collect();
            json!({
                "id": w.id,
                "num": nums.workspace(w.id),
                "index": wi,
                "name": w.name,
                "active": m.active_workspace == Some(w.id),
                "surfaces": surfaces,
            })
        })
        .collect();
    json!({ "focused": focused, "pid": std::process::id(), "workspaces": workspaces })
}

#[derive(Default)]
struct Chan {
    generation: u64,
    pending: bool,
    waiters: usize,
    locked: bool,
}

/// `tmux wait-for` channels.
#[derive(Default)]
pub struct WaitChannels {
    chans: Mutex<HashMap<String, Chan>>,
    cv: Condvar,
}

impl WaitChannels {
    pub fn signal(&self, name: &str) {
        let mut m = self.chans.lock();
        let c = m.entry(name.to_string()).or_default();
        if c.waiters > 0 {
            c.generation += 1;
        } else {
            c.pending = true;
        }
        self.cv.notify_all();
    }

    pub fn wait(&self, name: &str) {
        let mut m = self.chans.lock();
        let c = m.entry(name.to_string()).or_default();
        if c.pending {
            c.pending = false;
            return;
        }
        c.waiters += 1;
        let g = c.generation;
        while m.get(name).is_some_and(|c| c.generation == g) {
            self.cv.wait(&mut m);
        }
        if let Some(c) = m.get_mut(name) {
            c.waiters -= 1;
        }
    }

    pub fn lock(&self, name: &str) {
        let mut m = self.chans.lock();
        while m.get(name).is_some_and(|c| c.locked) {
            self.cv.wait(&mut m);
        }
        m.entry(name.to_string()).or_default().locked = true;
    }

    pub fn unlock(&self, name: &str) {
        let mut m = self.chans.lock();
        if let Some(c) = m.get_mut(name) {
            c.locked = false;
        }
        self.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn numbers_are_stable_and_per_kind() {
        let mut n = Nums::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(n.pane(a), 1);
        assert_eq!(n.pane(b), 2);
        assert_eq!(n.pane(a), 1);
        assert_eq!(n.surface(a), 1);
        assert_eq!(n.workspace(b), 1);
    }

    #[test]
    fn wait_for_signal_before_and_after() {
        let w = Arc::new(WaitChannels::default());
        w.signal("x"); // stored: the next wait returns at once
        w.wait("x");
        let w2 = w.clone();
        let t = std::thread::spawn(move || w2.wait("y"));
        while w.chans.lock().get("y").map_or(0, |c| c.waiters) == 0 {
            std::thread::yield_now();
        }
        w.signal("y");
        t.join().unwrap();
        w.lock("l");
        let w3 = w.clone();
        let t = std::thread::spawn(move || {
            w3.lock("l");
            w3.unlock("l");
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        w.unlock("l");
        t.join().unwrap();
    }
}
