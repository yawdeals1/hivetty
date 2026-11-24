//! Remote workspaces (`hivetty ssh`): SOCKS tunnels for their browser panes, and dropping files
//! onto terminals (paths are pasted, translated for WSL; ssh panes upload with scp first).

use crate::app::{emit_state, AppState};
use crate::browser::BrowserProxy;
use crate::config::data_dir;
use anyhow::{anyhow, Result};
use hivetty_core::layout::PaneId;
use hivetty_core::model::PaneKind;
use hivetty_core::paths::{self, shell_host, ShellHost};
use hivetty_core::remote::Remote;
use parking_lot::Mutex;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// wry's default WebView2 arguments, kept when we add proxy flags.
const WEBVIEW_DEFAULT_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

/// Windows' own OpenSSH first: it uses the Windows ssh-agent and Windows key paths, unlike
/// MSYS builds (Git for Windows) that may come earlier on PATH.
fn openssh_tool(name: &str) -> Result<PathBuf> {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let p = PathBuf::from(sys).join(r"System32\OpenSSH").join(name);
    if p.is_file() {
        return Ok(p);
    }
    if let Ok(p) = which::which(name) {
        return Ok(p);
    }
    Err(anyhow!("{name} not found. Install the OpenSSH Client (Settings → System → Optional features)."))
}

pub fn ssh_exe() -> Result<String> {
    openssh_tool("ssh.exe").map(|p| p.display().to_string())
}

fn scp_exe() -> Result<PathBuf> {
    openssh_tool("scp.exe")
}

struct Tunnel {
    port: u16,
    child: Option<Child>,
}

/// One `ssh -N -D` SOCKS proxy per remote connection, shared by its workspaces' browser panes.
#[derive(Default)]
pub struct Tunnels {
    map: Mutex<HashMap<String, Tunnel>>,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(1080)
}

impl Tunnels {
    /// Start (or reuse) the tunnel for `r`; returns its local port. The port stays the same for
    /// the app's lifetime so the browser profile's proxy setting stays valid across restarts.
    pub fn ensure(&self, r: &Remote) -> Result<u16> {
        let mut m = self.map.lock();
        let t = m.entry(r.key()).or_insert_with(|| Tunnel { port: free_port(), child: None });
        let alive = t.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        if !alive {
            let child = Command::new(ssh_exe()?)
                .args(r.socks_args(t.port))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|e| anyhow!("starting ssh tunnel: {e}"))?;
            t.child = Some(child);
        }
        Ok(t.port)
    }

    /// Wait until the proxy accepts connections, or report why ssh gave up.
    pub fn wait_ready(&self, r: &Remote, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            let port = {
                let mut m = self.map.lock();
                let t = m.get_mut(&r.key()).ok_or("tunnel stopped")?;
                if let Some(c) = t.child.as_mut() {
                    if let Ok(Some(status)) = c.try_wait() {
                        let mut err = String::new();
                        if let Some(mut e) = c.stderr.take() {
                            let _ = e.read_to_string(&mut err);
                        }
                        t.child = None;
                        let err = err.trim();
                        return Err(if err.is_empty() { format!("ssh exited ({status})") } else { err.to_string() });
                    }
                }
                t.port
            };
            if std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(200)).is_ok() {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err("timed out waiting for the ssh tunnel".into());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Stop tunnels no workspace uses anymore.
    pub fn retain(&self, used: &HashSet<String>) {
        let mut m = self.map.lock();
        for (k, t) in m.iter_mut() {
            if !used.contains(k) {
                if let Some(mut c) = t.child.take() {
                    let _ = c.kill();
                }
            }
        }
    }

    pub fn stop_all(&self) {
        self.retain(&HashSet::new());
    }
}

pub fn prune_tunnels(app: &AppHandle) {
    let st = app.state::<AppState>();
    let used: HashSet<String> = st.model.lock().workspaces.iter().filter_map(|w| w.remote.as_ref().map(|r| r.key())).collect();
    st.tunnels.retain(&used);
}

fn profile_dir(r: &Remote) -> PathBuf {
    let safe: String = r.key().chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).take(80).collect();
    data_dir().join("webview").join(format!("remote-{safe}"))
}

/// Create a browser pane. In an ssh workspace it gets its own WebView2 profile whose traffic
/// (including `localhost`) goes through the host's SOCKS tunnel.
pub fn start_browser(app: &AppHandle, pane: PaneId, url: &str, remote: Option<&Remote>) -> Result<()> {
    let st = app.state::<AppState>();
    let Some(r) = remote else {
        return st.browsers.create(app, pane, url, None).map_err(|e| anyhow!(e));
    };
    let port = st.tunnels.ensure(r)?;
    let proxy = BrowserProxy {
        data_dir: profile_dir(r),
        args: format!("{WEBVIEW_DEFAULT_ARGS} --proxy-server=socks5://127.0.0.1:{port} --proxy-bypass-list=<-loopback>"),
    };
    st.browsers.create(app, pane, "about:blank", Some(proxy)).map_err(|e| anyhow!(e))?;
    let (app, r, url) = (app.clone(), r.clone(), url.to_string());
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let target = match st.tunnels.wait_ready(&r, Duration::from_secs(20)) {
            Ok(()) => url,
            Err(e) => error_page(&r, &e),
        };
        let _ = st.browsers.navigate(&app, pane, &target);
    });
    Ok(())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn error_page(r: &Remote, err: &str) -> String {
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Tunnel failed</title><body style=\"font:14px system-ui;background:#111317;color:#ddd;padding:32px\">\
         <h2 style=\"font-weight:600\">Couldn't open the SSH tunnel to {}</h2><pre style=\"white-space:pre-wrap;color:#f88\">{}</pre>\
         <p>Browser panes in remote workspaces route through <code>ssh -D</code> without a terminal, so the host must accept \
         key or agent authentication. Close this pane and open a new one to retry.</p>",
        html_escape(&r.label()),
        html_escape(err)
    );
    let enc: String = html
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b' ' | b'<' | b'>' | b'=' | b'/' | b':' | b';' | b',' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect::<String>()
        .replace(' ', "%20");
    format!("data:text/html;charset=utf-8,{enc}")
}

/// Transient status line shown on a pane (uploads).
fn status(app: &AppHandle, pane: PaneId, text: &str, kind: &str) {
    let _ = app.emit("pane-status", json!({ "pane": pane, "text": text, "kind": kind }));
}

/// Files dropped onto a terminal: paste their paths in the shell's syntax. Over ssh they are
/// uploaded to the pane's remote directory first and the remote paths are pasted.
pub fn drop_files(app: &AppHandle, pane: PaneId, files: Vec<String>) -> Result<()> {
    let st = app.state::<AppState>();
    let files: Vec<String> = files.into_iter().filter(|f| !f.is_empty()).collect();
    if files.is_empty() {
        return Ok(());
    }
    let (profile, cwd, remote) = {
        let m = st.model.lock();
        let p = m.panes.get(&pane).ok_or_else(|| anyhow!("no pane"))?;
        let PaneKind::Terminal { profile, .. } = &p.kind else { return Ok(()) };
        (profile.clone(), p.cwd.clone(), m.workspace_of(pane).and_then(|w| w.remote.clone()))
    };
    let text = match shell_host(&profile) {
        ShellHost::Ssh => {
            let r = remote.ok_or_else(|| anyhow!("this workspace has no ssh connection"))?;
            let dir = cwd.filter(|c| paths::is_posix_abs(c)).unwrap_or_default();
            upload(app, pane, r, files, dir);
            return Ok(());
        }
        ShellHost::Wsl(distro) => files.iter().map(|f| paths::posix_quote(&paths::windows_to_wsl(&distro, f).unwrap_or_else(|| f.replace('\\', "/")))).collect::<Vec<_>>(),
        ShellHost::Local => {
            let shell = st.profiles.lock().iter().find(|p| p.id == profile).map(|p| p.shell.clone()).unwrap_or_default();
            files
                .iter()
                .map(|f| match shell.as_str() {
                    "bash" => paths::posix_quote(&paths::windows_to_msys(f)),
                    "cmd" => paths::cmd_quote(f),
                    _ => paths::pwsh_quote(f),
                })
                .collect()
        }
    };
    st.ptys.write(pane, format!("{} ", text.join(" ")).as_bytes());
    Ok(())
}

/// Upload with scp in the background, then paste the remote paths into the pane.
pub fn upload(app: &AppHandle, pane: PaneId, r: Remote, files: Vec<String>, dir: String) {
    let app = app.clone();
    std::thread::spawn(move || {
        let names: Vec<String> = files.iter().map(|f| std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| f.clone())).collect();
        let shown_dir = if dir.is_empty() { "~".to_string() } else { dir.clone() };
        let what = if names.len() == 1 { names[0].clone() } else { format!("{} files", names.len()) };
        status(&app, pane, &format!("Uploading {what} to {}:{shown_dir} …", r.hostname()), "busy");
        let result = scp_exe().map_err(|e| e.to_string()).and_then(|scp| {
            Command::new(scp)
                .args(r.scp_args(&files, &dir))
                .stdin(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .map_err(|e| e.to_string())
        });
        let st = app.state::<AppState>();
        match result {
            Ok(out) if out.status.success() => {
                status(&app, pane, &format!("Uploaded {what} to {}:{shown_dir}", r.hostname()), "done");
                let pasted: Vec<String> = names.iter().map(|n| paths::posix_quote(&paths::posix_join(&dir, n)).replace("'~/", "~/'")).collect();
                st.ptys.write(pane, format!("{} ", pasted.join(" ")).as_bytes());
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("scp failed").trim().to_string();
                status(&app, pane, &format!("Upload failed: {line}"), "error");
            }
            Err(e) => status(&app, pane, &format!("Upload failed: {e}"), "error"),
        }
        emit_state(&app);
    });
}
