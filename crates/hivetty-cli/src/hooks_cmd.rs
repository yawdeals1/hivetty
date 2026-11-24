//! `hivetty hooks ...` and `hivetty notify --from-hook ...`.

use crate::client::Client;
use anyhow::{Context, Result};
use hivetty_core::hooks::{self, Agent};
use hivetty_core::protocol::ENV_PANE;
use serde_json::{json, Value};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn env_dir(var: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or(default)
}

fn claude_settings() -> PathBuf {
    env_dir("CLAUDE_CONFIG_DIR", home().join(".claude")).join("settings.json")
}

fn codex_dir() -> PathBuf {
    env_dir("CODEX_HOME", home().join(".codex"))
}

fn gemini_dir() -> PathBuf {
    home().join(".gemini")
}

fn copilot_dir() -> PathBuf {
    env_dir("COPILOT_HOME", home().join(".copilot"))
}

fn cursor_dir() -> PathBuf {
    home().join(".cursor")
}

fn opencode_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("opencode")
}

/// Where each agent's Hivetty hook lives.
fn hook_file(agent: Agent) -> PathBuf {
    match agent {
        Agent::Claude => claude_settings(),
        Agent::Codex => codex_dir().join("hooks.json"),
        Agent::Gemini => gemini_dir().join("settings.json"),
        Agent::Copilot => copilot_dir().join("hooks").join("hivetty.json"),
        Agent::Cursor => cursor_dir().join("hooks.json"),
        Agent::OpenCode => opencode_dir().join("plugins").join("hivetty.js"),
    }
}

/// Is the agent installed (its program on PATH or its config folder present)?
fn detected(agent: Agent) -> bool {
    let (bins, dir): (&[&str], PathBuf) = match agent {
        Agent::Claude => (&["claude"], claude_settings().parent().map(Path::to_path_buf).unwrap_or_default()),
        Agent::Codex => (&["codex"], codex_dir()),
        Agent::Gemini => (&["gemini"], gemini_dir()),
        Agent::Copilot => (&["copilot"], copilot_dir()),
        Agent::Cursor => (&["cursor-agent"], cursor_dir()),
        Agent::OpenCode => (&["opencode"], opencode_dir()),
    };
    bins.iter().any(|b| on_path(b)) || dir.is_dir()
}

/// Absolute path to this exe with forward slashes, so it works from bash and cmd alike.
fn exe_path() -> String {
    let p = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "hivetty".into());
    p.strip_prefix(r"\\?\").unwrap_or(&p).replace('\\', "/")
}

/// Program name for hooks an agent runs through a shell we can't predict (bash, cmd or
/// PowerShell): the bare name works in all of them, and Hivetty panes always have it on PATH.
/// Fall back to the quoted absolute path when this exe isn't the one PATH finds.
fn shell_program() -> String {
    let ours = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let on_path_dir = std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).find(|d| d.join("hivetty.exe").is_file()));
    match (ours, on_path_dir) {
        (Some(a), Some(b)) if same_dir(&a, &b) => "hivetty".into(),
        _ => format!("\"{}\"", exe_path()),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| [".exe", ".cmd", ".bat", ""].iter().any(|ext| d.join(format!("{bin}{ext}")).is_file())))
        .unwrap_or(false)
}

fn backup(path: &Path) -> Result<()> {
    if path.is_file() {
        let bak = path.with_extension(format!("{}.hivetty-bak", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
        std::fs::copy(path, &bak).with_context(|| format!("backing up {}", path.display()))?;
    }
    Ok(())
}

fn write_atomic(path: &Path, data: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("hivetty-tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn read_json(path: &Path) -> Result<Value> {
    if !path.is_file() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON; not modifying it", path.display()))
}

/// Edit a shared JSON config in place (backed up first), only writing when something changed.
fn edit_json(path: &Path, f: impl FnOnce(&mut Value) -> bool) -> Result<bool> {
    let mut v = read_json(path)?;
    if !f(&mut v) {
        return Ok(false);
    }
    backup(path)?;
    write_atomic(path, &(serde_json::to_string_pretty(&v)? + "\n"))?;
    Ok(true)
}

/// Write a file Hivetty owns outright, only when its content differs.
fn write_owned(path: &Path, content: &str) -> Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|t| t == content) {
        return Ok(false);
    }
    write_atomic(path, content)?;
    Ok(true)
}

fn install(agent: Agent) -> Result<bool> {
    let path = hook_file(agent);
    match agent {
        Agent::Claude => edit_json(&path, |s| hooks::claude_install(s, &format!("\"{}\"", exe_path()))),
        Agent::Gemini => edit_json(&path, |s| hooks::gemini_install(s, &shell_program())),
        Agent::Cursor => edit_json(&path, |s| hooks::cursor_install(s, &shell_program())),
        Agent::Codex => {
            // The old `notify = [...]` hook in config.toml would now report every turn twice.
            let legacy = codex_dir().join("config.toml");
            if let Ok(text) = std::fs::read_to_string(&legacy) {
                if let Ok(mut doc) = text.parse::<toml_edit::DocumentMut>() {
                    if hooks::codex_uninstall(&mut doc) {
                        backup(&legacy)?;
                        write_atomic(&legacy, &doc.to_string())?;
                    }
                }
            }
            edit_json(&path, |s| hooks::codex_hooks_install(s, &shell_program()))
        }
        Agent::Copilot => write_owned(&path, &(serde_json::to_string_pretty(&hooks::copilot_file(&exe_path()))? + "\n")),
        Agent::OpenCode => write_owned(&path, &hooks::opencode_plugin(&exe_path())),
    }
}

fn uninstall(agent: Agent) -> Result<bool> {
    let path = hook_file(agent);
    if !path.is_file() && agent != Agent::Codex {
        return Ok(false);
    }
    match agent {
        Agent::Claude | Agent::Gemini => edit_json(&path, hooks::nested_uninstall),
        Agent::Cursor => edit_json(&path, hooks::cursor_uninstall),
        Agent::Codex => {
            let mut changed = path.is_file() && edit_json(&path, hooks::nested_uninstall)?;
            let legacy = codex_dir().join("config.toml");
            if let Ok(text) = std::fs::read_to_string(&legacy) {
                let mut doc: toml_edit::DocumentMut = text.parse()?;
                if hooks::codex_uninstall(&mut doc) {
                    backup(&legacy)?;
                    write_atomic(&legacy, &doc.to_string())?;
                    changed = true;
                }
            }
            Ok(changed)
        }
        Agent::Copilot | Agent::OpenCode => {
            std::fs::remove_file(&path)?;
            Ok(true)
        }
    }
}

fn installed(agent: Agent) -> bool {
    std::fs::read_to_string(hook_file(agent)).is_ok_and(|t| t.contains("--from-hook") && t.to_lowercase().contains("hivetty"))
}

fn selected(only: Option<Agent>) -> Vec<Agent> {
    match only {
        Some(a) => vec![a],
        None => Agent::ALL.to_vec(),
    }
}

pub fn setup(only: Option<Agent>) -> Result<()> {
    let mut failed = false;
    for agent in selected(only) {
        if only.is_none() && !detected(agent) {
            println!("{}: not found, skipped (use --agent {} to force)", agent.label(), agent.id());
            continue;
        }
        match install(agent) {
            Ok(true) => {
                println!("{}: hooks installed in {}", agent.label(), hook_file(agent).display());
                if agent == Agent::Codex {
                    println!("  Codex asks you to trust new hooks once: open /hooks in Codex and trust the Hivetty hooks.");
                }
            }
            Ok(false) => println!("{}: already set up", agent.label()),
            Err(e) => {
                eprintln!("{}: {e:#}", agent.label());
                failed = true;
            }
        }
    }
    if failed {
        anyhow::bail!("some agents could not be set up");
    }
    Ok(())
}

pub fn remove(only: Option<Agent>) -> Result<()> {
    for agent in selected(only) {
        match uninstall(agent) {
            Ok(true) => println!("{}: hooks removed", agent.label()),
            Ok(false) => {
                if only.is_some() {
                    println!("{}: no Hivetty hooks found", agent.label());
                }
            }
            Err(e) => eprintln!("{}: {e:#}", agent.label()),
        }
    }
    Ok(())
}

pub fn status() -> Result<()> {
    for agent in Agent::ALL {
        let state = if installed(agent) {
            "installed"
        } else if detected(agent) {
            "not installed"
        } else {
            "not found"
        };
        println!("{} ({}): {state}", agent.label(), hook_file(agent).display());
    }
    Ok(())
}

/// Read a hook payload from stdin without ever hanging the agent (some agents leave stdin open).
fn read_stdin(limit: u64, wait: Duration) -> String {
    if std::io::stdin().is_terminal() {
        return String::new();
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::stdin().take(limit).read_to_string(&mut s);
        let _ = tx.send(s);
    });
    rx.recv_timeout(wait).unwrap_or_default()
}

/// Hook entry point. Only acts inside a Hivetty pane; never blocks or fails the agent.
pub fn from_hook(agent: Agent, pane: Option<String>) -> Result<()> {
    let Some(pane) = pane.or_else(|| std::env::var(ENV_PANE).ok()) else { return Ok(()) };
    // Codex's legacy `notify` program gets the payload as its last argument; everything else
    // (including Codex hooks.json) pipes it on stdin.
    let arg_payload = std::env::args().last().and_then(|a| serde_json::from_str::<Value>(&a).ok()).filter(Value::is_object);
    let payload = arg_payload.unwrap_or_else(|| serde_json::from_str(&read_stdin(1 << 20, Duration::from_secs(3))).unwrap_or(Value::Null));
    let mut n = hooks::parse(agent, &payload);
    if agent == Agent::Claude && payload.get("hook_event_name").and_then(Value::as_str) == Some("Stop") {
        if let Some(text) = n.transcript_path.as_deref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| hooks::claude_last_assistant_text(&t)) {
            n.body = Some(text);
        }
    }
    let mut c = Client::connect_timeout(Duration::from_millis(800))?;
    // Only agents Hivetty can resume are recorded.
    if let (Some(sid), Agent::Claude | Agent::Codex) = (&n.session_id, agent) {
        let _ = c.call("agent.session", json!({ "pane": pane, "agent": agent.id(), "session_id": sid }));
    }
    if let Some(body) = n.body {
        c.call("notify", json!({ "pane": pane, "title": n.title, "body": body }))?;
    }
    Ok(())
}
