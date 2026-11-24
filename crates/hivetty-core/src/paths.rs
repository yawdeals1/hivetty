//! Path translation between Windows, WSL and remote (POSIX) shells.
//!
//! A pane's `cwd` is whatever its shell reports (OSC 7): a Windows path for local shells, a
//! Linux path for WSL, a remote path for ssh. Anything that touches the local filesystem (git
//! metadata, find in files, project commands, opening files) uses the pane's *local* path, which
//! these helpers derive.

/// Profile id prefix for WSL distros ("wsl:Ubuntu").
pub const WSL_PREFIX: &str = "wsl:";
/// Profile id for terminals that run over the workspace's ssh connection.
pub const SSH_PROFILE: &str = "ssh";

/// Where a terminal's shell runs, derived from its profile id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellHost {
    Local,
    Wsl(String),
    Ssh,
}

pub fn shell_host(profile: &str) -> ShellHost {
    if profile == SSH_PROFILE {
        ShellHost::Ssh
    } else if let Some(d) = profile.strip_prefix(WSL_PREFIX) {
        ShellHost::Wsl(d.to_string())
    } else {
        ShellHost::Local
    }
}

/// `/home/me` style absolute path.
pub fn is_posix_abs(p: &str) -> bool {
    p.starts_with('/') || p == "~" || p.starts_with("~/")
}

/// `C:\x` or `C:/x`.
fn drive_path(p: &str) -> Option<(char, &str)> {
    let b = p.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':').then(|| ((b[0] as char).to_ascii_lowercase(), &p[2..]))
}

/// The distro and Linux path of a `\\wsl.localhost\<distro>\...` or `\\wsl$\<distro>\...` path.
pub fn wsl_unc(p: &str) -> Option<(String, String)> {
    let norm = p.replace('/', "\\");
    let lower = norm.to_ascii_lowercase();
    let rest = ["\\\\wsl.localhost\\", "\\\\wsl$\\"].iter().find_map(|pre| lower.starts_with(pre).then(|| &norm[pre.len()..]))?;
    let (distro, tail) = match rest.find('\\') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "\\"),
    };
    if distro.is_empty() {
        return None;
    }
    Some((distro.to_string(), tail.replace('\\', "/")))
}

/// A Linux path inside `distro` as a path Windows programs can open.
/// `/mnt/c/dev` -> `C:\dev`; `/home/me` -> `\\wsl.localhost\<distro>\home\me`.
pub fn wsl_to_windows(distro: &str, linux: &str) -> String {
    if let Some(rest) = linux.strip_prefix("/mnt/") {
        let b = rest.as_bytes();
        if !b.is_empty() && b[0].is_ascii_alphabetic() && (b.len() == 1 || b[1] == b'/') {
            let drive = (b[0] as char).to_ascii_uppercase();
            let tail = if b.len() > 1 { &rest[1..] } else { "/" };
            return format!("{drive}:{}", tail.replace('/', "\\"));
        }
    }
    let tail = linux.trim_end_matches('/').replace('/', "\\");
    format!("\\\\wsl.localhost\\{distro}{tail}")
}

/// A Windows path as WSL sees it. `C:\dev` -> `/mnt/c/dev`; `\\wsl.localhost\Ubuntu\x` -> `/x`
/// (only for the same distro, other UNC paths can't be reached).
pub fn windows_to_wsl(distro: &str, win: &str) -> Option<String> {
    if let Some((d, tail)) = wsl_unc(win) {
        return d.eq_ignore_ascii_case(distro).then_some(tail);
    }
    let (drive, rest) = drive_path(win)?;
    let rest = rest.replace('\\', "/");
    let rest = rest.trim_end_matches('/');
    Some(format!("/mnt/{drive}{}{rest}", if rest.starts_with('/') || rest.is_empty() { "" } else { "/" }))
}

/// Windows path as Git Bash (MSYS) sees it: `C:\a b` -> `/c/a b`.
pub fn windows_to_msys(win: &str) -> String {
    match drive_path(win) {
        Some((d, rest)) => format!("/{d}{}", rest.replace('\\', "/")),
        None => win.replace('\\', "/"),
    }
}

/// The local (Windows-openable) path for a pane's reported cwd, or None when it lives elsewhere.
pub fn local_path(profile: &str, cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    match shell_host(profile) {
        ShellHost::Ssh => None,
        ShellHost::Wsl(d) => {
            if cwd.starts_with('~') {
                None
            } else if cwd.starts_with('/') {
                Some(wsl_to_windows(&d, cwd))
            } else {
                Some(cwd.to_string())
            }
        }
        ShellHost::Local => (!cwd.starts_with('/') && !cwd.starts_with('~')).then(|| cwd.to_string()),
    }
}

/// Single-quote for POSIX shells: `it's` -> `'it'\''s'`. Plain words are left alone.
pub fn posix_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:@%,~".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Quote a path for PowerShell when needed.
pub fn pwsh_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || r"\/._-:".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "''"))
}

/// Quote a path for cmd.exe when needed.
pub fn cmd_quote(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '&', '(', ')', '^', '%', '!', ',', ';', '=']) {
        return s.to_string();
    }
    format!("\"{s}\"")
}

/// Join a remote directory and a file name.
pub fn posix_join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "~" {
        return if dir.is_empty() { name.to_string() } else { format!("~/{name}") };
    }
    format!("{}/{name}", dir.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_both_ways() {
        assert_eq!(wsl_to_windows("Ubuntu", "/mnt/c/dev/x"), r"C:\dev\x");
        assert_eq!(wsl_to_windows("Ubuntu", "/mnt/d"), r"D:\");
        assert_eq!(wsl_to_windows("Ubuntu", "/home/me/proj"), r"\\wsl.localhost\Ubuntu\home\me\proj");
        assert_eq!(wsl_to_windows("Ubuntu", "/"), r"\\wsl.localhost\Ubuntu");
        assert_eq!(wsl_to_windows("Ubuntu", "/mnt/wsl/x"), r"\\wsl.localhost\Ubuntu\mnt\wsl\x");
        assert_eq!(windows_to_wsl("Ubuntu", r"C:\dev\My App").as_deref(), Some("/mnt/c/dev/My App"));
        assert_eq!(windows_to_wsl("Ubuntu", r"C:\").as_deref(), Some("/mnt/c"));
        assert_eq!(windows_to_wsl("Ubuntu", r"\\wsl.localhost\Ubuntu\home\me").as_deref(), Some("/home/me"));
        assert_eq!(windows_to_wsl("ubuntu", r"\\wsl$\Ubuntu\home").as_deref(), Some("/home"));
        assert_eq!(windows_to_wsl("Debian", r"\\wsl.localhost\Ubuntu\home"), None);
        assert_eq!(windows_to_wsl("Ubuntu", r"\\server\share"), None);
        assert_eq!(wsl_unc(r"\\wsl.localhost\Ubuntu"), Some(("Ubuntu".into(), "/".into())));
    }

    #[test]
    fn local_paths_per_host() {
        assert_eq!(local_path("pwsh", r"C:\dev").as_deref(), Some(r"C:\dev"));
        assert_eq!(local_path("pwsh", "/home/x"), None);
        assert_eq!(local_path("wsl:Ubuntu", "/mnt/c/dev").as_deref(), Some(r"C:\dev"));
        assert_eq!(local_path("wsl:Ubuntu", "/home/me").as_deref(), Some(r"\\wsl.localhost\Ubuntu\home\me"));
        assert_eq!(local_path("wsl:Ubuntu", "~"), None);
        assert_eq!(local_path("ssh", "/home/me"), None);
        assert_eq!(shell_host("wsl:Ubuntu-22.04"), ShellHost::Wsl("Ubuntu-22.04".into()));
    }

    #[test]
    fn quoting() {
        assert_eq!(posix_quote("/home/me/x.txt"), "/home/me/x.txt");
        assert_eq!(posix_quote("a b's"), r"'a b'\''s'");
        assert_eq!(pwsh_quote(r"C:\x\y.txt"), r"C:\x\y.txt");
        assert_eq!(pwsh_quote(r"C:\it's here"), r"'C:\it''s here'");
        assert_eq!(cmd_quote(r"C:\a b"), r#""C:\a b""#);
        assert_eq!(windows_to_msys(r"C:\a b\c"), "/c/a b/c");
        assert_eq!(posix_join("/srv/app/", "f.txt"), "/srv/app/f.txt");
        assert_eq!(posix_join("~", "f.txt"), "~/f.txt");
        assert_eq!(posix_join("", "f.txt"), "f.txt");
    }
}
