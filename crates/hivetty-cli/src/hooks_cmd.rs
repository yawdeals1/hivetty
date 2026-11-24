//! `hivetty hooks ...` and `hivetty notify --from-hook ...`.

use crate::client::Client;
use anyhow::{Context, Result};
use hivetty_core::hooks::{self, TomlOutcome};
use hivetty_core::protocol::ENV_PANE;
use serde_json::{json, Value};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn claude_settings() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude")).join("settings.json")
}

fn codex_config() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex")).join("config.toml")
}

/// Absolute path to this exe with forward slashes, so it works from bash and cmd alike.
fn exe_path() -> String {
    let p = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "hivetty".into());
    p.strip_prefix(r"\\?\").unwrap_or(&p).replace('\\', "/")
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

fn want(selector: Option<bool>, claude: bool) -> bool {
    selector.is_none_or(|c| c == claude)
}

pub fn setup(only_claude: Option<bool>) -> Result<()> {
    let exe = exe_path();
    if want(only_claude, true) {
        let path = claude_settings();
        if only_claude.is_some() || on_path("claude") || path.exists() {
            let mut s = read_json(&path)?;
            if hooks::claude_install(&mut s, &format!("\"{exe}\"")) {
                backup(&path)?;
                write_atomic(&path, &(serde_json::to_string_pretty(&s)? + "\n"))?;
                println!("Claude Code: hooks installed in {}", path.display());
            } else {
                println!("Claude Code: already set up");
            }
        } else {
            println!("Claude Code: not found, skipped (use --agent claude to force)");
        }
    }
    if want(only_claude, false) {
        let path = codex_config();
        if only_claude.is_some() || on_path("codex") || path.exists() {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let mut doc: toml_edit::DocumentMut = text.parse().with_context(|| format!("{} is not valid TOML; not modifying it", path.display()))?;
            match hooks::codex_install(&mut doc, &exe) {
                TomlOutcome::Installed => {
                    backup(&path)?;
                    write_atomic(&path, &doc.to_string())?;
                    println!("Codex: notify hook installed in {}", path.display());
                }
                TomlOutcome::AlreadyInstalled => println!("Codex: already set up"),
                TomlOutcome::Conflict(existing) => println!(
                    "Codex: {} already has `notify = {existing}`; left unchanged. Replace it with [\"{exe}\", \"notify\", \"--from-hook\", \"codex\"] to use Hivetty.",
                    path.display()
                ),
            }
        } else {
            println!("Codex: not found, skipped (use --agent codex to force)");
        }
    }
    Ok(())
}

pub fn remove(only_claude: Option<bool>) -> Result<()> {
    if want(only_claude, true) {
        let path = claude_settings();
        if path.is_file() {
            let mut s = read_json(&path)?;
            if hooks::claude_uninstall(&mut s) {
                backup(&path)?;
                write_atomic(&path, &(serde_json::to_string_pretty(&s)? + "\n"))?;
                println!("Claude Code: hooks removed");
            } else {
                println!("Claude Code: no Hivetty hooks found");
            }
        }
    }
    if want(only_claude, false) {
        let path = codex_config();
        if path.is_file() {
            let mut doc: toml_edit::DocumentMut = std::fs::read_to_string(&path)?.parse()?;
            if hooks::codex_uninstall(&mut doc) {
                backup(&path)?;
                write_atomic(&path, &doc.to_string())?;
                println!("Codex: notify hook removed");
            } else {
                println!("Codex: no Hivetty hook found");
            }
        }
    }
    Ok(())
}

pub fn status() -> Result<()> {
    let c = claude_settings();
    let claude = read_json(&c).map(|s| s.to_string().contains(hooks::MARKER)).unwrap_or(false);
    println!("Claude Code ({}): {}", c.display(), if claude { "installed" } else { "not installed" });
    let x = codex_config();
    let codex = std::fs::read_to_string(&x).map(|t| t.contains("--from-hook") && t.to_lowercase().contains("hivetty")).unwrap_or(false);
    println!("Codex ({}): {}", x.display(), if codex { "installed" } else { "not installed" });
    Ok(())
}

/// Hook entry point. Only acts inside a Hivetty pane; never blocks or fails the agent.
pub fn from_hook(claude: bool, pane: Option<String>) -> Result<()> {
    let Some(pane) = pane.or_else(|| std::env::var(ENV_PANE).ok()) else { return Ok(()) };
    let payload: Value = if claude {
        let mut s = String::new();
        if !std::io::stdin().is_terminal() {
            std::io::stdin().take(1 << 20).read_to_string(&mut s)?;
        }
        serde_json::from_str(&s).unwrap_or(Value::Null)
    } else {
        // Codex passes the JSON payload as the final argument.
        std::env::args().last().and_then(|a| serde_json::from_str(&a).ok()).unwrap_or(Value::Null)
    };
    let mut n = if claude { hooks::parse_claude(&payload) } else { hooks::parse_codex(&payload) };
    let is_stop = claude && payload.get("hook_event_name").and_then(Value::as_str) == Some("Stop");
    if is_stop {
        if let Some(text) = n.transcript_path.as_deref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| hooks::claude_last_assistant_text(&t)) {
            n.body = Some(text);
        }
    }
    let mut c = Client::connect_timeout(Duration::from_millis(800))?;
    if let Some(sid) = &n.session_id {
        let _ = c.call("agent.session", json!({ "pane": pane, "agent": if claude { "claude" } else { "codex" }, "session_id": sid }));
    }
    if let Some(body) = n.body {
        c.call("notify", json!({ "pane": pane, "title": n.title, "body": body }))?;
    }
    Ok(())
}
