//! Injects Hivetty shell-integration scripts (cwd reporting, prompt marks, exit codes).

use crate::profiles::Profile;
use std::path::PathBuf;
use tauri::{path::BaseDirectory, AppHandle, Manager};

fn script(app: &AppHandle, name: &str) -> Option<PathBuf> {
    let p = app.path().resolve(format!("shell-integration/{name}"), BaseDirectory::Resource).ok()?;
    if p.is_file() {
        return Some(p);
    }
    // dev fallback: repo checkout
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("shell-integration").join(name);
    dev.is_file().then_some(dev)
}

fn clean(p: PathBuf) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

fn enabled() -> bool {
    std::env::var("HIVETTY_NO_SHELL_INTEGRATION").is_err()
}

pub fn launch_args(app: &AppHandle, prof: &Profile, _command: Option<&str>) -> Vec<String> {
    if !enabled() {
        return vec![];
    }
    match prof.shell.as_str() {
        "pwsh" | "powershell" => match script(app, "hivetty.ps1") {
            Some(p) => vec!["-NoExit".into(), "-Command".into(), format!(". '{}'", clean(p).replace('\'', "''"))],
            None => vec![],
        },
        "bash" => match script(app, "hivetty.bash") {
            // replace --login -i with an init file that emulates login startup itself
            Some(p) => vec!["--init-file".into(), to_msys(&clean(p))],
            None => vec![],
        },
        _ => vec![],
    }
}

pub fn env(_app: &AppHandle, prof: &Profile) -> Vec<(String, String)> {
    if !enabled() {
        return vec![];
    }
    match prof.shell.as_str() {
        // cmd: report cwd (OSC 9;9) and prompt marks through PROMPT
        "cmd" => vec![("PROMPT".into(), r"$e]9;9;$P$e\$e]133;A$e\$P$G$e]133;B$e\".into())],
        _ => vec![],
    }
}

/// C:\foo\bar -> /c/foo/bar for Git Bash
fn to_msys(p: &str) -> String {
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        format!("/{}{}", (b[0] as char).to_ascii_lowercase(), p[2..].replace('\\', "/"))
    } else {
        p.replace('\\', "/")
    }
}

/// Profile args with shell integration applied (bash drops its own login flags).
pub fn final_args(app: &AppHandle, prof: &Profile, command: Option<&str>) -> Vec<String> {
    let extra = launch_args(app, prof, command);
    let mut args = prof.args.clone();
    if prof.shell == "bash" && !extra.is_empty() {
        args.retain(|a| a != "--login" && a != "-l");
    }
    args.extend(extra);
    args
}

/// Arguments for a WSL terminal: starts in `cwd` (Linux or Windows path) and, when the distro's
/// login shell is bash, loads Hivetty's bash integration so cwd and prompt marks are reported.
pub fn wsl_args(app: &AppHandle, prof: &Profile, distro: &str, cwd: Option<&str>) -> Vec<String> {
    use hivetty_core::paths::windows_to_wsl;
    let mut args = prof.args.clone();
    let dir = cwd.and_then(|c| if c.starts_with('/') || c.starts_with('~') { Some(c.to_string()) } else { windows_to_wsl(distro, c) });
    if let Some(d) = dir {
        match args.iter().position(|a| a == "--cd") {
            Some(i) if i + 1 < args.len() => args[i + 1] = d,
            Some(_) => args.push(d),
            None => args.splice(0..0, ["--cd".to_string(), d]).for_each(drop),
        }
    }
    let custom_command = args.iter().any(|a| a == "-e" || a == "--exec" || a == "--");
    let wanted = enabled() && !custom_command && {
        use tauri::Manager;
        app.state::<crate::app::AppState>().config.lock().config.terminal.wsl_shell_integration
    };
    if wanted {
        if let Some(script) = script(app, "hivetty.bash").and_then(|p| windows_to_wsl(distro, &clean(p))) {
            let shell = login_shell(app, distro);
            if shell.ends_with("/bash") {
                args.extend(["-e".to_string(), shell, "--init-file".into(), script]);
            }
        }
    }
    args
}

/// The distro's login shell (cached per run; empty when it can't be determined).
fn login_shell(app: &AppHandle, distro: &str) -> String {
    use std::os::windows::process::CommandExt;
    use tauri::Manager;
    let st = app.state::<crate::app::AppState>();
    if let Some(s) = st.wsl_shells.lock().get(distro) {
        return s.clone();
    }
    let shell = (|| {
        let mut child = std::process::Command::new("wsl.exe")
            .args(["-d", distro, "-e", "sh", "-c", "getent passwd $(id -un) | cut -d: -f7"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .ok()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(50)),
                _ => {
                    let _ = child.kill();
                    return None;
                }
            }
        }
        let mut out = String::new();
        std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
        let s = out.trim().to_string();
        s.starts_with('/').then_some(s)
    })()
    .unwrap_or_default();
    st.wsl_shells.lock().insert(distro.to_string(), shell.clone());
    shell
}

/// WSLENV with Hivetty's variables added, so `hivetty.exe` works from inside WSL.
pub fn wslenv() -> String {
    use hivetty_core::protocol::{ENV_PANE, ENV_PIPE, ENV_WORKSPACE};
    let existing = std::env::var("WSLENV").unwrap_or_default();
    let mut parts: Vec<String> = existing.split(':').filter(|s| !s.is_empty()).map(str::to_string).collect();
    for v in [ENV_PIPE, ENV_PANE, ENV_WORKSPACE] {
        if !parts.iter().any(|p| p.split('/').next() == Some(v)) {
            parts.push(v.to_string());
        }
    }
    parts.join(":")
}
