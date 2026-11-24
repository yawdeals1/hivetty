//! Session snapshots: what Hivetty saves on exit (and periodically) so the next launch can
//! reopen the same workspaces, tabs, splits, working directories and browser URLs.
//!
//! Commands that started a pane are deliberately *not* saved or re-run (they may hold secrets
//! and re-running arbitrary commands on launch is surprising). Agent sessions recorded by hooks
//! are kept so the user can resume them.

use crate::layout::PaneId;
use crate::model::{AgentSession, Model, Pane, PaneKind, Workspace, WorkspaceId};
use crate::redact::redact;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const SESSION_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFile {
    pub version: u32,
    /// Unix millis
    pub saved_at: u64,
    pub active_workspace: Option<WorkspaceId>,
    pub workspaces: Vec<Workspace>,
    pub panes: Vec<PaneSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub id: PaneId,
    pub kind: PaneKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSession>,
}

/// Build a snapshot of the model, redacting anything secret-looking.
pub fn snapshot(m: &Model, now_ms: u64) -> SessionFile {
    let mut panes = Vec::new();
    for w in &m.workspaces {
        for s in &w.surfaces {
            for id in s.root.panes() {
                let Some(p) = m.panes.get(&id) else { continue };
                let kind = match &p.kind {
                    PaneKind::Terminal { profile, cwd, .. } => PaneKind::Terminal { profile: profile.clone(), cwd: p.cwd.clone().or_else(|| cwd.clone()), command: None },
                    PaneKind::Browser { url } => PaneKind::Browser { url: redact(url) },
                };
                panes.push(PaneSnapshot { id, kind, title: p.title.as_deref().map(redact), agent: p.agent.clone() });
            }
        }
    }
    let workspaces = m
        .workspaces
        .iter()
        .cloned()
        .map(|mut w| {
            w.name = redact(&w.name);
            w.description = w.description.as_deref().map(redact);
            for s in &mut w.surfaces {
                s.title = s.title.as_deref().map(redact);
            }
            w
        })
        .collect();
    SessionFile { version: SESSION_VERSION, saved_at: now_ms, active_workspace: m.active_workspace, workspaces, panes }
}

/// Rebuild a model from a snapshot. Tolerates damaged files: leaves without a pane record are
/// dropped, empty tabs/workspaces are removed and focus is repaired.
pub fn restore(f: SessionFile) -> Result<Model, String> {
    if f.version > SESSION_VERSION {
        return Err(format!("session file version {} is newer than this Hivetty", f.version));
    }
    let snaps: HashMap<PaneId, PaneSnapshot> = f.panes.into_iter().map(|p| (p.id, p)).collect();
    let mut m = Model::new();
    for mut w in f.workspaces {
        w.surfaces.retain_mut(|s| {
            let mut root = Some(s.root.clone());
            for id in s.root.panes() {
                if !snaps.contains_key(&id) {
                    root = root.and_then(|r| r.remove(id));
                }
            }
            let Some(root) = root else { return false };
            // A pane id may only appear once across the whole session.
            if root.panes().iter().any(|p| m.panes.contains_key(p)) {
                return false;
            }
            s.root = root;
            let panes = s.root.panes();
            if !panes.contains(&s.focused) {
                s.focused = panes[0];
            }
            if s.zoomed.is_some_and(|z| !panes.contains(&z)) {
                s.zoomed = None;
            }
            for id in panes {
                let snap = &snaps[&id];
                let mut p = Pane::new(snap.kind.clone());
                p.id = id;
                p.title = snap.title.clone();
                p.agent = snap.agent.clone();
                p.resumable = snap.agent.is_some();
                m.panes.insert(id, p);
            }
            true
        });
        if w.surfaces.is_empty() {
            continue;
        }
        if !w.surfaces.iter().any(|s| s.id == w.active_surface) {
            w.active_surface = w.surfaces[0].id;
        }
        m.workspaces.push(w);
    }
    m.active_workspace = f.active_workspace.filter(|a| m.ws_index(*a).is_some()).or_else(|| m.workspaces.first().map(|w| w.id));
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Dir, Node};

    fn term(cmd: Option<&str>) -> PaneKind {
        PaneKind::Terminal { profile: "pwsh".into(), cwd: Some(r"C:\start".into()), command: cmd.map(str::to_string) }
    }

    #[test]
    fn roundtrip_layout_and_state() {
        let mut m = Model::new();
        let (w1, p1) = m.new_workspace(Some("api".into()), term(Some("GITHUB_TOKEN=abc npm run dev")));
        let p2 = m.split(p1, Dir::Right, PaneKind::Browser { url: "http://localhost:3000/".into() }).unwrap();
        let p3 = m.split(p2, Dir::Down, term(None)).unwrap();
        m.panes.get_mut(&p1).unwrap().cwd = Some(r"C:\dev\api".into());
        m.panes.get_mut(&p3).unwrap().agent = Some(AgentSession { agent: "claude".into(), session_id: "abc-123".into() });
        m.panes.get_mut(&p3).unwrap().title = Some("export API_KEY=zzz".into());
        let (w2, _) = m.new_workspace(None, term(None));
        m.new_surface(w2, term(None));
        m.activate_workspace(w1);
        m.notify(p1, None, "hi".into(), 1, false);

        let json = serde_json::to_string(&snapshot(&m, 42)).unwrap();
        assert!(!json.contains("abc npm"), "commands are not saved");
        assert!(!json.contains("zzz"), "titles are redacted");
        let back = restore(serde_json::from_str(&json).unwrap()).unwrap();

        assert_eq!(back.workspaces.len(), 2);
        assert_eq!(back.active_workspace, Some(w1));
        assert_eq!(back.workspaces[0].name, "api");
        assert_eq!(back.workspaces[1].surfaces.len(), 2);
        let s = &back.workspaces[0].surfaces[0];
        assert_eq!(s.root, m.workspaces[0].surfaces[0].root, "split tree and ratios survive");
        assert_eq!(s.focused, p3);
        match &back.panes[&p1].kind {
            PaneKind::Terminal { cwd, command, .. } => {
                assert_eq!(cwd.as_deref(), Some(r"C:\dev\api"), "latest cwd wins");
                assert!(command.is_none());
            }
            _ => panic!(),
        }
        assert!(matches!(&back.panes[&p2].kind, PaneKind::Browser { url } if url == "http://localhost:3000/"));
        assert!(back.panes[&p3].resumable);
        assert!(!back.panes[&p1].resumable);
        assert!(back.notifications.is_empty());
        assert!(back.panes.values().all(|p| !p.unread && !p.running && !p.exited));
    }

    #[test]
    fn repairs_damaged_files() {
        let mut m = Model::new();
        let (_, p1) = m.new_workspace(None, term(None));
        let p2 = m.split(p1, Dir::Right, term(None)).unwrap();
        m.new_workspace(None, term(None));
        let mut f = snapshot(&m, 0);
        // lose the focused pane of workspace 1 and every pane of workspace 2
        let lost: Vec<PaneId> = vec![p2, m.workspaces[1].surfaces[0].focused];
        f.panes.retain(|p| !lost.contains(&p.id));
        f.active_workspace = Some(m.workspaces[1].id);
        let back = restore(f).unwrap();
        assert_eq!(back.workspaces.len(), 1);
        assert_eq!(back.workspaces[0].surfaces[0].root, Node::leaf(p1));
        assert_eq!(back.workspaces[0].surfaces[0].focused, p1);
        assert_eq!(back.active_workspace, Some(back.workspaces[0].id));
        assert_eq!(back.panes.len(), 1);
    }

    #[test]
    fn rejects_future_versions() {
        let mut f = snapshot(&Model::new(), 0);
        f.version = SESSION_VERSION + 1;
        assert!(restore(f).is_err());
    }
}
