//! Client for the detached session host (`hivetty-host.exe`). With persistent sessions on, the
//! host owns the ConPTYs; this connection forwards input/resizes to it and feeds its output into
//! each pane's [`Sink`], exactly like a local ConPTY reader would.

use crate::pty::{Sink, SpawnSpec};
use anyhow::{anyhow, bail, Context, Result};
use hivetty_core::hostproto::{self, Frame};
use hivetty_core::layout::PaneId;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

const PROTO: u64 = 1;
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

pub struct HostClient {
    tx: UnboundedSender<Vec<u8>>,
    sinks: Mutex<HashMap<PaneId, Arc<Sink>>>,
    pending: Mutex<HashMap<u64, std::sync::mpsc::Sender<Value>>>,
    next: AtomicU64,
    alive: AtomicBool,
}

pub struct Attached {
    pub exited: bool,
    pub code: Option<i32>,
    pub pid: Option<u32>,
}

pub fn pipe_name() -> String {
    hostproto::host_pipe_name(&std::env::var("USERNAME").unwrap_or_else(|_| "user".into()))
}

/// The session host to start: `HIVETTY_HOST_EXE`, or a per-version copy of the shipped
/// `hivetty-host.exe` (see [`crate::bundled`]) so a host that outlives the app never locks the
/// install folder against updates.
pub fn host_exe() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("HIVETTY_HOST_EXE") {
        return Some(p.into());
    }
    let sub = format!("host-{}", env!("CARGO_PKG_VERSION"));
    let exe = crate::bundled::installed_copy("hivetty-host.exe", &sub)?;
    crate::bundled::prune_versions("host-", &sub);
    Some(exe)
}

const ERROR_PIPE_BUSY: i32 = 231;

impl HostClient {
    /// Connect to a running host, or None.
    pub fn connect_existing() -> Option<Arc<Self>> {
        match Self::connect(Duration::from_millis(500)) {
            Ok(c) => Some(c),
            Err(e) => {
                if std::env::var_os("HIVETTY_DEBUG_HOST").is_some() {
                    eprintln!("[host] no running host: {e:#}");
                }
                None
            }
        }
    }

    /// Connect, starting the host process first if none is running.
    pub fn connect_or_start() -> Result<Arc<Self>> {
        if let Some(c) = Self::connect_existing() {
            return Ok(c);
        }
        let exe = host_exe().ok_or_else(|| anyhow!("hivetty-host.exe not found (reinstall Hivetty)"))?;
        start_detached(&exe)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match Self::connect(Duration::from_millis(300)) {
                Ok(c) => return Ok(c),
                Err(e) if Instant::now() > deadline => return Err(e.context("session host didn't start")),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    fn connect(timeout: Duration) -> Result<Arc<Self>> {
        let name = pipe_name();
        let rt = tauri::async_runtime::handle();
        let _guard = rt.inner().enter();
        let start = Instant::now();
        let pipe = loop {
            match ClientOptions::new().open(&name) {
                Ok(p) => break p,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(e).with_context(|| format!("connecting to {name}")),
            }
        };
        let (mut r, mut w) = tokio::io::split(pipe);
        let (tx, mut rx) = unbounded_channel::<Vec<u8>>();
        let client = Arc::new(HostClient { tx, sinks: Mutex::default(), pending: Mutex::default(), next: AtomicU64::new(1), alive: AtomicBool::new(true) });
        tauri::async_runtime::spawn(async move {
            while let Some(buf) = rx.recv().await {
                if w.write_all(&buf).await.is_err() {
                    break;
                }
            }
        });
        {
            let c = client.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let mut len = [0u8; 4];
                    if r.read_exact(&mut len).await.is_err() {
                        break;
                    }
                    let Ok(n) = hostproto::frame_len(len) else { break };
                    let mut body = vec![0u8; n];
                    if r.read_exact(&mut body).await.is_err() {
                        break;
                    }
                    match hostproto::decode_body(&body) {
                        Ok(Frame::Data(pane, data)) => {
                            let sink = c.sinks.lock().get(&pane).cloned();
                            if let Some(s) = sink {
                                s.ingest(&data);
                            }
                        }
                        Ok(Frame::Control(v)) => c.on_control(v),
                        Err(_) => break,
                    }
                }
                c.lost();
            });
        }
        let hello = client.call("hello", json!({})).context("session host handshake")?;
        if hello["proto"].as_u64() != Some(PROTO) {
            bail!("session host speaks protocol {}, expected {PROTO}", hello["proto"]);
        }
        Ok(client)
    }

    fn on_control(&self, v: Value) {
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            if let Some(tx) = self.pending.lock().remove(&id) {
                let _ = tx.send(v);
            }
            return;
        }
        if v["event"] == "exit" {
            let pane = v["pane"].as_str().and_then(|s| uuid::Uuid::parse_str(s).ok());
            let code = v["code"].as_i64().map(|c| c as i32);
            if let Some(sink) = pane.and_then(|p| self.sinks.lock().get(&p).cloned()) {
                sink.exited(code);
            }
        }
    }

    /// Connection dropped (host crashed or was stopped): every hosted shell is gone.
    fn lost(&self) {
        if !self.alive.swap(false, Ordering::SeqCst) {
            return;
        }
        self.pending.lock().clear();
        let sinks: Vec<_> = self.sinks.lock().drain().map(|(_, s)| s).collect();
        for s in sinks {
            s.exited(None);
        }
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn call(&self, op: &str, mut params: Value) -> Result<Value> {
        if !self.alive() {
            bail!("session host disconnected");
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending.lock().insert(id, tx);
        params["id"] = json!(id);
        params["op"] = json!(op);
        if self.tx.send(hostproto::encode_control(&params)).is_err() {
            self.pending.lock().remove(&id);
            bail!("session host disconnected");
        }
        let v = rx.recv_timeout(CALL_TIMEOUT).map_err(|_| {
            self.pending.lock().remove(&id);
            anyhow!("session host didn't answer `{op}`")
        })?;
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            bail!("{e}");
        }
        Ok(v)
    }

    fn send(&self, op: &str, mut params: Value) {
        params["op"] = json!(op);
        let _ = self.tx.send(hostproto::encode_control(&params));
    }

    pub fn register(&self, sink: Arc<Sink>) {
        self.sinks.lock().insert(sink.pane(), sink);
    }

    /// Stop routing output for `pane` (only if `generation` is still the registered one).
    pub fn unregister(&self, pane: PaneId, generation: u64) {
        let mut m = self.sinks.lock();
        if m.get(&pane).is_some_and(|s| s.generation() == generation) {
            m.remove(&pane);
        }
    }

    pub fn spawn(&self, pane: PaneId, spec: &SpawnSpec, env: &[(String, String)], meta: Value) -> Result<Option<u32>> {
        let v = self.call(
            "spawn",
            json!({
                "pane": pane,
                "command": spec.command,
                "args": spec.args,
                "cwd": spec.cwd,
                "env": env.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                "cols": spec.cols,
                "rows": spec.rows,
                "meta": meta,
            }),
        )?;
        Ok(v["pid"].as_u64().map(|p| p as u32))
    }

    pub fn attach(&self, pane: PaneId) -> Result<Attached> {
        let v = self.call("attach", json!({ "pane": pane }))?;
        Ok(Attached { exited: v["exited"].as_bool().unwrap_or(false), code: v["code"].as_i64().map(|c| c as i32), pid: v["pid"].as_u64().map(|p| p as u32) })
    }

    pub fn list(&self) -> Result<Vec<Value>> {
        let v = self.call("list", json!({}))?;
        Ok(v["sessions"].as_array().cloned().unwrap_or_default())
    }

    pub fn write(&self, pane: PaneId, data: &[u8]) -> bool {
        self.alive() && self.tx.send(hostproto::encode_data(pane, data)).is_ok()
    }

    pub fn resize(&self, pane: PaneId, cols: u16, rows: u16) {
        self.send("resize", json!({ "pane": pane, "cols": cols, "rows": rows }));
    }

    pub fn kill(&self, pane: PaneId) {
        self.send("kill", json!({ "pane": pane }));
    }

    /// Wait until everything sent so far has been processed.
    pub fn flush(&self) {
        let _ = self.call("hello", json!({}));
    }
}

fn start_detached(exe: &std::path::Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let mk = |flags: u32| {
        let mut c = Command::new(exe);
        // Its working directory would otherwise be ours, keeping that folder in use.
        if let Some(dir) = exe.parent() {
            c.current_dir(dir);
        }
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(flags);
        c
    };
    // Break out of any job object (terminals, IDEs) so closing Hivetty's launcher doesn't take
    // the host down; not every job allows it.
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    match mk(base | CREATE_BREAKAWAY_FROM_JOB).spawn() {
        Ok(_) => Ok(()),
        Err(_) => mk(base).spawn().map(|_| ()).with_context(|| format!("starting {}", exe.display())),
    }
}
