//! Keep coding-agent notification hooks installed (Claude Code, Codex, Gemini CLI, Copilot CLI,
//! Cursor CLI, OpenCode — see `hivetty_core::hooks`), so agents that finish or need input
//! reach Hivetty (`hivetty notify --from-hook ...`) and mark their pane, tab and workspace.
//!
//! The install itself is the CLI's `hivetty hooks setup`: it is idempotent, backs up the agent's
//! config before changing it, never replaces a user's own Codex `notify` program, and records
//! the CLI's own path (`<install>\bin\hivetty.exe`), which stays valid across updates.

use crate::app::AppState;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use tauri::{AppHandle, Manager};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn ensure(app: &AppHandle) {
    if !app.state::<AppState>().config.lock().config.terminal.agent_hooks {
        return;
    }
    // Dev builds and isolated test runs (HIVETTY_HOME) must not point the user's real agent
    // configs at a throwaway target\debug\hivetty.exe.
    if cfg!(debug_assertions) || std::env::var_os("HIVETTY_HOME").is_some() {
        return;
    }
    let Some(cli) = crate::bundled::find("hivetty.exe") else { return };
    std::thread::Builder::new()
        .name("agent-hooks".into())
        .spawn(move || {
            let out = Command::new(&cli).args(["hooks", "setup"]).stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW).output();
            match out {
                Ok(o) if o.status.success() => {}
                Ok(o) => eprintln!("agent hooks setup failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
                Err(e) => eprintln!("agent hooks setup: {e}"),
            }
        })
        .ok();
}
