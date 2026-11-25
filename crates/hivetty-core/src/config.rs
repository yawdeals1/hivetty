//! User configuration (`%APPDATA%\Hivetty\hivetty.json`) and per-project command files.
//!
//! Files are JSON with comments and trailing commas allowed (JSONC). Parsing is lenient about
//! unknown keys (reported as warnings) but strict about types (reported as an error, and the
//! caller keeps the last good config).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CONFIG_FILE: &str = "hivetty.json";
pub const SCHEMA_FILE: &str = "hivetty.schema.json";
/// Per-project command files, looked up from a pane's cwd towards the filesystem root.
pub const PROJECT_FILES: [&str; 2] = ["hivetty.json", ".hivetty.json"];

/// Hivetty settings. Every key is optional; missing keys use their defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    /// JSON schema reference for editor completion.
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub font: FontConfig,
    pub cursor: CursorConfig,
    /// Active color theme: a built-in name or one from `themes`.
    pub theme: String,
    /// Extra color themes (import Windows Terminal schemes from Settings → Appearance).
    pub themes: Vec<Theme>,
    /// Interface theme for the sidebar, tabs, panels and dialogs: "match" follows the terminal
    /// theme's colors, or a built-in name ("Honeycomb", "Propolis", "Night Shift", "Lichen",
    /// "Aubergine", "Contrast", "Beeswax", "Porcelain").
    pub ui_theme: String,
    pub terminal: TerminalConfig,
    /// Profile id for new terminals (e.g. "pwsh", "cmd", "gitbash", "wsl:Ubuntu", or a custom id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    /// Custom shell profiles. A profile with the id of a detected one replaces it.
    pub profiles: Vec<CustomProfile>,
    /// Detected profile ids to hide from menus.
    pub hidden_profiles: Vec<String>,
    /// Key chord → action id. Use null to unbind a default chord. Example: {"ctrl+shift+d": "pane.splitRight"}.
    pub keybindings: BTreeMap<String, Option<String>>,
    pub session: SessionConfig,
    /// How "Find in Files" results open. Unset: VS Code when installed, otherwise the system default app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor: Option<EditorConfig>,
    /// Commands shown in the command palette for every project.
    pub commands: Vec<CustomCommand>,
    pub updates: UpdatesConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            schema: None,
            font: FontConfig::default(),
            cursor: CursorConfig::default(),
            theme: "Hivetty Dark".into(),
            themes: Vec::new(),
            ui_theme: "match".into(),
            terminal: TerminalConfig::default(),
            default_profile: None,
            profiles: Vec::new(),
            hidden_profiles: Vec::new(),
            keybindings: BTreeMap::new(),
            session: SessionConfig::default(),
            editor: None,
            commands: Vec::new(),
            updates: UpdatesConfig::default(),
        }
    }
}

/// Which builds the updater offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Tested releases.
    #[default]
    Stable,
    /// A build of the latest code every day; may be rough.
    Nightly,
}

impl UpdateChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Nightly => "nightly",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct UpdatesConfig {
    pub channel: UpdateChannel,
    /// Check for updates at startup and every few hours (you're asked before anything installs).
    pub check: bool,
    /// Update manifest URL, overriding the built-in one. `{{channel}}` becomes stable or nightly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        UpdatesConfig { channel: UpdateChannel::Stable, check: true, endpoint: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct FontConfig {
    /// CSS font-family list.
    pub family: String,
    #[schemars(range(min = 6, max = 48))]
    pub size: f64,
    #[schemars(range(min = 0.8, max = 3.0))]
    pub line_height: f64,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig { family: "\"Cascadia Code\", \"Cascadia Mono\", Consolas, \"Courier New\", monospace".into(), size: 14.0, line_height: 1.1 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
    Bar,
    Block,
    Underline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CursorConfig {
    pub style: CursorStyle,
    pub blink: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        CursorConfig { style: CursorStyle::Bar, blink: true }
    }
}

/// What a BEL character does in a pane you aren't looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum BellMode {
    /// Mark the pane as needing attention (blue ring).
    #[default]
    Attention,
    /// Attention ring plus a short sound.
    Sound,
    /// Ignore bells.
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TerminalConfig {
    /// Lines of scrollback kept per terminal.
    #[schemars(range(min = 0, max = 200000))]
    pub scrollback: u32,
    /// Copy selected text to the clipboard as soon as it is selected.
    pub copy_on_select: bool,
    /// Ask before pasting more than one line into a shell that doesn't use bracketed paste.
    pub confirm_multiline_paste: bool,
    pub bell: BellMode,
    /// Put a tmux-compatible `tmux` command on PATH (and set $TMUX) in local terminals, so tools
    /// that drive tmux (e.g. Claude Code agent teams) open native Hivetty splits instead.
    pub tmux_shim: bool,
    /// On launch, install Hivetty's notification hooks into the coding agents found on this
    /// machine (Claude Code, Codex, Gemini CLI, Copilot CLI, Cursor CLI, OpenCode),
    /// so an agent that finishes or needs input marks its pane, tab and workspace.
    pub agent_hooks: bool,
    /// Load Hivetty shell integration in WSL terminals whose login shell is bash
    /// (reports the working directory and prompt marks).
    pub wsl_shell_integration: bool,
    /// Expose terminal output to screen readers (Narrator, NVDA, JAWS). Costs some speed.
    pub screen_reader: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        TerminalConfig { scrollback: 20000, copy_on_select: false, confirm_multiline_paste: true, bell: BellMode::Attention, tmux_shim: true, agent_hooks: true, wsl_shell_integration: true, screen_reader: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SessionConfig {
    /// Reopen workspaces, tabs, splits, working directories and browser URLs on launch.
    pub restore: bool,
    /// Save and restore terminal scrollback (secret-looking values are redacted first).
    pub scrollback: bool,
    /// Maximum scrollback lines saved per terminal.
    #[schemars(range(min = 0, max = 100000))]
    pub scrollback_lines: u32,
    /// Automatically resume recorded agent sessions (e.g. `claude --resume <id>`) on restore.
    /// When false, restored panes offer a Resume button instead.
    pub resume_agents: bool,
    /// Keep shells running after Hivetty closes: new terminals run in a detached session host
    /// (`hivetty-host.exe`) and are reattached, still running, on the next launch.
    pub persistent: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig { restore: true, scrollback: true, scrollback_lines: 2000, resume_agents: false, persistent: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EditorConfig {
    /// Command template. Placeholders: {file}, {line}, {col}. Example: "nvim +{line} {file}".
    pub command: String,
    /// Run inside a new Hivetty split (true, for terminal editors) or as a separate program (false).
    #[serde(default = "yes")]
    pub terminal: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CustomProfile {
    /// Unique id, used by `default_profile` and `hivetty new --profile`.
    pub id: String,
    pub name: String,
    /// Executable path or name on PATH.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Shell family for shell integration: pwsh, powershell, cmd, bash, wsl or other. Guessed from `command` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Starting directory when no other cwd applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Extra environment variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// Where a custom command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CommandTarget {
    /// Type it into the focused terminal.
    Current,
    #[default]
    SplitRight,
    SplitDown,
    Tab,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CustomCommand {
    /// Name shown in the command palette.
    pub name: String,
    /// Shell command line to run.
    pub command: String,
    #[serde(default)]
    pub target: CommandTarget,
    /// Working directory; relative paths resolve against the project file's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Shell profile id to run it in (new panes only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Key chord that runs this command (user config only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keybinding: Option<String>,
}

/// A per-project `hivetty.json` (or `.hivetty.json`) checked into a repository.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ProjectFile {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub commands: Vec<CustomCommand>,
}

/// Terminal colors (#rrggbb or #rrggbbaa).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Theme {
    pub name: String,
    pub background: String,
    pub foreground: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_background: Option<String>,
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
    pub bright_black: String,
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_magenta: String,
    pub bright_cyan: String,
    pub bright_white: String,
}

/// Result of loading a config file.
#[derive(Debug, Clone, Serialize)]
pub struct Loaded {
    pub config: Config,
    /// Unknown keys and similar non-fatal problems.
    pub warnings: Vec<String>,
    /// A fatal problem; `config` is then the defaults (callers usually keep their previous config).
    pub error: Option<String>,
}

/// Strip `//` and `/* */` comments and trailing commas so serde_json can read JSONC.
/// Positions of everything else are preserved (comments become spaces) so error line/column
/// numbers still point at the original text.
pub fn strip_jsonc(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_str = true;
                out.push(c);
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                out.extend_from_slice(b"  ");
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
                if i < b.len() {
                    out.extend_from_slice(b"  ");
                    i += 2;
                }
            }
            b',' => {
                // trailing comma: next non-space (skipping comments handled above lazily) is } or ]
                let mut j = i + 1;
                loop {
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if b.get(j) == Some(&b'/') && b.get(j + 1) == Some(&b'/') {
                        while j < b.len() && b[j] != b'\n' {
                            j += 1;
                        }
                    } else if b.get(j) == Some(&b'/') && b.get(j + 1) == Some(&b'*') {
                        j += 2;
                        while j < b.len() && !(b[j] == b'*' && b.get(j + 1) == Some(&b'/')) {
                            j += 1;
                        }
                        j = (j + 2).min(b.len());
                    } else {
                        break;
                    }
                }
                out.push(if matches!(b.get(j), Some(b'}') | Some(b']')) { b' ' } else { b',' });
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    // Only ASCII bytes were replaced, so this stays valid UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

/// Parse JSONC text into a Value.
pub fn parse_jsonc(src: &str) -> Result<Value, String> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    if src.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str(&strip_jsonc(src)).map_err(|e| e.to_string())
}

fn parse_with_warnings<T: for<'de> Deserialize<'de>>(src: &str) -> Result<(T, Vec<String>), String> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let cleaned = if src.trim().is_empty() { "{}".to_string() } else { strip_jsonc(src) };
    let mut warnings = Vec::new();
    let de = &mut serde_json::Deserializer::from_str(&cleaned);
    let v: T = serde_ignored::deserialize(de, |path| warnings.push(format!("unknown setting `{path}`"))).map_err(|e| e.to_string())?;
    Ok((v, warnings))
}

/// Parse a user config file's text.
pub fn load_config(src: &str) -> Loaded {
    match parse_with_warnings::<Config>(src) {
        Ok((config, mut warnings)) => {
            warnings.extend(validate(&config));
            Loaded { config, warnings, error: None }
        }
        Err(e) => Loaded { config: Config::default(), warnings: vec![], error: Some(e) },
    }
}

/// Parse a project command file.
pub fn load_project(src: &str) -> Result<(ProjectFile, Vec<String>), String> {
    parse_with_warnings::<ProjectFile>(src)
}

fn validate(c: &Config) -> Vec<String> {
    let mut w = Vec::new();
    if !(6.0..=48.0).contains(&c.font.size) {
        w.push(format!("font.size {} is outside 6–48", c.font.size));
    }
    if !(0.8..=3.0).contains(&c.font.line_height) {
        w.push(format!("font.line_height {} is outside 0.8–3", c.font.line_height));
    }
    if !builtin_themes().iter().chain(&c.themes).any(|t| t.name == c.theme) {
        w.push(format!("theme `{}` not found; using Hivetty Dark", c.theme));
    }
    for t in &c.themes {
        for (k, v) in theme_colors(t) {
            if !is_color(v) {
                w.push(format!("theme `{}`: {k} `{v}` is not a #rrggbb color", t.name));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for p in &c.profiles {
        if p.id.trim().is_empty() || p.command.trim().is_empty() {
            w.push(format!("profile `{}` needs an id and a command", p.name));
        }
        if !seen.insert(&p.id) {
            w.push(format!("profile id `{}` is used more than once", p.id));
        }
    }
    for cmd in &c.commands {
        if cmd.name.trim().is_empty() || cmd.command.trim().is_empty() {
            w.push("custom commands need a name and a command".into());
        }
    }
    if let Some(e) = &c.editor {
        if !e.command.contains("{file}") {
            w.push("editor.command should contain {file}".into());
        }
    }
    w
}

fn is_color(s: &str) -> bool {
    let h = s.strip_prefix('#').unwrap_or("");
    matches!(h.len(), 3 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit())
}

fn theme_colors(t: &Theme) -> Vec<(&'static str, &str)> {
    let mut v = vec![
        ("background", t.background.as_str()),
        ("foreground", t.foreground.as_str()),
        ("black", &t.black),
        ("red", &t.red),
        ("green", &t.green),
        ("yellow", &t.yellow),
        ("blue", &t.blue),
        ("magenta", &t.magenta),
        ("cyan", &t.cyan),
        ("white", &t.white),
        ("bright_black", &t.bright_black),
        ("bright_red", &t.bright_red),
        ("bright_green", &t.bright_green),
        ("bright_yellow", &t.bright_yellow),
        ("bright_blue", &t.bright_blue),
        ("bright_magenta", &t.bright_magenta),
        ("bright_cyan", &t.bright_cyan),
        ("bright_white", &t.bright_white),
    ];
    if let Some(c) = &t.cursor {
        v.push(("cursor", c));
    }
    if let Some(c) = &t.selection_background {
        v.push(("selection_background", c));
    }
    v
}

/// Set (or with `value: None`, remove) the key at `path` inside a JSON object tree,
/// creating intermediate objects. Used by the settings UI so it changes only what it edits.
pub fn set_path(root: &mut Value, path: &[String], value: Option<Value>) -> Result<(), String> {
    let Some((last, parents)) = path.split_last() else { return Err("empty path".into()) };
    if !root.is_object() {
        *root = Value::Object(Default::default());
    }
    let mut cur = root;
    for k in parents {
        let obj = cur.as_object_mut().ok_or_else(|| format!("`{k}` is not an object"))?;
        let next = obj.entry(k.clone()).or_insert_with(|| Value::Object(Default::default()));
        if !next.is_object() {
            *next = Value::Object(Default::default());
        }
        cur = next;
    }
    let obj = cur.as_object_mut().ok_or("not an object")?;
    match value {
        Some(v) => {
            obj.insert(last.clone(), v);
        }
        None => {
            obj.shift_remove(last);
        }
    }
    Ok(())
}

/// JSON Schema for the user config file.
pub fn config_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(Config)).unwrap_or_default()
}

/// JSON Schema for per-project command files.
pub fn project_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(ProjectFile)).unwrap_or_default()
}

/// Guess a profile's shell family from its executable name.
pub fn guess_shell(command: &str) -> String {
    let exe = command.rsplit(['\\', '/']).next().unwrap_or(command).to_ascii_lowercase();
    let exe = exe.strip_suffix(".exe").unwrap_or(&exe);
    match exe {
        "pwsh" => "pwsh",
        "powershell" => "powershell",
        "cmd" => "cmd",
        "bash" | "sh" | "zsh" => "bash",
        "wsl" => "wsl",
        _ => "other",
    }
    .to_string()
}

// ---- themes ----

#[allow(clippy::too_many_arguments)]
fn theme(name: &str, bg: &str, fg: &str, cursor: &str, sel: &str, c: [&str; 16]) -> Theme {
    let s = |x: &str| x.to_string();
    Theme {
        name: s(name),
        background: s(bg),
        foreground: s(fg),
        cursor: Some(s(cursor)),
        selection_background: Some(s(sel)),
        black: s(c[0]),
        red: s(c[1]),
        green: s(c[2]),
        yellow: s(c[3]),
        blue: s(c[4]),
        magenta: s(c[5]),
        cyan: s(c[6]),
        white: s(c[7]),
        bright_black: s(c[8]),
        bright_red: s(c[9]),
        bright_green: s(c[10]),
        bright_yellow: s(c[11]),
        bright_blue: s(c[12]),
        bright_magenta: s(c[13]),
        bright_cyan: s(c[14]),
        bright_white: s(c[15]),
    }
}

pub fn builtin_themes() -> Vec<Theme> {
    vec![
        theme(
            "Hivetty Dark",
            "#111317",
            "#d8dee9",
            "#f5b53d",
            "#3b4252aa",
            ["#1d2027", "#ef6b73", "#8fd18b", "#f5c76b", "#5fa8ff", "#c98bf5", "#5fd1d1", "#d8dee9", "#5c6370", "#ff8a91", "#a8e6a3", "#ffd98a", "#86beff", "#dcaaff", "#86e6e6", "#ffffff"],
        ),
        theme(
            "Campbell",
            "#0c0c0c",
            "#cccccc",
            "#ffffff",
            "#ffffff55",
            ["#0c0c0c", "#c50f1f", "#13a10e", "#c19c00", "#0037da", "#881798", "#3a96dd", "#cccccc", "#767676", "#e74856", "#16c60c", "#f9f1a5", "#3b78ff", "#b4009e", "#61d6d6", "#f2f2f2"],
        ),
        theme(
            "One Half Dark",
            "#282c34",
            "#dcdfe4",
            "#a3b3cc",
            "#ffffff40",
            ["#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#dcdfe4", "#5a6374", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#dcdfe4"],
        ),
        theme(
            "One Half Light",
            "#fafafa",
            "#383a42",
            "#4f525d",
            "#38394233",
            ["#383a42", "#e45649", "#50a14f", "#c18301", "#0184bc", "#a626a4", "#0997b3", "#fafafa", "#4f525d", "#df6c75", "#98c379", "#e4c07a", "#61afef", "#c577dd", "#56b5c1", "#ffffff"],
        ),
        theme(
            "Solarized Dark",
            "#002b36",
            "#839496",
            "#93a1a1",
            "#073642",
            ["#002b36", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5", "#073642", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"],
        ),
        theme(
            "Tango Dark",
            "#000000",
            "#d3d7cf",
            "#ffffff",
            "#ffffff40",
            ["#000000", "#cc0000", "#4e9a06", "#c4a000", "#3465a4", "#75507b", "#06989a", "#d3d7cf", "#555753", "#ef2929", "#8ae234", "#fce94f", "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec"],
        ),
    ]
}

/// Find the theme a config selects (falls back to Hivetty Dark).
pub fn resolve_theme(c: &Config) -> Theme {
    c.themes
        .iter()
        .rev()
        .find(|t| t.name == c.theme)
        .cloned()
        .or_else(|| builtin_themes().into_iter().find(|t| t.name == c.theme))
        .unwrap_or_else(|| builtin_themes().remove(0))
}

/// Extract color schemes from a Windows Terminal settings.json (JSONC). Also accepts a single
/// scheme object or an array of schemes, so users can paste one from the web.
pub fn import_wt_schemes(src: &str) -> Result<Vec<Theme>, String> {
    let v = parse_jsonc(src)?;
    let list: Vec<&Value> = match &v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) if o.contains_key("schemes") => o["schemes"].as_array().map(|a| a.iter().collect()).unwrap_or_default(),
        Value::Object(_) => vec![&v],
        _ => vec![],
    };
    let mut out = Vec::new();
    for s in list {
        let g = |k: &str| s.get(k).and_then(Value::as_str).map(str::to_string);
        let (Some(name), Some(bg), Some(fg)) = (g("name"), g("background"), g("foreground")) else { continue };
        let need = |k: &str| g(k).unwrap_or_else(|| fg.clone());
        out.push(Theme {
            name,
            background: bg,
            foreground: fg.clone(),
            cursor: g("cursorColor"),
            selection_background: g("selectionBackground"),
            black: need("black"),
            red: need("red"),
            green: need("green"),
            yellow: need("yellow"),
            blue: need("blue"),
            magenta: g("purple").or_else(|| g("magenta")).unwrap_or_else(|| fg.clone()),
            cyan: need("cyan"),
            white: need("white"),
            bright_black: need("brightBlack"),
            bright_red: need("brightRed"),
            bright_green: need("brightGreen"),
            bright_yellow: need("brightYellow"),
            bright_blue: need("brightBlue"),
            bright_magenta: g("brightPurple").or_else(|| g("brightMagenta")).unwrap_or_else(|| fg.clone()),
            bright_cyan: need("brightCyan"),
            bright_white: need("brightWhite"),
        });
    }
    if out.is_empty() {
        return Err("no color schemes found".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_comments_and_trailing_commas() {
        let src = r#"{
  // line comment
  "a": "x // not a comment", /* block
  comment */ "b": [1, 2, ],
  "c": { "d": "\"/*\"", },
}"#;
        let v = parse_jsonc(src).unwrap();
        assert_eq!(v["a"], "x // not a comment");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
        assert_eq!(v["c"]["d"], "\"/*\"");
        // error positions still point into the original text
        let err = parse_jsonc("{\n  // c\n  \"a\": nope\n}").unwrap_err();
        assert!(err.contains("line 3"), "{err}");
    }

    #[test]
    fn config_defaults_and_warnings() {
        let l = load_config("");
        assert!(l.error.is_none());
        assert_eq!(l.config, Config::default());
        let l = load_config(r#"{ "font": { "size": 16, "weight": 3 }, "bogus": 1, "theme": "Nope" }"#);
        assert!(l.error.is_none());
        assert_eq!(l.config.font.size, 16.0);
        assert_eq!(l.config.font.line_height, 1.1);
        assert!(l.warnings.iter().any(|w| w.contains("font.weight")), "{:?}", l.warnings);
        assert!(l.warnings.iter().any(|w| w.contains("bogus")));
        assert!(l.warnings.iter().any(|w| w.contains("theme `Nope`")));
        let l = load_config(r#"{ "font": { "size": "big" } }"#);
        assert!(l.error.is_some());
    }

    #[test]
    fn keybinding_nulls_survive() {
        let l = load_config(r#"{ "keybindings": { "ctrl+shift+d": null, "ctrl+alt+x": "pane.zoom" } }"#);
        assert_eq!(l.config.keybindings.get("ctrl+shift+d"), Some(&None));
        assert_eq!(l.config.keybindings.get("ctrl+alt+x"), Some(&Some("pane.zoom".into())));
    }

    #[test]
    fn set_path_edits_in_place() {
        let mut v = serde_json::json!({ "$schema": "s", "font": { "size": 12, "family": "x" }, "other": 1 });
        set_path(&mut v, &["font".into(), "size".into()], Some(serde_json::json!(15))).unwrap();
        set_path(&mut v, &["keybindings".into(), "ctrl+k".into()], Some(Value::Null)).unwrap();
        set_path(&mut v, &["other".into()], None).unwrap();
        assert_eq!(v, serde_json::json!({ "$schema": "s", "font": { "size": 15, "family": "x" }, "keybindings": { "ctrl+k": null } }));
        // key order is preserved
        assert_eq!(v.as_object().unwrap().keys().next().unwrap(), "$schema");
    }

    #[test]
    fn wt_import() {
        let src = r##"{
            "profiles": {},
            // comment
            "schemes": [
                { "name": "Neon", "background": "#000000", "foreground": "#ffffff", "cursorColor": "#ff00ff",
                  "black": "#000000", "red": "#ff0000", "green": "#00ff00", "yellow": "#ffff00", "blue": "#0000ff",
                  "purple": "#ff00ff", "cyan": "#00ffff", "white": "#eeeeee", "brightBlack": "#555555",
                  "brightRed": "#ff5555", "brightGreen": "#55ff55", "brightYellow": "#ffff55", "brightBlue": "#5555ff",
                  "brightPurple": "#ff55ff", "brightCyan": "#55ffff", "brightWhite": "#ffffff" },
                { "name": "broken" },
            ],
        }"##;
        let t = import_wt_schemes(src).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].magenta, "#ff00ff");
        assert_eq!(t[0].bright_magenta, "#ff55ff");
        assert_eq!(t[0].cursor.as_deref(), Some("#ff00ff"));
        assert!(import_wt_schemes("{}").is_err());
        let single = import_wt_schemes(r##"{"name":"S","background":"#111111","foreground":"#eeeeee"}"##).unwrap();
        assert_eq!(single[0].red, "#eeeeee");
    }

    #[test]
    fn theme_resolution() {
        let mut c = Config::default();
        assert_eq!(resolve_theme(&c).name, "Hivetty Dark");
        c.theme = "Campbell".into();
        assert_eq!(resolve_theme(&c).background, "#0c0c0c");
        c.theme = "Missing".into();
        assert_eq!(resolve_theme(&c).name, "Hivetty Dark");
        for t in builtin_themes() {
            assert!(theme_colors(&t).iter().all(|(_, v)| is_color(v)), "{}", t.name);
        }
    }

    #[test]
    fn project_file() {
        let (p, w) = load_project(r#"{ "commands": [ { "name": "Dev", "command": "pnpm dev", "target": "split-down", "cwd": "ui" } ] }"#).unwrap();
        assert!(w.is_empty());
        assert_eq!(p.commands[0].target, CommandTarget::SplitDown);
        let (p, _) = load_project(r#"{ "commands": [ { "name": "T", "command": "cargo test" } ] }"#).unwrap();
        assert_eq!(p.commands[0].target, CommandTarget::SplitRight);
        assert!(load_project(r#"{ "commands": [ { "name": "x" } ] }"#).is_err());
    }

    #[test]
    fn shells() {
        assert_eq!(guess_shell(r"C:\Program Files\PowerShell\7\pwsh.exe"), "pwsh");
        assert_eq!(guess_shell("bash"), "bash");
        assert_eq!(guess_shell("nu.exe"), "other");
    }

    /// The checked-in schemas must match the Rust types. Regenerate with HIVETTY_UPDATE_SCHEMA=1.
    #[test]
    fn schemas_up_to_date() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
        for (file, schema) in [("hivetty.schema.json", config_schema()), ("hivetty-project.schema.json", project_schema())] {
            let path = dir.join(file);
            let want = serde_json::to_string_pretty(&schema).unwrap() + "\n";
            if std::env::var_os("HIVETTY_UPDATE_SCHEMA").is_some() {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &want).unwrap();
            }
            let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
            assert!(have == want, "{file} is stale; run `HIVETTY_UPDATE_SCHEMA=1 cargo test -p hivetty-core`");
        }
    }
}
