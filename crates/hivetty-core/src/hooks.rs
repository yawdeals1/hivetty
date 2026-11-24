//! Non-destructive install/uninstall of Hivetty hooks into coding-agent configs, plus parsing
//! of the payloads those agents send to `hivetty notify --from-hook <agent>`.
//!
//! Every agent ends up calling the same command when it finishes a turn or needs the user, and
//! the pane it runs in (`HIVETTY_PANE_ID`) is marked for attention. Outside a Hivetty pane the
//! command does nothing, so a hook in a global agent config is harmless elsewhere.

use serde_json::{json, Map, Value};

/// Marker substring identifying hook commands we own.
pub const MARKER: &str = "notify --from-hook";

/// Coding agents Hivetty knows how to hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Gemini,
    Copilot,
    Cursor,
    OpenCode,
}

impl Agent {
    pub const ALL: [Agent; 6] = [Agent::Claude, Agent::Codex, Agent::Gemini, Agent::Copilot, Agent::Cursor, Agent::OpenCode];

    /// The `--from-hook` value.
    pub fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Gemini => "gemini",
            Agent::Copilot => "copilot",
            Agent::Cursor => "cursor",
            Agent::OpenCode => "opencode",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Gemini => "Gemini CLI",
            Agent::Copilot => "Copilot CLI",
            Agent::Cursor => "Cursor CLI",
            Agent::OpenCode => "OpenCode",
        }
    }

    pub fn from_id(id: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|a| a.id() == id)
    }
}

pub const CLAUDE_EVENTS: &[&str] = &["Notification", "Stop", "SessionStart"];
pub const GEMINI_EVENTS: &[&str] = &["AfterAgent", "Notification"];
pub const CODEX_EVENTS: &[&str] = &["Stop", "PermissionRequest"];
pub const CURSOR_EVENTS: &[&str] = &["stop"];

fn is_ours(cmd: &Value) -> bool {
    let text = match cmd.get("command").and_then(Value::as_str) {
        Some(c) => c.to_string(),
        // Copilot's `exec` + `args` form
        None => cmd.to_string(),
    };
    text.contains("--from-hook") && text.to_lowercase().contains("hivetty")
}

/// `hivetty notify --from-hook <agent>` with the given program (already quoted if needed).
pub fn hook_command(program: &str, agent: Agent) -> String {
    format!("{program} {MARKER} {}", agent.id())
}

fn object(v: &mut Value) -> &mut Map<String, Value> {
    if !v.is_object() {
        *v = json!({});
    }
    v.as_object_mut().unwrap()
}

fn array<'a>(m: &'a mut Map<String, Value>, key: &str) -> &'a mut Vec<Value> {
    let v = m.entry(key).or_insert_with(|| json!([]));
    if !v.is_array() {
        *v = json!([]);
    }
    v.as_array_mut().unwrap()
}

/// Merge our hook into the nested `hooks.<Event>[] = { matcher?, hooks: [entry] }` layout shared by
/// Claude Code, Codex (`hooks.json`) and Gemini CLI. Existing user hooks are kept; an existing
/// Hivetty hook is updated in place. Returns true if anything changed.
pub fn nested_install(settings: &mut Value, events: &[&str], matcher: Option<&str>, entry: &Value) -> bool {
    let hooks = object(object(settings).entry("hooks").or_insert_with(|| json!({})));
    let mut changed = false;
    for ev in events {
        let arr = array(hooks, ev);
        let mut found = false;
        for group in arr.iter_mut() {
            if let Some(hs) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                for h in hs.iter_mut().filter(|h| is_ours(h)) {
                    found = true;
                    if h.get("command") != entry.get("command") {
                        *h = entry.clone();
                        changed = true;
                    }
                }
            }
        }
        if !found {
            let mut group = json!({ "hooks": [entry.clone()] });
            if let Some(m) = matcher {
                group["matcher"] = json!(m);
            }
            arr.push(group);
            changed = true;
        }
    }
    changed
}

/// Remove our hooks from the nested layout, dropping groups/events left empty.
pub fn nested_uninstall(settings: &mut Value) -> bool {
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

/// Merge Hivetty hooks into a Claude Code settings.json value. Returns true if anything changed.
pub fn claude_install(settings: &mut Value, exe_cmd: &str) -> bool {
    let entry = json!({ "type": "command", "command": hook_command(exe_cmd, Agent::Claude), "timeout": 10 });
    nested_install(settings, CLAUDE_EVENTS, Some(""), &entry)
}

pub fn claude_uninstall(settings: &mut Value) -> bool {
    nested_uninstall(settings)
}

/// Gemini CLI `settings.json` (timeouts in milliseconds; stdout must be JSON).
pub fn gemini_install(settings: &mut Value, program: &str) -> bool {
    let entry = json!({ "name": "hivetty", "type": "command", "command": hook_command(program, Agent::Gemini), "timeout": 10000 });
    nested_install(settings, GEMINI_EVENTS, Some("*"), &entry)
}

/// Codex `hooks.json` (timeouts in seconds; Codex asks the user to trust new hooks in `/hooks`).
pub fn codex_hooks_install(hooks_json: &mut Value, program: &str) -> bool {
    let entry = json!({ "type": "command", "command": hook_command(program, Agent::Codex), "timeout": 10 });
    nested_install(hooks_json, CODEX_EVENTS, None, &entry)
}

/// Cursor `hooks.json`: `{ "version": 1, "hooks": { "stop": [ { "command": ... } ] } }`.
pub fn cursor_install(hooks_json: &mut Value, program: &str) -> bool {
    let root = object(hooks_json);
    let mut changed = false;
    if !root.contains_key("version") {
        root.insert("version".into(), json!(1));
        changed = true;
    }
    let entry = json!({ "command": hook_command(program, Agent::Cursor), "timeout": 10 });
    let hooks = object(root.entry("hooks").or_insert_with(|| json!({})));
    for ev in CURSOR_EVENTS {
        let arr = array(hooks, ev);
        match arr.iter_mut().find(|h| is_ours(h)) {
            Some(h) if h.get("command") != entry.get("command") => {
                *h = entry.clone();
                changed = true;
            }
            Some(_) => {}
            None => {
                arr.push(entry.clone());
                changed = true;
            }
        }
    }
    changed
}

pub fn cursor_uninstall(hooks_json: &mut Value) -> bool {
    let Some(hooks) = hooks_json.get_mut("hooks").and_then(Value::as_object_mut) else { return false };
    let mut changed = false;
    for (_, list) in hooks.iter_mut() {
        if let Some(arr) = list.as_array_mut() {
            let before = arr.len();
            arr.retain(|h| !is_ours(h));
            changed |= arr.len() != before;
        }
    }
    hooks.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
    changed
}

/// The whole `~/.copilot/hooks/hivetty.json` file (Copilot CLI loads every file in that folder).
/// `exec` runs the program directly, so the payload reaches it on stdin with no shell in between.
pub fn copilot_file(exe: &str) -> Value {
    let entry = |matcher: Option<&str>| {
        let mut e = json!({ "type": "command", "exec": exe, "args": ["notify", "--from-hook", "copilot"], "timeoutSec": 10 });
        if let Some(m) = matcher {
            e["matcher"] = json!(m);
        }
        e
    };
    json!({
        "version": 1,
        "hooks": {
            "agentStop": [entry(None)],
            // finished turns already come from agentStop; only forward "needs you" notifications
            "notification": [entry(Some("permission_prompt|elicitation_dialog"))],
        }
    })
}

/// OpenCode plugin (`~/.config/opencode/plugins/hivetty.js`).
pub fn opencode_plugin(exe: &str) -> String {
    let exe = serde_json::to_string(exe).unwrap_or_else(|_| "\"hivetty\"".into());
    format!(
        r#"// Installed by Hivetty (`hivetty hooks setup`); removed by `hivetty hooks remove`.
// Tells the Hivetty pane OpenCode runs in when a session finishes or needs approval.
import {{ spawn }} from "node:child_process";

const HIVETTY = {exe};
const EVENTS = new Set(["session.idle", "permission.asked", "permission.updated"]);

export const HivettyNotify = async () => ({{
  event: async ({{ event }}) => {{
    if (!process.env.HIVETTY_PANE_ID || !EVENTS.has(event?.type)) return;
    try {{
      const p = spawn(HIVETTY, ["notify", "--from-hook", "opencode"], {{ stdio: ["pipe", "ignore", "ignore"], windowsHide: true }});
      p.on("error", () => {{}});
      p.stdin.end(JSON.stringify({{ type: event.type, sessionID: event.properties?.sessionID ?? null }}));
    }} catch {{}}
  }},
}});
"#
    )
}

/// Legacy Codex hook (`notify = [...]` in config.toml); replaced by `hooks.json`.
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

const FINISHED: &str = "Finished — waiting for your input";

fn str_of(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| payload.get(*k).and_then(Value::as_str)).map(str::to_string).filter(|s| !s.trim().is_empty())
}

pub fn parse(agent: Agent, payload: &Value) -> HookNotice {
    match agent {
        Agent::Claude => parse_claude(payload),
        Agent::Codex => parse_codex(payload),
        Agent::Gemini => parse_gemini(payload),
        Agent::Copilot => parse_copilot(payload),
        Agent::Cursor => parse_cursor(payload),
        Agent::OpenCode => parse_opencode(payload),
    }
}

pub fn parse_claude(payload: &Value) -> HookNotice {
    let s = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let event = s("hook_event_name").unwrap_or_default();
    let body = match event.as_str() {
        "Notification" => s("message").or(Some("Claude needs your attention".into())),
        "Stop" | "SubagentStop" => Some(FINISHED.into()),
        _ => None,
    };
    HookNotice { title: Some("Claude Code".into()), body, session_id: s("session_id"), cwd: s("cwd"), transcript_path: s("transcript_path") }
}

pub fn parse_codex(payload: &Value) -> HookNotice {
    let s = |k: &[&str]| str_of(payload, k);
    let body = match s(&["hook_event_name"]).as_deref() {
        // hooks.json events
        Some("Stop") => Some(s(&["last_assistant_message"]).map(|m| truncate(&m, 240)).unwrap_or_else(|| "Turn complete".into())),
        Some("PermissionRequest") => Some("Codex needs your approval".into()),
        Some(_) => None,
        // legacy `notify` program payload
        None => match s(&["type"]).as_deref() {
            Some("agent-turn-complete") => Some(s(&["last-assistant-message"]).map(|m| truncate(&m, 240)).unwrap_or_else(|| "Turn complete".into())),
            None => None,
            Some(other) => Some(other.replace('-', " ")),
        },
    };
    HookNotice { title: Some("Codex".into()), body, session_id: s(&["session_id", "thread-id"]), cwd: s(&["cwd"]), transcript_path: None }
}

pub fn parse_gemini(payload: &Value) -> HookNotice {
    let s = |k: &[&str]| str_of(payload, k);
    let body = match s(&["hook_event_name"]).as_deref() {
        Some("AfterAgent") => Some(s(&["prompt_response"]).map(|m| truncate(&m.split_whitespace().collect::<Vec<_>>().join(" "), 240)).unwrap_or_else(|| FINISHED.into())),
        Some("Notification") => Some(s(&["message"]).unwrap_or_else(|| "Gemini needs your attention".into())),
        _ => None,
    };
    HookNotice { title: Some("Gemini CLI".into()), body, session_id: s(&["session_id"]), cwd: s(&["cwd"]), transcript_path: None }
}

pub fn parse_copilot(payload: &Value) -> HookNotice {
    let s = |k: &[&str]| str_of(payload, k);
    let kind = s(&["notificationType", "notification_type"]);
    let event = s(&["hook_event_name"]);
    let body = if kind.is_some() || event.as_deref().is_some_and(|e| e.eq_ignore_ascii_case("notification")) {
        match kind.as_deref() {
            // turn completions are reported by agentStop
            Some("agent_idle") | Some("shell_completion") => None,
            _ => Some(s(&["message"]).unwrap_or_else(|| "Copilot needs your attention".into())),
        }
    } else {
        match s(&["stopReason", "stop_reason"]).as_deref() {
            Some("error") => Some("Stopped with an error".into()),
            Some("abort") | Some("user_exit") => None,
            _ => Some(FINISHED.into()),
        }
    };
    HookNotice { title: Some("Copilot CLI".into()), body, session_id: s(&["sessionId", "session_id"]), cwd: s(&["cwd"]), transcript_path: None }
}

pub fn parse_cursor(payload: &Value) -> HookNotice {
    let s = |k: &[&str]| str_of(payload, k);
    let body = match s(&["status"]).as_deref() {
        // the user stopped it themselves
        Some("aborted") => None,
        Some("error") => Some("Stopped with an error".into()),
        _ => Some(FINISHED.into()),
    };
    HookNotice { title: Some("Cursor".into()), body, session_id: s(&["conversation_id"]), cwd: None, transcript_path: None }
}

pub fn parse_opencode(payload: &Value) -> HookNotice {
    let s = |k: &[&str]| str_of(payload, k);
    let body = match s(&["type"]).as_deref() {
        Some("session.idle") | None => Some(FINISHED.into()),
        Some(t) if t.starts_with("permission.") => Some("OpenCode needs your approval".into()),
        Some(_) => None,
    };
    HookNotice { title: Some("OpenCode".into()), body, session_id: s(&["sessionID"]), cwd: None, transcript_path: None }
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
    fn gemini_and_codex_hooks() {
        let mut g = json!({ "theme": "dark" });
        assert!(gemini_install(&mut g, "hivetty"));
        assert!(!gemini_install(&mut g, "hivetty"));
        assert_eq!(g["hooks"]["AfterAgent"][0]["matcher"], "*");
        assert_eq!(g["hooks"]["Notification"][0]["hooks"][0]["command"], "hivetty notify --from-hook gemini");
        assert_eq!(g["hooks"]["AfterAgent"][0]["hooks"][0]["timeout"], 10000);
        assert!(nested_uninstall(&mut g));
        assert_eq!(g, json!({ "theme": "dark" }));

        let mut c = json!({});
        assert!(codex_hooks_install(&mut c, "hivetty"));
        assert!(c["hooks"]["Stop"][0].get("matcher").is_none());
        assert_eq!(c["hooks"]["PermissionRequest"][0]["hooks"][0]["command"], "hivetty notify --from-hook codex");
        assert!(!codex_hooks_install(&mut c, "hivetty"));
    }

    #[test]
    fn cursor_hooks() {
        let mut h = json!({ "version": 1, "hooks": { "stop": [{ "command": "./audit.sh" }] } });
        assert!(cursor_install(&mut h, "hivetty"));
        assert!(!cursor_install(&mut h, "hivetty"));
        assert_eq!(h["hooks"]["stop"].as_array().unwrap().len(), 2);
        assert!(cursor_uninstall(&mut h));
        assert_eq!(h, json!({ "version": 1, "hooks": { "stop": [{ "command": "./audit.sh" }] } }));
        let mut empty = json!({});
        assert!(cursor_install(&mut empty, "hivetty"));
        assert_eq!(empty["version"], 1);
    }

    #[test]
    fn copilot_and_opencode_files() {
        let f = copilot_file("C:/Program Files/Hivetty/bin/hivetty.exe");
        assert_eq!(f["version"], 1);
        assert!(is_ours(&f["hooks"]["agentStop"][0]));
        assert_eq!(f["hooks"]["agentStop"][0]["args"], json!(["notify", "--from-hook", "copilot"]));
        let p = opencode_plugin(r"C:\Program Files\Hivetty\bin\hivetty.exe");
        assert!(p.contains(r#"const HIVETTY = "C:\\Program Files\\Hivetty\\bin\\hivetty.exe";"#));
        assert!(p.contains("session.idle"));
    }

    #[test]
    fn codex_legacy_uninstall() {
        let mut doc: toml_edit::DocumentMut = "notify = [\"C:/x/hivetty.exe\", \"notify\", \"--from-hook\", \"codex\"]\nmodel = \"o3\"\n".parse().unwrap();
        assert!(codex_uninstall(&mut doc));
        assert_eq!(doc.to_string().trim(), "model = \"o3\"");
        let mut other: toml_edit::DocumentMut = "notify = [\"notify-send\"]\n".parse().unwrap();
        assert!(!codex_uninstall(&mut other));
    }

    #[test]
    fn payloads() {
        let n = parse_claude(&json!({"hook_event_name":"Notification","message":"Claude needs permission to use Bash","session_id":"abc"}));
        assert_eq!(n.body.as_deref(), Some("Claude needs permission to use Bash"));
        assert_eq!(n.session_id.as_deref(), Some("abc"));
        assert_eq!(parse_claude(&json!({"hook_event_name":"SessionStart","session_id":"abc"})).body, None);
        let c = parse_codex(&json!({"type":"agent-turn-complete","last-assistant-message":"Done refactoring"}));
        assert_eq!(c.body.as_deref(), Some("Done refactoring"));
        let c = parse_codex(&json!({"hook_event_name":"Stop","session_id":"s1","last_assistant_message":"All green"}));
        assert_eq!((c.body.as_deref(), c.session_id.as_deref()), (Some("All green"), Some("s1")));
        assert_eq!(parse_codex(&json!({"hook_event_name":"PermissionRequest"})).body.as_deref(), Some("Codex needs your approval"));
        assert_eq!(parse_gemini(&json!({"hook_event_name":"AfterAgent","prompt_response":"Done.\n\nBye"})).body.as_deref(), Some("Done. Bye"));
        assert_eq!(parse_gemini(&json!({"hook_event_name":"Notification","notification_type":"ToolPermission","message":"Allow shell?"})).body.as_deref(), Some("Allow shell?"));
        assert_eq!(parse_gemini(&json!({"hook_event_name":"BeforeAgent"})).body, None);
        assert_eq!(parse_copilot(&json!({"sessionId":"x","stopReason":"end_turn"})).body.as_deref(), Some(FINISHED));
        assert_eq!(parse_copilot(&json!({"notificationType":"permission_prompt","message":"Approve edit?"})).body.as_deref(), Some("Approve edit?"));
        assert_eq!(parse_copilot(&json!({"notificationType":"agent_idle"})).body, None);
        assert_eq!(parse_cursor(&json!({"status":"completed","loop_count":0})).body.as_deref(), Some(FINISHED));
        assert_eq!(parse_cursor(&json!({"status":"aborted"})).body, None);
        assert_eq!(parse_cursor(&Value::Null).body.as_deref(), Some(FINISHED));
        assert_eq!(parse_opencode(&json!({"type":"permission.asked"})).body.as_deref(), Some("OpenCode needs your approval"));
        let t = "{\"type\":\"user\"}\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"All  tests\\npass.\"}]}}\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\"}]}}\n";
        assert_eq!(claude_last_assistant_text(t).as_deref(), Some("All tests pass."));
    }
}
