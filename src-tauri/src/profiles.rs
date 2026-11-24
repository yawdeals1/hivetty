//! Shell profile detection (PowerShell 7, Windows PowerShell, cmd, Git Bash, WSL distros).

use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// "pwsh" | "powershell" | "cmd" | "bash" | "wsl" — drives shell-integration injection.
    pub shell: String,
    /// Starting directory when nothing else applies (custom profiles).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Extra environment (custom profiles). Never sent to the UI.
    #[serde(skip)]
    pub env: Vec<(String, String)>,
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn exists(p: &Path) -> bool {
    p.is_file()
}

pub fn detect() -> Vec<Profile> {
    let mut out = Vec::new();
    let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());

    let pwsh = which::which("pwsh.exe")
        .ok()
        .or_else(|| Some(PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe")).filter(|p| exists(p)));
    if let Some(p) = pwsh {
        out.push(Profile { id: "pwsh".into(), name: "PowerShell".into(), command: p.display().to_string(), args: vec!["-NoLogo".into()], shell: "pwsh".into(), cwd: None, env: vec![] });
    }
    let winps = PathBuf::from(&sysroot).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if exists(&winps) {
        out.push(Profile { id: "powershell".into(), name: "Windows PowerShell".into(), command: winps.display().to_string(), args: vec!["-NoLogo".into()], shell: "powershell".into(), cwd: None, env: vec![] });
    }
    let cmd = std::env::var("ComSpec").unwrap_or_else(|_| format!(r"{sysroot}\System32\cmd.exe"));
    out.push(Profile { id: "cmd".into(), name: "Command Prompt".into(), command: cmd, args: vec![], shell: "cmd".into(), cwd: None, env: vec![] });

    let git_bash = [r"C:\Program Files\Git\bin\bash.exe", r"C:\Program Files (x86)\Git\bin\bash.exe"]
        .iter()
        .map(PathBuf::from)
        .find(|p| exists(p))
        .or_else(|| {
            // derive from git.exe on PATH: <root>\cmd\git.exe -> <root>\bin\bash.exe
            which::which("git.exe").ok().and_then(|g| g.parent()?.parent().map(|r| r.join(r"bin\bash.exe"))).filter(|p| exists(p))
        });
    if let Some(p) = git_bash {
        out.push(Profile { id: "gitbash".into(), name: "Git Bash".into(), command: p.display().to_string(), args: vec!["--login".into(), "-i".into()], shell: "bash".into(), cwd: None, env: vec![] });
    }

    for distro in wsl_distros() {
        out.push(Profile {
            id: format!("wsl:{distro}"),
            name: format!("WSL: {distro}"),
            command: "wsl.exe".into(),
            args: vec!["-d".into(), distro.clone(), "--cd".into(), "~".into()],
            shell: "wsl".into(),
            cwd: None,
            env: vec![],
        });
    }
    out
}

/// Installed WSL distros, the default one first. Read from the registry (instant); `wsl -l -q`
/// costs ~100 ms of startup, so it's only the fallback.
fn wsl_distros() -> Vec<String> {
    wsl_distros_registry().unwrap_or_else(wsl_distros_cli)
}

fn wsl_distros_registry() -> Option<Vec<String>> {
    use winreg::enums::HKEY_CURRENT_USER;
    let lxss = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Lxss").ok()?;
    let default: String = lxss.get_value("DefaultDistribution").unwrap_or_default();
    let mut list: Vec<(bool, String)> = Vec::new();
    for guid in lxss.enum_keys().flatten() {
        let Ok(k) = lxss.open_subkey(&guid) else { continue };
        let Ok(name) = k.get_value::<String, _>("DistributionName") else { continue };
        // State 1 = installed (others: installing / uninstalling)
        if k.get_value::<u32, _>("State").map(|s| s != 1).unwrap_or(false) || name.starts_with("docker-desktop") {
            continue;
        }
        list.push((!guid.eq_ignore_ascii_case(&default), name));
    }
    if list.is_empty() {
        return None;
    }
    list.sort_by_key(|(not_default, _)| *not_default);
    Some(list.into_iter().map(|(_, n)| n).collect())
}

fn wsl_distros_cli() -> Vec<String> {
    let Ok(out) = std::process::Command::new("wsl.exe").args(["-l", "-q"]).creation_flags(CREATE_NO_WINDOW).output() else {
        return vec![];
    };
    if !out.status.success() {
        return vec![];
    }
    // wsl.exe writes UTF-16LE
    let u16s: Vec<u16> = out.stdout.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&u16s)
        .lines()
        .map(|l| l.trim().trim_matches('\u{feff}').to_string())
        .filter(|l| !l.is_empty() && !l.starts_with("docker-desktop"))
        .collect()
}

pub fn default_profile(profiles: &[Profile]) -> String {
    profiles.first().map(|p| p.id.clone()).unwrap_or_else(|| "cmd".into())
}

/// Profile for a WSL distro that isn't in the detected list (e.g. a restored pane).
pub fn wsl_profile(distro: &str) -> Profile {
    Profile {
        id: format!("wsl:{distro}"),
        name: format!("WSL: {distro}"),
        command: "wsl.exe".into(),
        args: vec!["-d".into(), distro.to_string(), "--cd".into(), "~".into()],
        shell: "wsl".into(),
        cwd: None,
        env: vec![],
    }
}
