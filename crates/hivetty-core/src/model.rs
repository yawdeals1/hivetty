//! Authoritative app state: workspaces -> surfaces (tabs) -> split tree of panes.
//! Pure data + operations; side effects (spawning PTYs, webviews) are done by the app
//! based on the ids these operations return.

use crate::layout::{Dir, Node, PaneId};
use crate::remote::Remote;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub type WorkspaceId = Uuid;
pub type SurfaceId = Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PaneKind {
    Terminal {
        profile: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    Browser {
        url: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub kind: PaneKind,
    pub title: Option<String>,
    pub cwd: Option<String>,
    /// `cwd` as a path Windows programs can open (WSL paths translated; None for ssh panes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_cwd: Option<String>,
    pub unread: bool,
    pub running: bool,
    pub last_exit: Option<i32>,
    #[serde(default)]
    pub exited: bool,
    /// Agent session recorded via hooks, used for resume (e.g. {"agent":"claude","session_id":"..."}).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSession>,
    /// Git info for the pane's cwd (filled by the metadata watcher).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitInfo>,
    /// TCP ports listened on by processes in this pane's process tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    /// Restored from a saved session with a recorded agent session that hasn't been resumed yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resumable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitInfo {
    pub root: String,
    /// Branch name, or short commit hash when detached.
    pub branch: String,
    pub detached: bool,
    pub dirty: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<PrInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// OPEN | CLOSED | MERGED
    pub state: String,
    pub draft: bool,
    /// passing | failing | pending | none
    pub checks: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub agent: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Surface {
    pub id: SurfaceId,
    pub title: Option<String>,
    pub root: Node,
    pub focused: PaneId,
    pub zoomed: Option<PaneId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub surfaces: Vec<Surface>,
    pub active_surface: SurfaceId,
    /// Set for workspaces opened with `hivetty ssh`: terminals, browser and uploads go to this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<Remote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: Uuid,
    pub pane: PaneId,
    pub workspace: WorkspaceId,
    pub title: Option<String>,
    pub body: String,
    /// Unix millis
    pub ts: u64,
    pub read: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Model {
    pub workspaces: Vec<Workspace>,
    pub active_workspace: Option<WorkspaceId>,
    pub panes: HashMap<PaneId, Pane>,
    pub notifications: Vec<Notification>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub ws: usize,
    pub surface: usize,
}

/// What closing a pane removed, so the app can tear down resources.
#[derive(Debug, Default, PartialEq)]
pub struct Closed {
    pub panes: Vec<PaneId>,
    pub surface: Option<SurfaceId>,
    pub workspace: Option<WorkspaceId>,
}

const MAX_NOTIFICATIONS: usize = 500;

impl Pane {
    pub fn new(kind: PaneKind) -> Self {
        let cwd = match &kind {
            PaneKind::Terminal { cwd, .. } => cwd.clone(),
            _ => None,
        };
        let mut p = Pane { id: Uuid::new_v4(), kind, title: None, cwd: None, local_cwd: None, unread: false, running: false, last_exit: None, exited: false, agent: None, git: None, ports: Vec::new(), resumable: false };
        p.set_cwd(cwd);
        p
    }

    pub fn profile(&self) -> Option<&str> {
        match &self.kind {
            PaneKind::Terminal { profile, .. } => Some(profile),
            PaneKind::Browser { .. } => None,
        }
    }

    /// Record the shell's reported cwd and derive the local path. Returns true if it changed.
    pub fn set_cwd(&mut self, cwd: Option<String>) -> bool {
        if self.cwd == cwd {
            return false;
        }
        self.local_cwd = match (self.profile(), cwd.as_deref()) {
            (Some(prof), Some(c)) => crate::paths::local_path(prof, c),
            _ => None,
        };
        self.cwd = cwd;
        true
    }
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    fn add_pane(&mut self, kind: PaneKind) -> PaneId {
        let p = Pane::new(kind);
        let id = p.id;
        self.panes.insert(id, p);
        id
    }

    fn new_surface_struct(&mut self, kind: PaneKind) -> (Surface, PaneId) {
        let pane = self.add_pane(kind);
        (Surface { id: Uuid::new_v4(), title: None, root: Node::leaf(pane), focused: pane, zoomed: None }, pane)
    }

    pub fn new_workspace(&mut self, name: Option<String>, kind: PaneKind) -> (WorkspaceId, PaneId) {
        let (surface, pane) = self.new_surface_struct(kind);
        let name = name.unwrap_or_else(|| format!("Workspace {}", self.workspaces.len() + 1));
        let ws = Workspace { id: Uuid::new_v4(), name, description: None, color: None, active_surface: surface.id, surfaces: vec![surface], remote: None };
        let id = ws.id;
        self.workspaces.push(ws);
        self.active_workspace = Some(id);
        (id, pane)
    }

    pub fn new_surface(&mut self, ws: WorkspaceId, kind: PaneKind) -> Option<(SurfaceId, PaneId)> {
        let idx = self.ws_index(ws)?;
        let (surface, pane) = self.new_surface_struct(kind);
        let sid = surface.id;
        let w = &mut self.workspaces[idx];
        let pos = w.surfaces.iter().position(|s| s.id == w.active_surface).map(|i| i + 1).unwrap_or(w.surfaces.len());
        w.surfaces.insert(pos, surface);
        w.active_surface = sid;
        Some((sid, pane))
    }

    /// Add a workspace holding existing panes (e.g. shells found running in the session host),
    /// one tab each. Does not change the active workspace unless there was none.
    pub fn adopt_workspace(&mut self, name: String, panes: Vec<(PaneId, PaneKind)>) -> Option<WorkspaceId> {
        let mut surfaces = Vec::new();
        for (id, kind) in panes {
            if self.panes.contains_key(&id) {
                continue;
            }
            let mut p = Pane::new(kind);
            p.id = id;
            self.panes.insert(id, p);
            surfaces.push(Surface { id: Uuid::new_v4(), title: None, root: Node::leaf(id), focused: id, zoomed: None });
        }
        let first = surfaces.first()?.id;
        let ws = Workspace { id: Uuid::new_v4(), name, description: None, color: None, active_surface: first, surfaces, remote: None };
        let id = ws.id;
        self.workspaces.push(ws);
        self.active_workspace.get_or_insert(id);
        Some(id)
    }

    pub fn workspace_of(&self, pane: PaneId) -> Option<&Workspace> {
        self.locate(pane).map(|l| &self.workspaces[l.ws])
    }

    pub fn ws_index(&self, ws: WorkspaceId) -> Option<usize> {
        self.workspaces.iter().position(|w| w.id == ws)
    }

    pub fn locate(&self, pane: PaneId) -> Option<Location> {
        for (wi, w) in self.workspaces.iter().enumerate() {
            for (si, s) in w.surfaces.iter().enumerate() {
                if s.root.contains(pane) {
                    return Some(Location { ws: wi, surface: si });
                }
            }
        }
        None
    }

    pub fn surface_of(&self, pane: PaneId) -> Option<&Surface> {
        self.locate(pane).map(|l| &self.workspaces[l.ws].surfaces[l.surface])
    }

    fn surface_mut(&mut self, loc: Location) -> &mut Surface {
        &mut self.workspaces[loc.ws].surfaces[loc.surface]
    }

    pub fn active_surface(&self) -> Option<&Surface> {
        let w = self.workspaces.iter().find(|w| Some(w.id) == self.active_workspace)?;
        w.surfaces.iter().find(|s| s.id == w.active_surface)
    }

    pub fn focused_pane(&self) -> Option<PaneId> {
        self.active_surface().map(|s| s.focused)
    }

    pub fn split(&mut self, target: PaneId, dir: Dir, kind: PaneKind) -> Option<PaneId> {
        let loc = self.locate(target)?;
        let new = self.add_pane(kind);
        let s = self.surface_mut(loc);
        s.root.split(target, dir, new);
        s.focused = new;
        s.zoomed = None;
        Some(new)
    }

    /// Close a pane; collapses empty surfaces and workspaces.
    pub fn close_pane(&mut self, pane: PaneId) -> Closed {
        let Some(loc) = self.locate(pane) else { return Closed::default() };
        let mut closed = Closed { panes: vec![pane], ..Default::default() };
        self.panes.remove(&pane);
        self.notifications.retain(|n| n.pane != pane);
        let s = self.surface_mut(loc);
        let root = std::mem::replace(&mut s.root, Node::leaf(pane));
        match root.remove(pane) {
            Some(rest) => {
                // focus the neighbor that took the space (first leaf of remaining tree as fallback)
                s.root = rest;
                if s.focused == pane {
                    s.focused = s.root.panes()[0];
                }
                if s.zoomed == Some(pane) {
                    s.zoomed = None;
                }
            }
            None => {
                let sid = s.id;
                closed.surface = Some(sid);
                let w = &mut self.workspaces[loc.ws];
                w.surfaces.remove(loc.surface);
                if w.surfaces.is_empty() {
                    closed.workspace = Some(w.id);
                    self.workspaces.remove(loc.ws);
                    if self.active_workspace == closed.workspace {
                        let i = loc.ws.min(self.workspaces.len().saturating_sub(1));
                        self.active_workspace = self.workspaces.get(i).map(|w| w.id);
                    }
                } else if w.active_surface == sid {
                    let i = loc.surface.min(w.surfaces.len() - 1);
                    w.active_surface = w.surfaces[i].id;
                }
            }
        }
        closed
    }

    pub fn close_surface(&mut self, ws: WorkspaceId, sid: SurfaceId) -> Closed {
        let panes = self
            .ws_index(ws)
            .and_then(|i| self.workspaces[i].surfaces.iter().find(|s| s.id == sid))
            .map(|s| s.root.panes())
            .unwrap_or_default();
        let mut all = Closed::default();
        for p in panes {
            let c = self.close_pane(p);
            all.panes.extend(c.panes);
            all.surface = all.surface.or(c.surface);
            all.workspace = all.workspace.or(c.workspace);
        }
        all
    }

    pub fn close_workspace(&mut self, ws: WorkspaceId) -> Closed {
        let sids: Vec<_> = self.ws_index(ws).map(|i| self.workspaces[i].surfaces.iter().map(|s| s.id).collect()).unwrap_or_default();
        let mut all = Closed::default();
        for sid in sids {
            let c = self.close_surface(ws, sid);
            all.panes.extend(c.panes);
            all.workspace = all.workspace.or(c.workspace);
        }
        all
    }

    /// Focus a pane anywhere: activates its workspace and surface and clears its unread state.
    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        let Some(loc) = self.locate(pane) else { return false };
        let w = &mut self.workspaces[loc.ws];
        self.active_workspace = Some(w.id);
        w.active_surface = w.surfaces[loc.surface].id;
        let s = &mut w.surfaces[loc.surface];
        s.focused = pane;
        if s.zoomed.is_some_and(|z| z != pane) {
            s.zoomed = None;
        }
        self.mark_pane_read(pane);
        true
    }

    pub fn focus_dir(&mut self, dir: Dir) -> Option<PaneId> {
        let s = self.active_surface()?;
        let next = s.root.neighbor(s.focused, dir)?;
        self.focus_pane(next);
        Some(next)
    }

    pub fn resize_focused(&mut self, dir: Dir, delta: f32) -> bool {
        let Some(p) = self.focused_pane() else { return false };
        let Some(loc) = self.locate(p) else { return false };
        self.surface_mut(loc).root.resize(p, dir, delta)
    }

    pub fn toggle_zoom(&mut self) {
        if let Some(p) = self.focused_pane() {
            if let Some(loc) = self.locate(p) {
                let s = self.surface_mut(loc);
                s.zoomed = if s.zoomed.is_some() { None } else { Some(p) };
            }
        }
    }

    pub fn activate_workspace(&mut self, ws: WorkspaceId) -> bool {
        if self.ws_index(ws).is_none() {
            return false;
        }
        self.active_workspace = Some(ws);
        if let Some(p) = self.focused_pane() {
            self.mark_pane_read(p);
        }
        true
    }

    pub fn activate_surface(&mut self, ws: WorkspaceId, sid: SurfaceId) -> bool {
        let Some(i) = self.ws_index(ws) else { return false };
        if !self.workspaces[i].surfaces.iter().any(|s| s.id == sid) {
            return false;
        }
        self.workspaces[i].active_surface = sid;
        self.active_workspace = Some(ws);
        if let Some(p) = self.focused_pane() {
            self.mark_pane_read(p);
        }
        true
    }

    /// Cycle workspaces (+1 / -1) with wraparound.
    pub fn cycle_workspace(&mut self, step: isize) {
        let n = self.workspaces.len() as isize;
        if n == 0 {
            return;
        }
        let cur = self.active_workspace.and_then(|w| self.ws_index(w)).unwrap_or(0) as isize;
        let id = self.workspaces[((cur + step).rem_euclid(n)) as usize].id;
        self.activate_workspace(id);
    }

    pub fn cycle_surface(&mut self, step: isize) {
        let Some(wid) = self.active_workspace else { return };
        let Some(i) = self.ws_index(wid) else { return };
        let w = &self.workspaces[i];
        let n = w.surfaces.len() as isize;
        let cur = w.surfaces.iter().position(|s| s.id == w.active_surface).unwrap_or(0) as isize;
        let sid = w.surfaces[((cur + step).rem_euclid(n)) as usize].id;
        self.activate_surface(wid, sid);
    }

    pub fn move_workspace(&mut self, ws: WorkspaceId, to: usize) {
        if let Some(i) = self.ws_index(ws) {
            let w = self.workspaces.remove(i);
            let to = to.min(self.workspaces.len());
            self.workspaces.insert(to, w);
        }
    }

    /// Record a notification. Marks pane unread unless it's the focused pane of the active surface
    /// and `app_focused` is true.
    pub fn notify(&mut self, pane: PaneId, title: Option<String>, body: String, ts: u64, app_focused: bool) -> Option<Notification> {
        let loc = self.locate(pane)?;
        let ws = self.workspaces[loc.ws].id;
        let visible = app_focused && self.focused_pane() == Some(pane);
        let n = Notification { id: Uuid::new_v4(), pane, workspace: ws, title, body, ts, read: visible };
        if !visible {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.unread = true;
            }
        }
        self.notifications.push(n.clone());
        if self.notifications.len() > MAX_NOTIFICATIONS {
            let overflow = self.notifications.len() - MAX_NOTIFICATIONS;
            self.notifications.drain(..overflow);
        }
        Some(n)
    }

    pub fn mark_pane_read(&mut self, pane: PaneId) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.unread = false;
        }
        for n in self.notifications.iter_mut().filter(|n| n.pane == pane) {
            n.read = true;
        }
    }

    pub fn mark_all_read(&mut self) {
        for p in self.panes.values_mut() {
            p.unread = false;
        }
        for n in &mut self.notifications {
            n.read = true;
        }
    }

    pub fn clear_notifications(&mut self) {
        self.notifications.clear();
        for p in self.panes.values_mut() {
            p.unread = false;
        }
    }

    pub fn unread_count(&self) -> usize {
        self.notifications.iter().filter(|n| !n.read).count()
    }

    /// Most recent unread notification's pane.
    pub fn latest_unread(&self) -> Option<PaneId> {
        self.notifications.iter().rev().find(|n| !n.read).map(|n| n.pane)
    }

    pub fn workspace_unread(&self, ws: WorkspaceId) -> usize {
        self.notifications.iter().filter(|n| n.workspace == ws && !n.read).count()
    }

    pub fn latest_notification(&self, ws: WorkspaceId) -> Option<&Notification> {
        self.notifications.iter().rev().find(|n| n.workspace == ws)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term() -> PaneKind {
        PaneKind::Terminal { profile: "pwsh".into(), cwd: None, command: None }
    }

    #[test]
    fn workspace_lifecycle() {
        let mut m = Model::new();
        let (w1, p1) = m.new_workspace(None, term());
        let p2 = m.split(p1, Dir::Right, term()).unwrap();
        assert_eq!(m.focused_pane(), Some(p2));
        let (_, p3) = m.new_surface(w1, term()).unwrap();
        assert_eq!(m.focused_pane(), Some(p3));
        let c = m.close_pane(p3);
        assert_eq!(c.panes, vec![p3]);
        assert!(c.surface.is_some() && c.workspace.is_none());
        assert_eq!(m.focused_pane(), Some(p2));
        m.close_pane(p2);
        assert_eq!(m.focused_pane(), Some(p1));
        let c = m.close_pane(p1);
        assert_eq!(c.workspace, Some(w1));
        assert!(m.workspaces.is_empty() && m.active_workspace.is_none() && m.panes.is_empty());
    }

    #[test]
    fn local_cwd_follows_profile() {
        let mut p = Pane::new(PaneKind::Terminal { profile: "wsl:Ubuntu".into(), cwd: Some("/mnt/c/dev".into()), command: None });
        assert_eq!(p.local_cwd.as_deref(), Some(r"C:\dev"));
        assert!(p.set_cwd(Some("/home/me".into())));
        assert_eq!(p.local_cwd.as_deref(), Some(r"\\wsl.localhost\Ubuntu\home\me"));
        assert!(!p.set_cwd(Some("/home/me".into())));
        let p = Pane::new(PaneKind::Terminal { profile: "ssh".into(), cwd: Some("/srv".into()), command: None });
        assert_eq!(p.local_cwd, None);
        let p = Pane::new(term());
        assert_eq!(p.local_cwd, None);
    }

    #[test]
    fn adopt_keeps_ids() {
        let mut m = Model::new();
        let (w1, _) = m.new_workspace(None, term());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let w = m.adopt_workspace("Recovered".into(), vec![(a, term()), (b, term())]).unwrap();
        assert_eq!(m.active_workspace, Some(w1));
        let ws = &m.workspaces[m.ws_index(w).unwrap()];
        assert_eq!(ws.surfaces.len(), 2);
        assert!(m.panes.contains_key(&a) && m.panes.contains_key(&b));
        assert_eq!(m.workspace_of(b).map(|w| w.id), Some(w));
        assert_eq!(m.adopt_workspace("x".into(), vec![(a, term())]), None);
    }

    #[test]
    fn notifications_and_unread() {
        let mut m = Model::new();
        let (w1, p1) = m.new_workspace(None, term());
        let (w2, p2) = m.new_workspace(None, term());
        // p2 is focused; notification to p2 while app focused => read
        m.notify(p2, None, "x".into(), 1, true).unwrap();
        assert_eq!(m.unread_count(), 0);
        m.notify(p1, Some("Claude".into()), "needs input".into(), 2, true).unwrap();
        assert_eq!(m.unread_count(), 1);
        assert!(m.panes[&p1].unread);
        assert_eq!(m.workspace_unread(w1), 1);
        assert_eq!(m.workspace_unread(w2), 0);
        assert_eq!(m.latest_unread(), Some(p1));
        m.focus_pane(p1);
        assert_eq!(m.active_workspace, Some(w1));
        assert_eq!(m.unread_count(), 0);
        assert!(!m.panes[&p1].unread);
    }

    #[test]
    fn focus_dir_and_cycle() {
        let mut m = Model::new();
        let (_, p1) = m.new_workspace(None, term());
        let p2 = m.split(p1, Dir::Down, term()).unwrap();
        assert_eq!(m.focus_dir(Dir::Up), Some(p1));
        assert_eq!(m.focus_dir(Dir::Down), Some(p2));
        let (w2, _) = m.new_workspace(None, term());
        m.cycle_workspace(1);
        assert_ne!(m.active_workspace, Some(w2));
        m.cycle_workspace(-1);
        assert_eq!(m.active_workspace, Some(w2));
    }

    #[test]
    fn close_workspace_returns_all_panes() {
        let mut m = Model::new();
        let (w, p1) = m.new_workspace(None, term());
        m.split(p1, Dir::Right, term());
        m.new_surface(w, term());
        let c = m.close_workspace(w);
        assert_eq!(c.panes.len(), 3);
        assert_eq!(c.workspace, Some(w));
    }
}
