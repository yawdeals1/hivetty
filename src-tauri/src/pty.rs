//! Terminal sessions. Each session keeps:
//! - a replay buffer so a (re)attaching UI view gets recent output,
//! - a vt100 screen mirror so the API can read the screen without the UI,
//! - an OSC tap that turns escape sequences into app events.
//!
//! The ConPTY itself lives either in this process ("local") or in the detached session host
//! (`hivetty-host.exe`, see [`crate::host`]) when persistent sessions are on. Both feed the same
//! [`Sink`], so the rest of the app can't tell them apart.

use crate::host::HostClient;
use hivetty_core::layout::PaneId;
use hivetty_core::osc::{OscTap, TermEvent};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};
use tauri::ipc::{Channel, InvokeResponseBody};

const REPLAY_CAP: usize = 2 * 1024 * 1024;
/// Output for the UI is coalesced into at most one IPC message per pane per this interval (one
/// display frame). A pane that has been quiet that long flushes immediately, so keystroke echo
/// isn't delayed. Each message costs a WebView2 round trip, so under heavy output fewer, larger
/// messages cut CPU use several-fold (measured: 4 ms → 16 ms).
const FLUSH_EVERY: Duration = Duration::from_millis(16);

/// FLUSH_EVERY, or zero (send every read as it comes) with HIVETTY_DEBUG_NO_BATCH, for A/B tests.
fn flush_every() -> Duration {
    static D: OnceLock<Duration> = OnceLock::new();
    *D.get_or_init(|| if std::env::var_os("HIVETTY_DEBUG_NO_BATCH").is_some() { Duration::ZERO } else { FLUSH_EVERY })
}

#[derive(Clone)]
pub struct SpawnSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

struct Output {
    replay: VecDeque<u8>,
    subscriber: Option<Channel<InvokeResponseBody>>,
    /// Bytes not yet sent to the subscriber.
    pending: Vec<u8>,
    last_flush: Instant,
    /// A delayed flush is queued on the flusher thread.
    scheduled: bool,
}

impl Output {
    fn flush(&mut self) {
        self.scheduled = false;
        self.last_flush = Instant::now();
        if self.pending.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.pending);
        if let Some(ch) = &self.subscriber {
            if ch.send(InvokeResponseBody::Raw(data)).is_err() {
                self.subscriber = None;
            }
        }
    }
}

/// One thread runs every pane's delayed flushes, in deadline order (deadlines are all
/// "now + FLUSH_EVERY" when queued, so FIFO order is deadline order).
fn flusher() -> &'static std::sync::mpsc::Sender<(Weak<Sink>, Instant)> {
    static TX: OnceLock<std::sync::mpsc::Sender<(Weak<Sink>, Instant)>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<(Weak<Sink>, Instant)>();
        std::thread::Builder::new()
            .name("pty-flush".into())
            .spawn(move || {
                while let Ok((sink, due)) = rx.recv() {
                    let now = Instant::now();
                    if due > now {
                        std::thread::sleep(due - now);
                    }
                    if let Some(s) = sink.upgrade() {
                        s.output.lock().flush();
                    }
                }
            })
            .expect("spawn pty flusher");
        tx
    })
}

/// Callbacks into the app from reader/wait threads.
pub trait PtyEvents: Send + Sync + 'static {
    fn on_events(&self, pane: PaneId, events: Vec<TermEvent>);
    fn on_exit(&self, pane: PaneId, code: Option<i32>);
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

/// Where a session's output goes: OSC tap, screen mirror, replay buffer and the UI channel.
pub struct Sink {
    pane: PaneId,
    /// Distinguishes a restarted pane's new session from the old one (late exit events).
    generation: u64,
    tap: Mutex<OscTap>,
    screen: Mutex<vt100::Parser>,
    output: Mutex<Output>,
    events: Arc<dyn PtyEvents>,
    /// Local sessions answer cursor-position queries here; the host answers its own.
    cpr_reply: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    /// Reattaching to a host session: its replayed output must not re-fire notifications.
    pub replaying: AtomicBool,
    raw: Mutex<Option<std::fs::File>>,
}

impl Sink {
    fn new(pane: PaneId, rows: u16, cols: u16, events: Arc<dyn PtyEvents>, cpr_reply: Option<std::sync::mpsc::Sender<Vec<u8>>>) -> Arc<Self> {
        // HIVETTY_DEBUG_RAW=<dir>: append each pane's raw output to <dir>/<pane>.raw
        let raw = std::env::var_os("HIVETTY_DEBUG_RAW")
            .and_then(|d| std::fs::OpenOptions::new().create(true).append(true).open(std::path::Path::new(&d).join(format!("{pane}.raw"))).ok());
        Arc::new(Sink {
            pane,
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            tap: Mutex::new(OscTap::new()),
            screen: Mutex::new(vt100::Parser::new(rows.max(2), cols.max(2), 5000)),
            output: Mutex::new(Output { replay: VecDeque::new(), subscriber: None, pending: Vec::new(), last_flush: Instant::now(), scheduled: false }),
            events,
            cpr_reply,
            replaying: AtomicBool::new(false),
            raw: Mutex::new(raw),
        })
    }

    pub fn pane(&self) -> PaneId {
        self.pane
    }

    pub fn ingest(self: &Arc<Self>, chunk: &[u8]) {
        if let Some(f) = self.raw.lock().as_mut() {
            let _ = f.write_all(chunk);
        }
        let mut evs = self.tap.lock().feed(chunk);
        let cursor = {
            let mut sc = self.screen.lock();
            sc.process(chunk);
            sc.screen().cursor_position()
        };
        // Answer cursor-position queries here so panes work with or without a UI attached.
        evs.retain(|e| {
            if *e == TermEvent::CursorPositionRequest {
                if let Some(r) = &self.cpr_reply {
                    let _ = r.send(format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1).into_bytes());
                }
                false
            } else {
                true
            }
        });
        if self.replaying.load(Ordering::Relaxed) {
            // Old output: keep what describes the current state, drop notifications/bells.
            evs.retain(|e| matches!(e, TermEvent::Title { .. } | TermEvent::Cwd { .. }));
        }
        {
            let mut out = self.output.lock();
            out.replay.extend(chunk);
            let over = out.replay.len().saturating_sub(REPLAY_CAP);
            if over > 0 {
                out.replay.drain(..over);
            }
            if out.subscriber.is_some() {
                out.pending.extend_from_slice(chunk);
                if !out.scheduled {
                    if out.last_flush.elapsed() >= flush_every() {
                        out.flush();
                    } else {
                        out.scheduled = true;
                        let due = out.last_flush + flush_every();
                        let _ = flusher().send((Arc::downgrade(self), due));
                    }
                }
            }
        }
        if !evs.is_empty() {
            self.events.on_events(self.pane, evs);
        }
    }

    pub fn exited(&self, code: Option<i32>) {
        self.events.on_exit(self.pane, code);
    }
}

enum Backend {
    Local {
        master: Mutex<Box<dyn MasterPty + Send>>,
        /// Input goes through a dedicated writer thread so callers never block on a busy shell.
        input: std::sync::mpsc::Sender<Vec<u8>>,
        killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    },
    Host(Arc<HostClient>),
}

struct Session {
    backend: Backend,
    sink: Arc<Sink>,
    pid: Option<u32>,
}

#[derive(Default)]
pub struct PtyManager {
    sessions: Arc<Mutex<HashMap<PaneId, Arc<Session>>>>,
    host: Mutex<Option<Arc<HostClient>>>,
}

impl PtyManager {
    /// Start a shell in this process.
    pub fn spawn(&self, pane: PaneId, spec: SpawnSpec, events: Arc<dyn PtyEvents>) -> anyhow::Result<()> {
        let size = PtySize { rows: spec.rows.max(2), cols: spec.cols.max(2), pixel_width: 0, pixel_height: 0 };
        let pair = native_pty_system().openpty(size)?;
        let mut cmd = CommandBuilder::new(&spec.command);
        cmd.args(&spec.args);
        let cwd = spec
            .cwd
            .filter(|c| std::path::Path::new(c).is_dir())
            .or_else(|| std::env::var("USERPROFILE").ok());
        if let Some(c) = cwd {
            cmd.cwd(c);
        }
        for (k, v) in base_env().into_iter().chain(spec.env) {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;

        let mut writer = pair.master.take_writer()?;
        let (input, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new().name(format!("pty-write-{pane}")).spawn(move || {
            while let Ok(data) = input_rx.recv() {
                if writer.write_all(&data).and_then(|_| writer.flush()).is_err() {
                    break;
                }
            }
        })?;
        let sink = Sink::new(pane, size.rows, size.cols, events, Some(input.clone()));
        {
            let sink = sink.clone();
            std::thread::Builder::new().name(format!("pty-read-{pane}")).spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                // HIVETTY_DEBUG_PERF: report time spent waiting on ConPTY vs processing output
                let perf = std::env::var_os("HIVETTY_DEBUG_PERF").is_some();
                let (mut reads, mut bytes, mut busy, mut wait, mut since) = (0u64, 0u64, Duration::ZERO, Duration::ZERO, Instant::now());
                loop {
                    let t0 = Instant::now();
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let t1 = Instant::now();
                    sink.ingest(&buf[..n]);
                    if perf {
                        reads += 1;
                        bytes += n as u64;
                        wait += t1 - t0;
                        busy += t1.elapsed();
                        if since.elapsed() > Duration::from_secs(5) && bytes > 0 {
                            eprintln!("[pty] {pane}: {reads} reads, {:.1} MB, waiting on conpty {:.0} ms, processing {:.0} ms", bytes as f64 / 1048576.0, wait.as_secs_f64() * 1e3, busy.as_secs_f64() * 1e3);
                            (reads, bytes, busy, wait, since) = (0, 0, Duration::ZERO, Duration::ZERO, Instant::now());
                        }
                    }
                }
            })?;
        }
        let session = Arc::new(Session { backend: Backend::Local { master: Mutex::new(pair.master), input, killer: Mutex::new(killer) }, sink: sink.clone(), pid });
        self.sessions.lock().insert(pane, session);
        let me = self.sessions.clone();
        std::thread::Builder::new().name(format!("pty-wait-{pane}")).spawn(move || {
            let code = child.wait().ok().map(|s| s.exit_code() as i32);
            if is_current(&me, pane, sink.generation) {
                sink.exited(code);
            }
        })?;
        Ok(())
    }

    /// The connected session host, starting it if needed.
    fn host(&self) -> anyhow::Result<Arc<HostClient>> {
        let mut h = self.host.lock();
        if let Some(c) = h.as_ref().filter(|c| c.alive()) {
            return Ok(c.clone());
        }
        let c = HostClient::connect_or_start()?;
        *h = Some(c.clone());
        Ok(c)
    }

    /// The host if it's already running (doesn't start one).
    pub fn existing_host(&self) -> Option<Arc<HostClient>> {
        let mut h = self.host.lock();
        if let Some(c) = h.as_ref().filter(|c| c.alive()) {
            return Some(c.clone());
        }
        let c = HostClient::connect_existing()?;
        *h = Some(c.clone());
        Some(c)
    }

    /// Start a shell in the detached session host so it survives the app closing.
    pub fn spawn_hosted(&self, pane: PaneId, spec: SpawnSpec, meta: serde_json::Value, events: Arc<dyn PtyEvents>) -> anyhow::Result<()> {
        let host = self.host()?;
        let sink = Sink::new(pane, spec.rows, spec.cols, events, None);
        host.register(sink.clone());
        let mut env = base_env();
        env.extend(spec.env.iter().cloned());
        let pid = match host.spawn(pane, &spec, &env, meta) {
            Ok(pid) => pid,
            Err(e) => {
                host.unregister(pane, sink.generation);
                return Err(e);
            }
        };
        self.sessions.lock().insert(pane, Arc::new(Session { backend: Backend::Host(host), sink, pid }));
        Ok(())
    }

    /// Reattach to a shell that kept running in the session host. Returns its exit code if it
    /// ended while the app was closed (`Some(None)` = ended, code unknown).
    pub fn reattach(&self, host: &Arc<HostClient>, pane: PaneId, rows: u16, cols: u16, events: Arc<dyn PtyEvents>) -> anyhow::Result<Option<Option<i32>>> {
        let sink = Sink::new(pane, rows, cols, events, None);
        sink.replaying.store(true, Ordering::Relaxed);
        host.register(sink.clone());
        let r = match host.attach(pane) {
            Ok(r) => r,
            Err(e) => {
                host.unregister(pane, sink.generation);
                return Err(e);
            }
        };
        sink.replaying.store(false, Ordering::Relaxed);
        self.sessions.lock().insert(pane, Arc::new(Session { backend: Backend::Host(host.clone()), sink, pid: r.pid }));
        Ok(r.exited.then_some(r.code))
    }

    fn get(&self, pane: PaneId) -> Option<Arc<Session>> {
        self.sessions.lock().get(&pane).cloned()
    }

    /// Attach a UI channel: replays buffered output, then streams live output.
    pub fn attach(&self, pane: PaneId, ch: Channel<InvokeResponseBody>) -> bool {
        let Some(s) = self.get(pane) else { return false };
        let mut out = s.sink.output.lock();
        if !out.replay.is_empty() {
            let (a, b) = out.replay.as_slices();
            let mut all = Vec::with_capacity(a.len() + b.len());
            all.extend_from_slice(a);
            all.extend_from_slice(b);
            let _ = ch.send(InvokeResponseBody::Raw(all));
        }
        // the replay already holds anything not yet flushed to the previous subscriber
        out.pending.clear();
        out.subscriber = Some(ch);
        true
    }

    /// Queue input for the shell. Never blocks.
    pub fn write(&self, pane: PaneId, data: &[u8]) -> bool {
        let Some(s) = self.get(pane) else { return false };
        match &s.backend {
            Backend::Local { input, .. } => input.send(data.to_vec()).is_ok(),
            Backend::Host(h) => h.write(pane, data),
        }
    }

    pub fn resize(&self, pane: PaneId, cols: u16, rows: u16) {
        if let Some(s) = self.get(pane) {
            let (cols, rows) = (cols.max(2), rows.max(2));
            match &s.backend {
                Backend::Local { master, .. } => {
                    let _ = master.lock().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                }
                Backend::Host(h) => h.resize(pane, cols, rows),
            }
            s.sink.screen.lock().screen_mut().set_size(rows, cols);
        }
    }

    pub fn kill(&self, pane: PaneId) {
        let s = self.sessions.lock().remove(&pane);
        if let Some(s) = s {
            match &s.backend {
                Backend::Local { killer, .. } => {
                    let _ = killer.lock().kill();
                }
                Backend::Host(h) => {
                    h.unregister(pane, s.sink.generation);
                    h.kill(pane);
                }
            }
        }
    }

    /// App shutdown: kill local shells. Hosted shells keep running unless `kill_hosted`.
    pub fn shutdown(&self, kill_hosted: bool) {
        let all: Vec<_> = self.sessions.lock().drain().collect();
        for (pane, s) in all {
            match &s.backend {
                Backend::Local { killer, .. } => {
                    let _ = killer.lock().kill();
                }
                Backend::Host(h) => {
                    if kill_hosted {
                        h.kill(pane);
                    }
                }
            }
        }
        if let Some(h) = self.host.lock().take() {
            h.flush();
        }
    }

    /// Visible screen text (or scrolled back by `scrollback` lines).
    pub fn read_screen(&self, pane: PaneId, scrollback: usize) -> Option<String> {
        let s = self.get(pane)?;
        let mut p = s.sink.screen.lock();
        if scrollback == 0 {
            return Some(p.screen().contents());
        }
        let prev = p.screen().scrollback();
        p.screen_mut().set_scrollback(scrollback);
        let text = p.screen().contents();
        p.screen_mut().set_scrollback(prev);
        Some(text)
    }

    /// Copy of the raw output replay buffer.
    pub fn replay(&self, pane: PaneId) -> Option<Vec<u8>> {
        let s = self.get(pane)?;
        let out = s.sink.output.lock();
        Some(out.replay.iter().copied().collect())
    }

    pub fn size(&self, pane: PaneId) -> Option<(u16, u16)> {
        let s = self.get(pane)?;
        let size = s.sink.screen.lock().screen().size();
        Some(size)
    }

    pub fn pid(&self, pane: PaneId) -> Option<u32> {
        self.get(pane)?.pid
    }

    pub fn pids(&self) -> Vec<(PaneId, u32)> {
        self.sessions.lock().iter().filter_map(|(k, s)| s.pid.map(|p| (*k, p))).collect()
    }
}

impl Sink {
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

fn base_env() -> Vec<(String, String)> {
    vec![
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "Hivetty".into()),
        ("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into()),
    ]
}

fn is_current(map: &Mutex<HashMap<PaneId, Arc<Session>>>, pane: PaneId, generation: u64) -> bool {
    map.lock().get(&pane).is_some_and(|s| s.sink.generation == generation)
}
