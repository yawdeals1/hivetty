//! Non-destructive install/uninstall of Hivetty hooks into coding-agent configs, plus parsing
//! of the payloads those agents send to `hivetty notify --from-hook <agent>`.

use serde_json::{json, Value};

/// Marker substring identifying hook commands we own.
pub const MARKER: &str = "notify --from-hook";

pub const CLAUDE_EVENTS: &[&str] = &["Notification", "Stop", "SessionStart"];

fn is_ours(cmd: &Value) -> bool {
    cmd.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(MARKER) && c.to_lowercase().contains("hivetty"))
}

/// Merge Hivetty hooks into a Claude Code settings.json value. Returns true if anything changed.
/// Existing user hooks are preserved; running twice is a no-op.
pub fn claude_install(settings: &mut Value, exe_cmd: &str) -> bool {
    if !settings.is_object() {
        *settings = json!({});
    }
    let hooks = settings.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let mut changed = false;
    for ev in CLAUDE_EVENTS {
        let list = hooks.as_object_mut().unwrap().entry(*ev).or_insert_with(|| json!([]));
        if !list.is_array() {
            *list = json!([]);
        }
        let arr = list.as_array_mut().unwrap();
        let cmd = format!("{exe_cmd} notify --from-hook claude");
        let mut found = false;
        for group in arr.iter_mut() {
            if let Some(hs) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                for h in hs.iter_mut().filter(|h| is_ours(h)) {
                    found = true;
                    if h["command"] != json!(cmd) {
                        h["command"] = json!(cmd);
                        changed = true;
                    }
                }
            }
        }
        if !found {
            arr.push(json!({ "matcher": "", "hooks": [{ "type": "command", "command": cmd, "timeout": 10 }] }));
            changed = true;
        }
    }
    changed
}

/// Remove Hivetty hooks from Claude settings, dropping groups/events left empty.
pub fn claude_uninstall(settings: &mut Value) -> bool {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else { return false };
    let mut changed = false;
    for (_, list) in hooks.iter_mut() {
        let Some(arr) = list.as_array_mut() else { continue };
        for group in arr.iter_mut() {
            if let Some(hs) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = hs.len();
                hs.retain(|h| !is_ours(h));
                changed |= hs.len() != before;
            }
        }
        arr.retain(|g| g.get("hooks").and_then(Value::as_array).map(|h| !h.is_empty()).unwrap_or(true));
    }
    hooks.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
    if hooks.is_empty() {
        settings.as_object_mut().map(|o| o.remove("hooks"));
    }
    changed
}

#[derive(Debug, PartialEq)]
pub enum TomlOutcome {
    Installed,
    AlreadyInstalled,
    /// User already has a different `notify` program configured; we don't clobber it.
    Conflict(String),
}

/// Set `notify = [exe, "notify", "--from-hook", "codex"]` in a Codex config.toml document.
pub fn codex_install(doc: &mut toml_edit::DocumentMut, exe: &str) -> TomlOutcome {
    if let Some(existing) = doc.get("notify") {
        let text = existing.to_string();
        if text.contains("--from-hook") && text.to_lowercase().contains("hivetty") {
            return TomlOutcome::AlreadyInstalled;
        }
        return TomlOutcome::Conflict(text.trim().to_string());
    }
    let mut arr = toml_edit::Array::new();
    for s in [exe, "notify", "--from-hook", "codex"] {
        arr.push(s);
    }
    // top-level keys must come before tables; toml_edit handles placement for root items.
    doc.insert("notify", toml_edit::value(arr));
    TomlOutcome::Installed
}

pub fn codex_uninstall(doc: &mut toml_edit::DocumentMut) -> bool {
    let ours = doc.get("notify").map(|v| v.to_string()).is_some_and(|t| t.contains("--from-hook") && t.to_lowercase().contains("hivetty"));
    if ours {
        doc.remove("notify");
    }
    ours
}

/// A notification derived from an agent hook payload.
#[derive(Debug, Default, PartialEq)]
pub struct HookNotice {
    pub title: Option<String>,
    /// None = record-only event (e.g. SessionStart); don't show a notification.
    pub body: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
}

pub fn parse_claude(payload: &Value) -> HookNotice {
    let s = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let event = s("hook_event_name").unwrap_or_default();
    let body = match event.as_str() {
        "Notification" => s("message").or(Some("Claude needs your attention".into())),
        "Stop" | "SubagentStop" => Some("Finished — waiting for your input".into()),
        _ => None,
    };
    HookNotice { title: Some("Claude Code".into()), body, session_id: s("session_id"), cwd: s("cwd"), transcript_path: s("transcript_path") }
}

pub fn parse_codex(payload: &Value) -> HookNotice {
    let s = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let kind = s("type").unwrap_or_default();
    let body = match kind.as_str() {
        "agent-turn-complete" => Some(s("last-assistant-message").map(|m| truncate(&m, 240)).unwrap_or_else(|| "Turn complete".into())),
        "" => None,
        other => Some(other.replace('-', " ")),
    };
    HookNotice { title: Some("Codex".into()), body, session_id: s("thread-id").or(s("session_id")), cwd: s("cwd"), transcript_path: None }
}

/// Last assistant text message from a Claude Code transcript JSONL, for richer Stop notifications.
pub fn claude_last_assistant_text(jsonl: &str) -> Option<String> {
    for line in jsonl.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let content = v.pointer("/message/content")?;
        let text = match content {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            return Some(truncate(&text, 240));
        }
    }
    None
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
    t.push('…');
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_install_preserves_and_is_idempotent() {
        let mut s = json!({
            "model": "opus",
            "hooks": { "Stop": [{ "matcher": "", "hooks": [{ "type": "command", "command": "say done" }] }] }
        });
        assert!(claude_install(&mut s, "\"C:/x/hivetty.exe\""));
        let snapshot = s.clone();
        assert!(!claude_install(&mut s, "\"C:/x/hivetty.exe\""));
        assert_eq!(s, snapshot);
        assert_eq!(s["model"], "opus");
        let stop = s["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "say done");
        for ev in CLAUDE_EVENTS {
            assert!(s["hooks"][ev].to_string().contains("--from-hook claude"));
        }
        // path change updates in place
        assert!(claude_install(&mut s, "\"D:/y/hivetty.exe\""));
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn claude_uninstall_removes_only_ours() {
        let mut s = json!({ "hooks": { "Stop": [{ "matcher": "", "hooks": [{ "type": "command", "command": "say done" }] }] } });
        claude_install(&mut s, "hivetty");
        assert!(claude_uninstall(&mut s));
        assert_eq!(s, json!({ "hooks": { "Stop": [{ "matcher": "", "hooks": [{ "type": "command", "command": "say done" }] }] } }));
        let mut empty = json!({});
        claude_install(&mut empty, "hivetty");
        claude_uninstall(&mut empty);
        assert_eq!(empty, json!({}));
    }

    #[test]
    fn codex_toml() {
        let mut doc: toml_edit::DocumentMut = "model = \"o3\"\n\n[profiles.x]\nmodel = \"y\"\n".parse().unwrap();
        assert_eq!(codex_install(&mut doc, "C:/x/hivetty.exe"), TomlOutcome::Installed);
        assert_eq!(codex_install(&mut doc, "C:/x/hivetty.exe"), TomlOutcome::AlreadyInstalled);
        let out = doc.to_string();
        assert!(out.contains("model = \"o3\""));
        assert!(out.find("notify").unwrap() < out.find("[profiles.x]").unwrap());
        let reparsed: toml_edit::DocumentMut = out.parse().unwrap();
        assert_eq!(reparsed["profiles"]["x"]["model"].as_str(), Some("y"));
        assert!(codex_uninstall(&mut doc));
        let mut other: toml_edit::DocumentMut = "notify = [\"notify-send\"]\n".parse().unwrap();
        assert!(matches!(codex_install(&mut other, "h"), TomlOutcome::Conflict(_)));
    }

    #[test]
    fn payloads() {
        let n = parse_claude(&json!({"hook_event_name":"Notification","message":"Claude needs permission to use Bash","session_id":"abc"}));
        assert_eq!(n.body.as_deref(), Some("Claude needs permission to use Bash"));
        assert_eq!(n.session_id.as_deref(), Some("abc"));
        assert_eq!(parse_claude(&json!({"hook_event_name":"SessionStart","session_id":"abc"})).body, None);
        let c = parse_codex(&json!({"type":"agent-turn-complete","last-assistant-message":"Done refactoring"}));
        assert_eq!(c.body.as_deref(), Some("Done refactoring"));
        let t = "{\"type\":\"user\"}\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"All  tests\\npass.\"}]}}\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\"}]}}\n";
        assert_eq!(claude_last_assistant_text(t).as_deref(), Some("All tests pass."));
    }
}
