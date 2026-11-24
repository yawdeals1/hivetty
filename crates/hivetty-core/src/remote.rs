//! Remote (ssh) workspaces: the connection a workspace's terminals, SOCKS tunnel and uploads use.
//! Hivetty drives the system OpenSSH client (`ssh.exe` / `scp.exe`), so keys, agents and
//! `~/.ssh/config` host aliases all work as they do in any other terminal.

use crate::paths::posix_quote;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Remote {
    /// ssh destination: `host`, `user@host` or a `~/.ssh/config` alias.
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Private key file (`ssh -i`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Extra ssh options, e.g. ["-J", "bastion"] or ["-o", "ForwardAgent=yes"].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

impl Remote {
    /// Parse `user@host`, `host:port`, `user@host:port` or `ssh://user@host:port`.
    pub fn parse(dest: &str) -> Result<Remote, String> {
        let s = dest.trim();
        let s = s.strip_prefix("ssh://").unwrap_or(s).trim_end_matches('/');
        if s.is_empty() || s.starts_with('-') || s.contains(char::is_whitespace) {
            return Err(format!("not an ssh destination: {dest:?}"));
        }
        let (userhost, port) = match s.rsplit_once(':') {
            // [::1]:22 / host:22 — but not a bare IPv6 address
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && (!h.contains(':') || h.ends_with(']')) => {
                (h, Some(p.parse::<u16>().map_err(|_| format!("bad port in {dest:?}"))?))
            }
            _ => (s, None),
        };
        let host_part = userhost.rsplit('@').next().unwrap_or("");
        if host_part.is_empty() || userhost.ends_with('@') {
            return Err(format!("missing host in {dest:?}"));
        }
        Ok(Remote { host: userhost.trim_start_matches('[').replace("]", "").replace("@[", "@"), port, identity: None, options: Vec::new() })
    }

    /// Host without the user part, for labels.
    pub fn hostname(&self) -> &str {
        self.host.rsplit('@').next().unwrap_or(&self.host)
    }

    /// Short label: `user@host` (plus `:port` when not 22).
    pub fn label(&self) -> String {
        match self.port {
            Some(p) if p != 22 => format!("{}:{p}", self.host),
            _ => self.host.clone(),
        }
    }

    /// Stable key for sharing one SOCKS tunnel / browser profile per connection.
    pub fn key(&self) -> String {
        let mut k = self.label();
        if let Some(i) = &self.identity {
            k.push('|');
            k.push_str(i);
        }
        for o in &self.options {
            k.push('|');
            k.push_str(o);
        }
        k
    }

    /// Connection options shared by ssh and scp (`port_flag` is "-p" for ssh, "-P" for scp).
    fn conn_args(&self, port_flag: &str) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(p) = self.port {
            a.push(port_flag.to_string());
            a.push(p.to_string());
        }
        if let Some(i) = &self.identity {
            a.push("-i".into());
            a.push(i.clone());
        }
        a.extend(self.options.iter().cloned());
        a
    }

    /// Arguments for an interactive terminal. Starts in `cwd` and/or runs `command` when given,
    /// then leaves the user in their login shell.
    pub fn terminal_args(&self, cwd: Option<&str>, command: Option<&str>) -> Vec<String> {
        let mut a = vec!["-t".to_string()];
        a.extend(self.conn_args("-p"));
        a.push(self.host.clone());
        let mut script = Vec::new();
        if let Some(d) = cwd.filter(|d| d.starts_with('/') || d.starts_with('~')) {
            // `~` must stay unquoted to expand
            let d = match d.strip_prefix("~/") {
                Some(rest) => format!("~/{}", posix_quote(rest)),
                None if d == "~" => "~".into(),
                None => posix_quote(d),
            };
            script.push(format!("cd {d} 2>/dev/null"));
        }
        if let Some(c) = command.filter(|c| !c.trim().is_empty()) {
            script.push(c.to_string());
        }
        if !script.is_empty() {
            script.push("exec ${SHELL:-sh} -l".into());
            a.push(script.join("; "));
        }
        a
    }

    /// Arguments for a background SOCKS5 proxy on 127.0.0.1:`port`.
    pub fn socks_args(&self, port: u16) -> Vec<String> {
        let mut a = vec![
            "-N".to_string(),
            "-D".into(),
            format!("127.0.0.1:{port}"),
            "-o".into(),
            "ExitOnForwardFailure=yes".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ServerAliveInterval=30".into(),
        ];
        a.extend(self.conn_args("-p"));
        a.push(self.host.clone());
        a
    }

    /// Arguments for uploading `files` into remote directory `dir` (empty = home).
    pub fn scp_args(&self, files: &[String], dir: &str) -> Vec<String> {
        let mut a = vec!["-r".to_string(), "-o".into(), "BatchMode=yes".into()];
        a.extend(self.conn_args("-P"));
        a.push("--".into());
        a.extend(files.iter().cloned());
        // scp (SFTP mode) takes the path literally; `~` works as the home prefix
        let dir = if dir.is_empty() { "~" } else { dir };
        a.push(format!("{}:{}/", self.host, dir.trim_end_matches('/')));
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_destinations() {
        let r = Remote::parse("me@box").unwrap();
        assert_eq!((r.host.as_str(), r.port), ("me@box", None));
        let r = Remote::parse("ssh://me@box:2222/").unwrap();
        assert_eq!((r.host.as_str(), r.port), ("me@box", Some(2222)));
        let r = Remote::parse("box:22").unwrap();
        assert_eq!((r.hostname(), r.port), ("box", Some(22)));
        let r = Remote::parse("me@[::1]:2200").unwrap();
        assert_eq!((r.host.as_str(), r.port), ("me@::1", Some(2200)));
        assert_eq!(Remote::parse("fe80::1").unwrap().port, None);
        assert!(Remote::parse("").is_err());
        assert!(Remote::parse("-oProxyCommand=x").is_err());
        assert!(Remote::parse("me@").is_err());
        assert!(Remote::parse("a b").is_err());
        assert_eq!(Remote::parse("me@box:2222").unwrap().label(), "me@box:2222");
    }

    #[test]
    fn args() {
        let mut r = Remote::parse("me@box:2222").unwrap();
        r.identity = Some(r"C:\k\id".into());
        assert_eq!(r.terminal_args(None, None), vec!["-t", "-p", "2222", "-i", r"C:\k\id", "me@box"]);
        let a = r.terminal_args(Some("/srv/my app"), Some("htop"));
        assert_eq!(a.last().unwrap(), "cd '/srv/my app' 2>/dev/null; htop; exec ${SHELL:-sh} -l");
        let a = r.terminal_args(Some("~/x y"), None);
        assert_eq!(a.last().unwrap(), "cd ~/'x y' 2>/dev/null; exec ${SHELL:-sh} -l");
        // Windows cwd from a local pane is ignored
        assert_eq!(r.terminal_args(Some(r"C:\dev"), None).last().unwrap(), "me@box");
        let s = r.socks_args(1080);
        assert!(s.windows(2).any(|w| w == ["-D", "127.0.0.1:1080"]));
        assert_eq!(s.last().unwrap(), "me@box");
        let c = r.scp_args(&[r"C:\a.txt".into()], "/srv/app/");
        assert_eq!(&c[c.len() - 3..], ["--", r"C:\a.txt", "me@box:/srv/app/"]);
        assert!(c.windows(2).any(|w| w == ["-P", "2222"]));
        assert_eq!(r.scp_args(&["f".into()], "").last().unwrap(), "me@box:~/");
    }
}
