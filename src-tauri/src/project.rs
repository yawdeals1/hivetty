//! Custom commands: the user config's `commands` plus any `hivetty.json` / `.hivetty.json`
//! found from the focused pane's cwd up to the drive root. They appear in the command palette
//! and can be run with `hivetty run <name>`.

use crate::app::{self, emit_state, AppState};
use hivetty_core::config::{self as cfg, CommandTarget, PROJECT_FILES};
use hivetty_core::layout::{Dir, PaneId};
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resolved {
    pub name: String,
    pub command: String,
    pub target: CommandTarget,
    /// Absolute working directory, if the command sets one.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub keybinding: Option<String>,
    /// Project file it came from; None for the user config.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Listing {
    pub commands: Vec<Resolved>,
    pub warnings: Vec<String>,
}

/// Project command files from `cwd` upwards, nearest first.
pub fn project_files(cwd: &Path) -> Vec<PathBuf> {
    let user = crate::config::config_path();
    let mut out = Vec::new();
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        for name in PROJECT_FILES {
            let p = d.join(name);
            if p.is_file() && p != user {
                out.push(p);
            }
        }
        dir = d.parent();
    }
    out
}

fn resolve(c: cfg::CustomCommand, base: Option<&Path>, source: Option<String>) -> Resolved {
    let cwd = c.cwd.map(|d| {
        let p = PathBuf::from(&d);
        match base {
            Some(b) if p.is_relative() => b.join(p).display().to_string(),
            _ => d,
        }
    });
    // keybindings from project files are ignored: a cloned repo shouldn't rebind your keys
    let keybinding = if source.is_none() { c.keybinding } else { None };
    Resolved { name: c.name, command: c.command, target: c.target, cwd, profile: c.profile, description: c.description, keybinding, source }
}

pub fn list_for(app: &AppHandle, cwd: Option<&str>) -> Listing {
    let st = app.state::<AppState>();
    let mut l = Listing::default();
    for c in st.config.lock().config.commands.clone() {
        l.commands.push(resolve(c, None, None));
    }
    let Some(cwd) = cwd.map(PathBuf::from).filter(|p| p.is_dir()) else { return l };
    for f in project_files(&cwd) {
        let src = f.display().to_string();
        match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|s| cfg::load_project(&s)) {
            Ok((p, warnings)) => {
                l.warnings.extend(warnings.into_iter().map(|w| format!("{src}: {w}")));
                for c in p.commands {
                    if c.name.trim().is_empty() || c.command.trim().is_empty() {
                        continue;
                    }
                    l.commands.push(resolve(c, f.parent(), Some(src.clone())));
                }
            }
            Err(e) => l.warnings.push(format!("{src}: {e}")),
        }
    }
    l
}

/// Commands available for a pane (default: the focused one).
pub fn list(app: &AppHandle, pane: Option<PaneId>) -> Listing {
    let st = app.state::<AppState>();
    let cwd = {
        let m = st.model.lock();
        pane.or(m.focused_pane()).and_then(|p| m.panes.get(&p)).and_then(|p| p.local_cwd.clone())
    };
    list_for(app, cwd.as_deref())
}

/// Run a command. Returns the pane it ran in.
pub fn run(app: &AppHandle, c: &Resolved, pane: Option<PaneId>) -> anyhow::Result<PaneId> {
    let st = app.state::<AppState>();
    let pane = pane.or_else(|| st.model.lock().focused_pane());
    let cwd = c.cwd.clone().or_else(|| app::cwd_of(app, pane));
    match c.target {
        CommandTarget::Current => {
            let p = pane.ok_or_else(|| anyhow::anyhow!("no focused pane"))?;
            let mut line = String::new();
            if let Some(d) = &c.cwd {
                line.push_str(&format!("cd \"{d}\"; "));
            }
            line.push_str(&c.command);
            line.push('\r');
            if !st.ptys.write(p, line.as_bytes()) {
                anyhow::bail!("the focused pane isn't a running terminal");
            }
            Ok(p)
        }
        CommandTarget::SplitRight | CommandTarget::SplitDown => {
            let dir = if c.target == CommandTarget::SplitDown { Dir::Down } else { Dir::Right };
            let kind = st.terminal_kind(c.profile.clone(), cwd, Some(c.command.clone()));
            app::act_split(app, pane, dir, Some(kind))
        }
        CommandTarget::Tab => {
            let ws = st.model.lock().active_workspace.ok_or_else(|| anyhow::anyhow!("no workspace"))?;
            let kind = st.terminal_kind(c.profile.clone(), cwd, Some(c.command.clone()));
            let (sid, p) = st.model.lock().new_surface(ws, kind).ok_or_else(|| anyhow::anyhow!("workspace not found"))?;
            {
                let mut m = st.model.lock();
                if let Some(s) = m.workspaces.iter_mut().flat_map(|w| w.surfaces.iter_mut()).find(|s| s.id == sid) {
                    s.title = Some(c.name.clone());
                }
            }
            app::start_pane(app, p)?;
            emit_state(app);
            Ok(p)
        }
        CommandTarget::Workspace => app::act_new_workspace(app, Some(c.name.clone()), c.profile.clone(), cwd, Some(c.command.clone())).map(|(_, p)| p),
    }
}

/// Quote a path for a command line if needed.
fn quote(s: &str) -> String {
    if s.contains([' ', '&', '(', ')', '^']) && !s.starts_with('"') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

pub fn fill_template(t: &str, file: &str, line: u64, col: u64) -> String {
    // a {file} already inside quotes in the template keeps them; otherwise we add them when needed
    let quoted_in_template = t.contains("\"{file}\"");
    t.replace("{file}", &if quoted_in_template { file.to_string() } else { quote(file) }).replace("{line}", &line.to_string()).replace("{col}", &col.to_string())
}

/// Open `path` at a line: the configured editor, else VS Code, else the system default app.
pub fn open_file_at(app: &AppHandle, path: &str, line: u64, col: u64, near: Option<PaneId>) -> anyhow::Result<()> {
    let st = app.state::<AppState>();
    let editor = st.config.lock().config.editor.clone();
    if let Some(e) = editor {
        let cmd = fill_template(&e.command, path, line, col);
        if e.terminal {
            let dir = Path::new(path).parent().map(|d| d.display().to_string());
            let kind = st.terminal_kind(None, dir, Some(cmd));
            app::act_split(app, near, Dir::Right, Some(kind))?;
        } else {
            std::process::Command::new("cmd").raw_arg(format!("/d /c {cmd}")).creation_flags(CREATE_NO_WINDOW).spawn()?;
        }
        return Ok(());
    }
    if which::which("code").is_ok() {
        std::process::Command::new("cmd")
            .raw_arg(format!("/d /c code -g \"{path}:{line}:{col}\""))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        return Ok(());
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(path, None::<&str>).map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates() {
        assert_eq!(fill_template("nvim +{line} {file}", r"C:\a b\x.rs", 3, 1), r#"nvim +3 "C:\a b\x.rs""#);
        assert_eq!(fill_template("hx \"{file}\":{line}:{col}", r"C:\a b\x.rs", 3, 7), r#"hx "C:\a b\x.rs":3:7"#);
        assert_eq!(fill_template("vim {file}", r"C:\x.rs", 1, 1), r"vim C:\x.rs");
    }

    #[test]
    fn finds_project_files_upwards() {
        let d = std::env::temp_dir().join(format!("hivetty-proj-{}", std::process::id()));
        let sub = d.join("pkg").join("src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(d.join("hivetty.json"), "{}").unwrap();
        std::fs::write(d.join("pkg").join(".hivetty.json"), "{}").unwrap();
        let found = project_files(&sub);
        assert_eq!(found[0], d.join("pkg").join(".hivetty.json"));
        assert_eq!(found[1], d.join("hivetty.json"));
        let r = resolve(
            cfg::CustomCommand { name: "t".into(), command: "x".into(), target: CommandTarget::Tab, cwd: Some("ui".into()), profile: None, description: None, keybinding: Some("ctrl+k".into()) },
            Some(&d),
            Some("p".into()),
        );
        assert_eq!(r.cwd.as_deref(), Some(d.join("ui").display().to_string().as_str()));
        assert_eq!(r.keybinding, None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
