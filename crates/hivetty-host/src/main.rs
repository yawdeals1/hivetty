//! `hivetty-host`: the detached session host. When persistent sessions are on, Hivetty asks this
//! process to own its ConPTYs, so shells (and the agents running in them) keep running after the
//! app closes. The next launch reattaches and replays what they printed meanwhile.
//!
//! One host per user, reachable on `\\.\pipe\hivetty-host-<user>` (local clients only). It exits
//! on its own once no client is connected and no shell is left running.

#![cfg_attr(not(test), windows_subsystem = "windows")]

use hivetty_core::hostproto::{self, Frame};
use hivetty_core::osc::{OscTap, TermEvent};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use uuid::Uuid;

const REPLAY_CAP: usize = 2 * 1024 * 1024;
const PROTO: u64 = 1;

type Tx = UnboundedSender<Vec<u8>>;

struct Out {
    replay: VecDeque<u8>,
    /// (connection id, sender) of the attached client.
    sub: Option<(u64, Tx)>,
}

struct Sess {
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: std::sync::mpsc::Sender<Vec<u8>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    out: Mutex<Out>,
    screen: Mutex<vt100::Parser>,
    pid: Option<u32>,
    meta: Value,
    created: u64,
    /// Some(code) once the process has exited.
    exit: Mutex<Option<Option<i32>>>,
    killed: AtomicBool,
}

#[derive(Default)]
struct Host {
    sessions: Mutex<HashMap<Uuid, Arc<Sess>>>,
    clients: AtomicUsize,
    next_conn: AtomicU64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn log(msg: &str) {
    if let Some(p) = std::env::var_os("HIVETTY_HOST_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "[{}] {msg}", now_ms());
        }
    }
}

fn main() {
    let mut pipe = None;
    let mut idle = Duration::from_secs(20);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pipe" => pipe = args.next(),
            "--idle-exit" => idle = Duration::from_secs(args.next().and_then(|s| s.parse().ok()).unwrap_or(20)),
            "--version" => {
                println!("hivetty-host {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => {}
        }
    }
    let pipe = pipe.unwrap_or_else(|| hostproto::host_pipe_name(&std::env::var("USERNAME").unwrap_or_else(|_| "user".into())));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
    rt.block_on(serve(pipe, idle));
}

async fn serve(pipe: String, idle: Duration) {
    let mut server = match ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(&pipe) {
        Ok(s) => s,
        Err(e) => {
            // Another host already owns the pipe.
            log(&format!("pipe {pipe} unavailable: {e}"));
            return;
        }
    };
    log(&format!("host {} listening on {pipe}", std::process::id()));
    let host = Arc::new(Host::default());
    {
        let host = host.clone();
        tokio::spawn(async move { idle_watch(host, idle).await });
    }
    loop {
        if server.connect().await.is_err() {
            continue;
        }
        let conn = server;
        server = match ServerOptions::new().reject_remote_clients(true).create(&pipe) {
            Ok(s) => s,
            Err(e) => {
                log(&format!("pipe re-create failed: {e}"));
                std::process::exit(1);
            }
        };
        let host = host.clone();
        tokio::spawn(async move { client(host, conn).await });
    }
}

/// Exit once nobody is connected and nothing is running for `idle`.
async fn idle_watch(host: Arc<Host>, idle: Duration) {
    let mut since = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let live = host.sessions.lock().values().any(|s| s.exit.lock().is_none());
        if host.clients.load(Ordering::SeqCst) > 0 || live {
            since = Instant::now();
        } else if since.elapsed() >= idle {
            log("idle, exiting");
            std::process::exit(0);
        }
    }
}

async fn client(host: Arc<Host>, conn: NamedPipeServer) {
    let id = host.next_conn.fetch_add(1, Ordering::SeqCst) + 1;
    host.clients.fetch_add(1, Ordering::SeqCst);
    log(&format!("client {id} connected"));
    let (mut r, mut w) = tokio::io::split(conn);
    let (tx, mut rx) = unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(async move {
        while let Some(buf) = rx.recv().await {
            if w.write_all(&buf).await.is_err() {
                break;
            }
        }
    });
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
                let s = host.sessions.lock().get(&pane).cloned();
                if let Some(s) = s {
                    let _ = s.input.send(data);
                }
            }
            Ok(Frame::Control(req)) => {
                let rid = req.get("id").cloned();
                let reply = match handle(&host, id, &tx, &req) {
                    Ok(mut v) => {
                        v["ok"] = json!(true);
                        v
                    }
                    Err(e) => json!({ "error": e }),
                };
                if let Some(rid) = rid {
                    let mut reply = reply;
                    reply["id"] = rid;
                    let _ = tx.send(hostproto::encode_control(&reply));
                }
                if req["op"] == "shutdown" {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    std::process::exit(0);
                }
            }
            Err(e) => {
                log(&format!("client {id}: {e}"));
                break;
            }
        }
    }
    // detach everything this client had attached
    for s in host.sessions.lock().values() {
        let mut o = s.out.lock();
        if o.sub.as_ref().is_some_and(|(c, _)| *c == id) {
            o.sub = None;
        }
    }
    drop(tx);
    writer.abort();
    host.clients.fetch_sub(1, Ordering::SeqCst);
    log(&format!("client {id} disconnected"));
}

fn pane_of(req: &Value) -> Result<Uuid, String> {
    req["pane"].as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| "pane id required".to_string())
}

fn handle(host: &Arc<Host>, conn: u64, tx: &Tx, req: &Value) -> Result<Value, String> {
    match req["op"].as_str().unwrap_or("") {
        "hello" => Ok(json!({
            "proto": PROTO,
            "version": env!("CARGO_PKG_VERSION"),
            "pid": std::process::id(),
            "sessions": host.sessions.lock().len(),
        })),
        "spawn" => {
            let pane = pane_of(req)?;
            if let Some(old) = host.sessions.lock().remove(&pane) {
                kill(&old);
            }
            let s = spawn(host, pane, req, Some((conn, tx.clone()))).map_err(|e| format!("{e:#}"))?;
            Ok(json!({ "pid": s.pid }))
        }
        "attach" => {
            let pane = pane_of(req)?;
            let s = host.sessions.lock().get(&pane).cloned().ok_or("no such session")?;
            {
                let mut o = s.out.lock();
                let (a, b) = o.replay.as_slices();
                let mut all = Vec::with_capacity(a.len() + b.len());
                all.extend_from_slice(a);
                all.extend_from_slice(b);
                for chunk in all.chunks(1 << 20) {
                    let _ = tx.send(hostproto::encode_data(pane, chunk));
                }
                o.sub = Some((conn, tx.clone()));
            }
            let exit = *s.exit.lock();
            Ok(json!({ "exited": exit.is_some(), "code": exit.flatten(), "pid": s.pid }))
        }
        "detach" => {
            let pane = pane_of(req)?;
            if let Some(s) = host.sessions.lock().get(&pane) {
                let mut o = s.out.lock();
                if o.sub.as_ref().is_some_and(|(c, _)| *c == conn) {
                    o.sub = None;
                }
            }
            Ok(json!({}))
        }
        "resize" => {
            let pane = pane_of(req)?;
            let cols = req["cols"].as_u64().unwrap_or(80).clamp(2, 1000) as u16;
            let rows = req["rows"].as_u64().unwrap_or(24).clamp(2, 1000) as u16;
            if let Some(s) = host.sessions.lock().get(&pane).cloned() {
                let _ = s.master.lock().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                s.screen.lock().screen_mut().set_size(rows, cols);
            }
            Ok(json!({}))
        }
        "kill" => {
            let pane = pane_of(req)?;
            let s = host.sessions.lock().remove(&pane);
            if let Some(s) = s {
                kill(&s);
            }
            Ok(json!({}))
        }
        "list" => {
            let list: Vec<Value> = host
                .sessions
                .lock()
                .iter()
                .map(|(id, s)| {
                    let exit = *s.exit.lock();
                    json!({
                        "pane": id,
                        "pid": s.pid,
                        "exited": exit.is_some(),
                        "code": exit.flatten(),
                        "attached": s.out.lock().sub.is_some(),
                        "meta": s.meta,
                        "created": s.created,
                    })
                })
                .collect();
            Ok(json!({ "sessions": list }))
        }
        "shutdown" => {
            let all: Vec<_> = host.sessions.lock().drain().map(|(_, s)| s).collect();
            for s in all {
                kill(&s);
            }
            Ok(json!({}))
        }
        op => Err(format!("unknown op: {op}")),
    }
}

fn kill(s: &Sess) {
    s.killed.store(true, Ordering::SeqCst);
    let _ = s.killer.lock().kill();
}

/// `sub`: the client that asked; it is attached from the first byte of output.
fn spawn(host: &Arc<Host>, pane: Uuid, req: &Value, sub: Option<(u64, Tx)>) -> anyhow_lite::Result<Arc<Sess>> {
    let command = req["command"].as_str().ok_or_else(|| anyhow_lite::msg("command required"))?;
    let cols = req["cols"].as_u64().unwrap_or(120).clamp(2, 1000) as u16;
    let rows = req["rows"].as_u64().unwrap_or(30).clamp(2, 1000) as u16;
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = native_pty_system().openpty(size).map_err(anyhow_lite::from)?;
    let mut cmd = CommandBuilder::new(command);
    for a in req["args"].as_array().into_iter().flatten() {
        cmd.arg(a.as_str().unwrap_or_default());
    }
    let cwd = req["cwd"].as_str().filter(|c| std::path::Path::new(c).is_dir()).map(str::to_string).or_else(|| std::env::var("USERPROFILE").ok());
    if let Some(c) = cwd {
        cmd.cwd(c);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "Hivetty");
    for kv in req["env"].as_array().into_iter().flatten() {
        if let (Some(k), Some(v)) = (kv[0].as_str(), kv[1].as_str()) {
            cmd.env(k, v);
        }
    }
    let mut child = pair.slave.spawn_command(cmd).map_err(anyhow_lite::from)?;
    drop(pair.slave);
    let pid = child.process_id();
    let killer = child.clone_killer();
    let mut reader = pair.master.try_clone_reader().map_err(anyhow_lite::from)?;
    let mut writer = pair.master.take_writer().map_err(anyhow_lite::from)?;
    let (input, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::Builder::new()
        .name(format!("write-{pane}"))
        .spawn(move || {
            while let Ok(data) = input_rx.recv() {
                if writer.write_all(&data).and_then(|_| writer.flush()).is_err() {
                    break;
                }
            }
        })
        .map_err(anyhow_lite::from)?;
    let s = Arc::new(Sess {
        master: Mutex::new(pair.master),
        input: input.clone(),
        killer: Mutex::new(killer),
        out: Mutex::new(Out { replay: VecDeque::new(), sub }),
        screen: Mutex::new(vt100::Parser::new(rows, cols, 0)),
        pid,
        meta: req.get("meta").cloned().unwrap_or(Value::Null),
        created: now_ms(),
        exit: Mutex::new(None),
        killed: AtomicBool::new(false),
    });
    {
        let s = s.clone();
        std::thread::Builder::new()
            .name(format!("read-{pane}"))
            .spawn(move || {
                let mut tap = OscTap::new();
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let chunk = &buf[..n];
                    let cpr = tap.feed(chunk).iter().any(|e| *e == TermEvent::CursorPositionRequest);
                    let cursor = {
                        let mut sc = s.screen.lock();
                        sc.process(chunk);
                        sc.screen().cursor_position()
                    };
                    // The host always answers cursor-position queries: ConPTY waits for one at
                    // startup, and nobody else may be attached.
                    if cpr {
                        let _ = input.send(format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1).into_bytes());
                    }
                    let mut o = s.out.lock();
                    o.replay.extend(chunk);
                    let over = o.replay.len().saturating_sub(REPLAY_CAP);
                    if over > 0 {
                        o.replay.drain(..over);
                    }
                    if let Some((_, tx)) = &o.sub {
                        if tx.send(hostproto::encode_data(pane, chunk)).is_err() {
                            o.sub = None;
                        }
                    }
                }
            })
            .map_err(anyhow_lite::from)?;
    }
    {
        let s = s.clone();
        std::thread::Builder::new()
            .name(format!("wait-{pane}"))
            .spawn(move || {
                let code = child.wait().ok().map(|st| st.exit_code() as i32);
                *s.exit.lock() = Some(code);
                if s.killed.load(Ordering::SeqCst) {
                    return;
                }
                // give the reader a moment to forward the last output first
                std::thread::sleep(Duration::from_millis(50));
                let o = s.out.lock();
                if let Some((_, tx)) = &o.sub {
                    let _ = tx.send(hostproto::encode_control(&json!({ "event": "exit", "pane": pane, "code": code })));
                }
            })
            .map_err(anyhow_lite::from)?;
    }
    host.sessions.lock().insert(pane, s.clone());
    log(&format!("spawned {pane} pid={pid:?} {command}"));
    Ok(s)
}

/// Minimal error type so the host doesn't need anyhow.
mod anyhow_lite {
    pub struct Error(pub String);
    pub type Result<T> = std::result::Result<T, Error>;
    pub fn msg(s: &str) -> Error {
        Error(s.to_string())
    }
    pub fn from<E: std::fmt::Display>(e: E) -> Error {
        Error(e.to_string())
    }
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
}
