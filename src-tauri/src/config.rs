//! Loads `%APPDATA%\Hivetty\hivetty.json`, hot-reloads it when the file changes, and applies
//! it to the backend (shell profiles, default profile). The UI gets every change as a `config`
//! event and applies fonts, theme and keybindings itself.

use crate::app::AppState;
use crate::profiles::{self, Profile};
use hivetty_core::config::{self as cfg, Config, Loaded, CONFIG_FILE, SCHEMA_FILE};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager};

const PROJECT_SCHEMA_FILE: &str = "hivetty-project.schema.json";

/// `%APPDATA%\Hivetty` (overridable with HIVETTY_HOME for tests and portable installs).
pub fn data_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("HIVETTY_HOME") {
        return PathBuf::from(h);
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Hivetty")
}

pub fn config_path() -> PathBuf {
    data_dir().join(CONFIG_FILE)
}

/// What the UI receives.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigState {
    pub config: Config,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub path: String,
    pub builtin_themes: Vec<cfg::Theme>,
}

fn mtime(p: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Write `data` to `path` atomically (temp file + rename) so readers never see half a file.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Create the data dir, refresh the bundled JSON schemas, and write a starter config if none exists.
fn ensure_files() {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    for (name, schema) in [(SCHEMA_FILE, cfg::config_schema()), (PROJECT_SCHEMA_FILE, cfg::project_schema())] {
        let text = serde_json::to_string_pretty(&schema).unwrap_or_default();
        let p = dir.join(name);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(text.as_str()) {
            let _ = write_atomic(&p, text.as_bytes());
        }
    }
    let path = config_path();
    if !path.exists() {
        let starter = format!(
            "{{\n  \"$schema\": \"./{SCHEMA_FILE}\"\n  // Every setting is optional. Your editor completes keys from the schema,\n  // or use Settings (Ctrl+,) in Hivetty. Changes apply as soon as you save.\n}}\n"
        );
        let _ = write_atomic(&path, starter.as_bytes());
    }
}

/// Text of the config file as last loaded or written by Hivetty, so the watcher can skip
/// reloading our own writes and no-op touches.
static LAST_TEXT: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// Read the config file's text. `Ok(None)` = no file. Retries briefly on sharing violations
/// (an editor or our own atomic rename may hold the file for a moment).
fn read_text() -> std::io::Result<Option<String>> {
    let path = config_path();
    let mut tries = 0;
    loop {
        match std::fs::read_to_string(&path) {
            Ok(s) => return Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A save in progress (write temp + rename) can briefly look like "no file".
                if tries >= 3 {
                    return Ok(None);
                }
            }
            Err(e) if tries >= 6 => return Err(e),
            Err(_) => {}
        }
        tries += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Read and parse the config file.
pub fn read() -> Loaded {
    match read_text() {
        Ok(text) => {
            let text = text.unwrap_or_default();
            let l = cfg::load_config(&text);
            *LAST_TEXT.lock() = Some(text);
            l
        }
        Err(e) => Loaded { config: Config::default(), warnings: vec![], error: Some(format!("can't read {}: {e}", config_path().display())) },
    }
}

/// Load at startup (before the window exists).
pub fn initial() -> Loaded {
    ensure_files();
    let l = read();
    if let Some(e) = &l.error {
        eprintln!("hivetty: config error, using defaults: {e}");
    }
    l
}

pub fn state(st: &AppState) -> ConfigState {
    let c = st.config.lock();
    ConfigState {
        config: c.config.clone(),
        warnings: c.warnings.clone(),
        error: c.error.clone(),
        path: config_path().display().to_string(),
        builtin_themes: cfg::builtin_themes(),
    }
}

/// Detected profiles merged with custom ones: custom ids replace detected ones, hidden ids are dropped.
pub fn build_profiles(detected: &[Profile], c: &Config) -> (Vec<Profile>, String) {
    let mut out: Vec<Profile> = detected.iter().filter(|p| !c.hidden_profiles.contains(&p.id)).cloned().collect();
    for cp in &c.profiles {
        if cp.id.trim().is_empty() || cp.command.trim().is_empty() {
            continue;
        }
        let p = Profile {
            id: cp.id.clone(),
            name: cp.name.clone(),
            command: cp.command.clone(),
            args: cp.args.clone(),
            shell: cp.shell.clone().unwrap_or_else(|| cfg::guess_shell(&cp.command)),
            cwd: cp.cwd.clone(),
            env: cp.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        };
        match out.iter_mut().find(|o| o.id == p.id) {
            Some(slot) => *slot = p,
            None => out.push(p),
        }
    }
    if out.is_empty() {
        out = detected.to_vec();
    }
    let default = c.default_profile.clone().filter(|d| out.iter().any(|p| &p.id == d)).unwrap_or_else(|| profiles::default_profile(&out));
    (out, default)
}

/// Apply a freshly loaded config. A broken file keeps the previous good settings but still
/// surfaces the error to the UI.
pub fn apply(app: &AppHandle, loaded: Loaded) {
    let st = app.state::<AppState>();
    {
        let mut cur = st.config.lock();
        if loaded.error.is_some() {
            cur.error = loaded.error;
            cur.warnings = loaded.warnings;
        } else {
            *cur = loaded;
        }
    }
    let c = st.config.lock().config.clone();
    let (profiles, default) = build_profiles(&st.detected_profiles, &c);
    *st.profiles.lock() = profiles;
    *st.default_profile.lock() = default;
    let _ = app.emit("config", state(&st));
    let _ = app.emit("info", crate::commands::info(&st));
}

/// Poll the config file for changes (cheap: one metadata call per second) and reload.
pub fn watch(app: AppHandle) {
    std::thread::Builder::new()
        .name("config-watch".into())
        .spawn(move || {
            let path = config_path();
            let mut last = mtime(&path);
            loop {
                std::thread::sleep(Duration::from_millis(800));
                let now = mtime(&path);
                if now != last {
                    // editors often write in two steps; let the file settle
                    std::thread::sleep(Duration::from_millis(120));
                    last = mtime(&path);
                    let Ok(text) = read_text() else { continue };
                    let text = text.unwrap_or_default();
                    if LAST_TEXT.lock().as_deref() == Some(text.as_str()) {
                        continue; // our own write, or a touch without changes
                    }
                    *LAST_TEXT.lock() = Some(text.clone());
                    apply(&app, cfg::load_config(&text));
                }
            }
        })
        .expect("spawn config watcher");
}

/// Change one setting in the file, keeping everything else (key order included) as it is.
/// Comments in the file are not preserved by this path.
pub fn set(app: &AppHandle, path: &[String], value: Option<Value>) -> Result<ConfigState, String> {
    let file = config_path();
    // Never treat an unreadable file as empty: that would overwrite the user's settings.
    let text = read_text().map_err(|e| format!("can't read {}: {e}", file.display()))?.unwrap_or_default();
    if std::env::var_os("HIVETTY_DEBUG_CONFIG").is_some() {
        eprintln!("[config] set {path:?} = {value:?}");
    }
    let mut root = cfg::parse_jsonc(&text).map_err(|e| format!("fix {} first: {e}", file.display()))?;
    if root.get("$schema").is_none() {
        cfg::set_path(&mut root, &["$schema".into()], Some(Value::String(format!("./{SCHEMA_FILE}"))))?;
    }
    cfg::set_path(&mut root, path, value)?;
    let out = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())? + "\n";
    // Validate before writing so the UI can't save something the loader rejects.
    let loaded = cfg::load_config(&out);
    if let Some(e) = loaded.error {
        return Err(e);
    }
    write_atomic(&file, out.as_bytes()).map_err(|e| e.to_string())?;
    *LAST_TEXT.lock() = Some(out);
    apply(app, loaded);
    Ok(state(&app.state::<AppState>()))
}

/// Locations Windows Terminal keeps its settings in (stable, preview, unpackaged).
pub fn windows_terminal_settings() -> Vec<PathBuf> {
    let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return vec![] };
    [
        local.join(r"Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json"),
        local.join(r"Packages\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\LocalState\settings.json"),
        local.join(r"Microsoft\Windows Terminal\settings.json"),
    ]
    .into_iter()
    .filter(|p| p.is_file())
    .collect()
}

/// Color schemes from pasted JSON, a file, or every Windows Terminal install.
pub fn wt_schemes(path: Option<String>, text: Option<String>) -> Result<Vec<cfg::Theme>, String> {
    if let Some(t) = text.filter(|t| !t.trim().is_empty()) {
        return cfg::import_wt_schemes(&t).map_err(|e| format!("couldn't read that JSON: {e}"));
    }
    let explicit = path.is_some();
    let files = match path {
        Some(p) => vec![PathBuf::from(p)],
        None => windows_terminal_settings(),
    };
    if files.is_empty() {
        return Err("Windows Terminal settings not found".into());
    }
    let mut out: Vec<cfg::Theme> = Vec::new();
    let mut last_err = None;
    for f in files {
        match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|s| cfg::import_wt_schemes(&s)) {
            Ok(list) => {
                for t in list {
                    if !out.iter().any(|o| o.name == t.name) {
                        out.push(t);
                    }
                }
            }
            Err(e) => last_err = Some(format!("{}: {e}", f.display())),
        }
    }
    if out.is_empty() {
        if !explicit {
            return Err("Windows Terminal has no custom color schemes in its settings (its built-in ones are already here). Paste a scheme's JSON below instead.".into());
        }
        return Err(last_err.unwrap_or_else(|| "no color schemes found".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hivetty_core::config::CustomProfile;

    fn prof(id: &str) -> Profile {
        Profile { id: id.into(), name: id.into(), command: format!("{id}.exe"), args: vec![], shell: id.into(), cwd: None, env: vec![] }
    }

    #[test]
    fn profiles_merge() {
        let detected = vec![prof("pwsh"), prof("cmd"), prof("gitbash")];
        let mut c = Config {
            hidden_profiles: vec!["gitbash".into()],
            ..Default::default()
        };
        c.profiles = vec![
            CustomProfile { id: "cmd".into(), name: "My cmd".into(), command: "cmd.exe".into(), args: vec!["/k".into()], shell: None, cwd: None, env: Default::default() },
            CustomProfile { id: "nu".into(), name: "Nushell".into(), command: r"C:\nu\nu.exe".into(), args: vec![], shell: None, cwd: None, env: Default::default() },
        ];
        c.default_profile = Some("nu".into());
        let (p, d) = build_profiles(&detected, &c);
        assert_eq!(p.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["pwsh", "cmd", "nu"]);
        assert_eq!(p[1].name, "My cmd");
        assert_eq!(p[1].shell, "cmd");
        assert_eq!(p[2].shell, "other");
        assert_eq!(d, "nu");
        c.default_profile = Some("missing".into());
        assert_eq!(build_profiles(&detected, &c).1, "pwsh");
    }
}
