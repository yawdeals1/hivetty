//! `tmux.exe` — a tmux-compatible shim. Hivetty puts it on PATH (with `$TMUX` set) inside its
//! terminals, so tools that drive tmux (e.g. Claude Code agent teams) get native Hivetty splits.
//!
//! Mapping: session = workspace (`$N`), window = tab (`@N`), pane = pane (`%N`). Window and pane
//! indexes are 0-based, like tmux's defaults.

#[path = "client.rs"]
mod client;

use anyhow::{anyhow, bail, Result};
use client::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;

const VERSION: &str = "tmux 3.4";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

// ---------- argument parsing ----------

#[derive(Debug, Default, PartialEq)]
struct Opts {
    flags: Vec<char>,
    vals: HashMap<char, String>,
    args: Vec<String>,
}

impl Opts {
    fn has(&self, c: char) -> bool {
        self.flags.contains(&c)
    }
    fn val(&self, c: char) -> Option<&str> {
        self.vals.get(&c).map(String::as_str)
    }
}

/// tmux-style getopt: `bools` take no value, `vals` take one (`-c dir` or `-cdir`); flags combine.
fn getopt(args: &[String], bools: &str, vals: &str) -> Result<Opts> {
    let mut o = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            o.args.extend(args[i + 1..].iter().cloned());
            break;
        }
        if !a.starts_with('-') || a.len() < 2 {
            o.args.extend(args[i..].iter().cloned());
            break;
        }
        let chars: Vec<char> = a[1..].chars().collect();
        let mut j = 0;
        while j < chars.len() {
            let c = chars[j];
            if vals.contains(c) {
                let rest: String = chars[j + 1..].iter().collect();
                let v = if !rest.is_empty() {
                    rest
                } else {
                    i += 1;
                    args.get(i).cloned().ok_or_else(|| anyhow!("-{c} expects an argument"))?
                };
                o.vals.insert(c, v);
                break;
            } else if bools.contains(c) {
                o.flags.push(c);
            } else {
                bail!("unknown flag -{c}");
            }
            j += 1;
        }
        i += 1;
    }
    Ok(o)
}

/// Split on `;` arguments (`tmux a \; b`); a trailing `\;` inside an argument also separates.
fn split_commands(args: Vec<String>) -> Vec<Vec<String>> {
    let mut out = vec![Vec::new()];
    for a in args {
        if a == ";" {
            out.push(Vec::new());
        } else if let Some(stripped) = a.strip_suffix(';').filter(|s| !s.ends_with('\\') && !s.is_empty()) {
            out.last_mut().unwrap().push(stripped.to_string());
            out.push(Vec::new());
        } else {
            out.last_mut().unwrap().push(a);
        }
    }
    out.retain(|c| !c.is_empty());
    out
}

// ---------- state ----------

#[derive(Debug, Clone)]
struct PaneInfo {
    v: Value,
    ws: usize,
    surface: usize,
}

struct State {
    root: Value,
    panes: Vec<PaneInfo>,
}

impl State {
    fn new(root: Value) -> Self {
        let mut panes = Vec::new();
        for (wi, w) in root["workspaces"].as_array().into_iter().flatten().enumerate() {
            for (si, s) in w["surfaces"].as_array().into_iter().flatten().enumerate() {
                for p in s["panes"].as_array().into_iter().flatten() {
                    panes.push(PaneInfo { v: p.clone(), ws: wi, surface: si });
                }
            }
        }
        State { root, panes }
    }
    fn ws(&self, i: usize) -> &Value {
        &self.root["workspaces"][i]
    }
    fn surface(&self, p: &PaneInfo) -> &Value {
        &self.ws(p.ws)["surfaces"][p.surface]
    }
    fn workspaces(&self) -> Vec<&Value> {
        self.root["workspaces"].as_array().map(|a| a.iter().collect()).unwrap_or_default()
    }
    fn by_num(&self, n: u64) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| p.v["num"].as_u64() == Some(n))
    }
    fn by_id(&self, id: &str) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| p.v["id"].as_str() == Some(id))
    }
    /// The pane this command runs from: $TMUX_PANE, else Hivetty's focused pane.
    fn current(&self) -> Option<&PaneInfo> {
        std::env::var("TMUX_PANE")
            .ok()
            .and_then(|s| s.strip_prefix('%').and_then(|n| n.parse().ok()))
            .and_then(|n| self.by_num(n))
            .or_else(|| self.root["focused"].as_str().and_then(|id| self.by_id(id)))
            .or_else(|| self.panes.first())
    }
    fn active_in_surface(&self, ws: usize, si: usize) -> Option<&PaneInfo> {
        let mut it = self.panes.iter().filter(|p| p.ws == ws && p.surface == si);
        let all: Vec<_> = it.by_ref().collect();
        all.iter().find(|p| p.v["active"] == true).copied().or_else(|| all.first().copied())
    }
    fn active_surface(&self, ws: usize) -> usize {
        self.ws(ws)["surfaces"].as_array().and_then(|a| a.iter().position(|s| s["active"] == true)).unwrap_or(0)
    }
    fn find_session(&self, s: &str) -> Option<usize> {
        let ws = self.workspaces();
        if let Some(n) = s.strip_prefix('$').and_then(|n| n.parse::<u64>().ok()) {
            return ws.iter().position(|w| w["num"].as_u64() == Some(n));
        }
        ws.iter()
            .position(|w| w["name"].as_str() == Some(s))
            .or_else(|| ws.iter().position(|w| w["name"].as_str().is_some_and(|n| n.starts_with(s))))
            .or_else(|| s.parse::<usize>().ok().filter(|i| *i < ws.len()))
    }
    fn find_window(&self, ws: usize, w: &str) -> Option<usize> {
        let surfaces = self.ws(ws)["surfaces"].as_array()?;
        if let Some(n) = w.strip_prefix('@').and_then(|n| n.parse::<u64>().ok()) {
            return surfaces.iter().position(|s| s["num"].as_u64() == Some(n));
        }
        if let Ok(i) = w.parse::<usize>() {
            return (i < surfaces.len()).then_some(i);
        }
        surfaces.iter().position(|s| s["title"].as_str() == Some(w))
    }

    /// Resolve a tmux target (`%3`, `@2`, `$1`, `sess:win.pane`, `:.1`, ...) to a pane.
    fn resolve(&self, target: Option<&str>) -> Result<&PaneInfo> {
        let cur = self.current().ok_or_else(|| anyhow!("no current pane"))?;
        let Some(t) = target.filter(|t| !t.is_empty()) else { return Ok(cur) };
        let fail = || anyhow!("can't find pane: {t}");
        if let Some(n) = t.strip_prefix('%') {
            return n.parse().ok().and_then(|n| self.by_num(n)).ok_or_else(fail);
        }
        if t == "{last}" || t == "!" || t == "{marked}" {
            return Ok(cur);
        }
        if let Some(n) = t.strip_prefix('@') {
            let (wpart, ppart) = n.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((n, None));
            let w = self.panes.iter().find(|p| self.surface(p)["num"].as_u64() == wpart.parse().ok()).ok_or_else(fail)?;
            return self.pick_pane(w.ws, w.surface, ppart).ok_or_else(fail);
        }
        // [session][:window][.pane]
        let (sess, rest) = match t.split_once(':') {
            Some((s, r)) => (Some(s), Some(r)),
            None if t.starts_with('.') => (None, Some(t)),
            None => (Some(t), None),
        };
        let ws = match sess.filter(|s| !s.is_empty()) {
            Some(s) => self.find_session(s).ok_or_else(|| anyhow!("can't find session: {s}"))?,
            None => cur.ws,
        };
        let Some(rest) = rest else {
            return self.active_in_surface(ws, self.active_surface(ws)).ok_or_else(fail);
        };
        let (wpart, ppart) = match rest.split_once('.') {
            Some((w, p)) => (w, Some(p)),
            None => (rest, None),
        };
        let si = if wpart.is_empty() {
            if sess.is_some_and(|s| !s.is_empty()) { self.active_surface(ws) } else { cur.surface }
        } else {
            self.find_window(ws, wpart).ok_or_else(|| anyhow!("can't find window: {wpart}"))?
        };
        self.pick_pane(ws, si, ppart).ok_or_else(fail)
    }

    fn pick_pane(&self, ws: usize, si: usize, p: Option<&str>) -> Option<&PaneInfo> {
        match p.filter(|p| !p.is_empty()) {
            None => self.active_in_surface(ws, si),
            Some(p) => {
                if let Some(n) = p.strip_prefix('%') {
                    return n.parse().ok().and_then(|n| self.by_num(n));
                }
                let i: usize = p.parse().ok()?;
                self.panes.iter().filter(|x| x.ws == ws && x.surface == si).nth(i)
            }
        }
    }

    fn session_for(&self, target: Option<&str>) -> Result<usize> {
        match target.filter(|t| !t.is_empty()) {
            None => Ok(self.current().map(|p| p.ws).unwrap_or(0)),
            Some(t) if t.starts_with('%') || t.starts_with('@') => Ok(self.resolve(Some(t))?.ws),
            Some(t) => {
                let s = t.split(':').next().unwrap_or(t);
                if s.is_empty() {
                    return Ok(self.current().map(|p| p.ws).unwrap_or(0));
                }
                self.find_session(s).ok_or_else(|| anyhow!("can't find session: {s}"))
            }
        }
    }

    // ---------- formats ----------

    fn vars(&self, p: &PaneInfo) -> HashMap<&'static str, String> {
        let w = self.ws(p.ws);
        let s = self.surface(p);
        let pv = &p.v;
        let str_of = |v: &Value| v.as_str().map(str::to_string).unwrap_or_default();
        let num = |v: &Value| v.as_u64().map(|n| n.to_string()).unwrap_or_default();
        let b = |x: bool| if x { "1" } else { "0" }.to_string();
        let npanes = self.panes.iter().filter(|x| x.ws == p.ws && x.surface == p.surface).count();
        let title = pv["title"].as_str().map(str::to_string).unwrap_or_else(|| "hivetty".into());
        let window_name = s["title"].as_str().map(str::to_string).unwrap_or_else(|| title.clone());
        let session_active = w["active"] == true;
        let window_active = s["active"] == true;
        let cmd = pv["command"].as_str().and_then(|c| c.split_whitespace().next()).map(str::to_string).unwrap_or_else(|| str_of(&pv["profile"]));
        let mut m = HashMap::new();
        m.insert("pane_id", format!("%{}", num(&pv["num"])));
        m.insert("pane_index", num(&pv["index"]));
        m.insert("pane_title", title);
        m.insert("pane_current_path", str_of(&pv["cwd"]));
        m.insert("pane_pid", num(&pv["pid"]));
        m.insert("pane_width", num(&pv["width"]));
        m.insert("pane_height", num(&pv["height"]));
        m.insert("pane_active", b(pv["active"] == true));
        m.insert("pane_dead", b(pv["dead"] == true));
        m.insert("pane_current_command", cmd);
        m.insert("pane_tty", String::new());
        m.insert("pane_left", ((pv["left"].as_f64().unwrap_or(0.0) * s["width"].as_f64().unwrap_or(0.0)).round() as u64).to_string());
        m.insert("pane_top", ((pv["top"].as_f64().unwrap_or(0.0) * s["height"].as_f64().unwrap_or(0.0)).round() as u64).to_string());
        m.insert("window_id", format!("@{}", num(&s["num"])));
        m.insert("window_index", num(&s["index"]));
        m.insert("window_name", window_name);
        m.insert("window_active", b(window_active));
        m.insert("window_panes", npanes.to_string());
        m.insert("window_width", num(&s["width"]));
        m.insert("window_height", num(&s["height"]));
        m.insert("window_zoomed_flag", b(s["zoomed"] == true));
        m.insert("window_layout", String::new());
        m.insert("session_id", format!("${}", num(&w["num"])));
        m.insert("session_name", str_of(&w["name"]));
        m.insert("session_windows", w["surfaces"].as_array().map_or(0, |a| a.len()).to_string());
        m.insert("session_attached", b(session_active));
        m.insert("session_created", "0".into());
        m.insert("client_tty", String::new());
        m.insert("pid", num(&self.root["pid"]));
        m.insert("version", VERSION.trim_start_matches("tmux ").into());
        m.insert("host", std::env::var("COMPUTERNAME").unwrap_or_default());
        m
    }

    fn expand(&self, fmt: &str, p: &PaneInfo) -> String {
        expand(fmt, &self.vars(p))
    }
}

/// Expand `#{var}`, `#{?cond,a,b}`, `#{=N:var}` and the short aliases (`#S`, `#I`, `#P`, `#D`, ...).
fn expand(fmt: &str, vars: &HashMap<&'static str, String>) -> String {
    let short = |c: char| -> Option<&'static str> {
        Some(match c {
            'S' => "session_name",
            'I' => "window_index",
            'W' => "window_name",
            'P' => "pane_index",
            'D' => "pane_id",
            'T' => "pane_title",
            'H' => "host",
            'F' => "window_flags",
            _ => return None,
        })
    };
    let lookup = |name: &str| -> String {
        let name = name.trim();
        if let Some(rest) = name.strip_prefix('=') {
            // #{=N:var} — truncate
            if let Some((n, v)) = rest.split_once(':') {
                let n: isize = n.parse().unwrap_or(0);
                let val = vars.get(v).cloned().unwrap_or_default();
                let chars: Vec<char> = val.chars().collect();
                return if n >= 0 { chars.iter().take(n as usize).collect() } else { chars.iter().rev().take((-n) as usize).rev().collect() };
            }
        }
        vars.get(name).cloned().unwrap_or_default()
    };
    let truthy = |s: &str| !s.is_empty() && s != "0";
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '#' || i + 1 >= chars.len() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let c = chars[i + 1];
        if c == '#' {
            out.push('#');
            i += 2;
        } else if c == '{' {
            // find matching brace
            let mut depth = 0;
            let mut j = i + 1;
            while j < chars.len() {
                match chars[j] {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            let inner: String = chars[i + 2..j.min(chars.len())].iter().collect();
            if let Some(cond) = inner.strip_prefix('?') {
                // split at top-level commas
                let mut parts = Vec::new();
                let (mut d, mut cur) = (0, String::new());
                for ch in cond.chars() {
                    match ch {
                        '{' => d += 1,
                        '}' => d -= 1,
                        ',' if d == 0 => {
                            parts.push(std::mem::take(&mut cur));
                            continue;
                        }
                        _ => {}
                    }
                    cur.push(ch);
                }
                parts.push(cur);
                let test = lookup(&parts[0]);
                let pick = if truthy(&test) { parts.get(1) } else { parts.get(2) };
                out.push_str(&expand(pick.map(String::as_str).unwrap_or(""), vars));
            } else {
                out.push_str(&lookup(&inner));
            }
            i = j + 1;
        } else if let Some(name) = short(c) {
            out.push_str(&lookup(name));
            i += 2;
        } else {
            out.push('#');
            i += 1;
        }
    }
    out
}

// ---------- keys ----------

/// Translate send-keys arguments into bytes. Unknown words are sent literally.
fn keys_to_bytes(keys: &[String], literal: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for k in keys {
        if literal {
            out.extend(k.as_bytes());
            continue;
        }
        match key_bytes(k) {
            Some(b) => out.extend(b),
            None => out.extend(k.as_bytes()),
        }
    }
    out
}

fn key_bytes(k: &str) -> Option<Vec<u8>> {
    let s = |x: &str| Some(x.as_bytes().to_vec());
    match k {
        "Enter" | "C-m" | "KPEnter" => return s("\r"),
        "C-j" => return s("\n"),
        "Escape" | "C-[" => return s("\x1b"),
        "Tab" | "C-i" => return s("\t"),
        "BTab" => return s("\x1b[Z"),
        "BSpace" => return s("\x7f"),
        "Space" => return s(" "),
        "Up" => return s("\x1b[A"),
        "Down" => return s("\x1b[B"),
        "Right" => return s("\x1b[C"),
        "Left" => return s("\x1b[D"),
        "Home" => return s("\x1b[H"),
        "End" => return s("\x1b[F"),
        "PageUp" | "PPage" | "PgUp" => return s("\x1b[5~"),
        "PageDown" | "NPage" | "PgDn" => return s("\x1b[6~"),
        "DC" | "Delete" => return s("\x1b[3~"),
        "IC" | "Insert" => return s("\x1b[2~"),
        _ => {}
    }
    if let Some(n) = k.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        let seq = match n {
            1 => "\x1bOP",
            2 => "\x1bOQ",
            3 => "\x1bOR",
            4 => "\x1bOS",
            5 => "\x1b[15~",
            6 => "\x1b[17~",
            7 => "\x1b[18~",
            8 => "\x1b[19~",
            9 => "\x1b[20~",
            10 => "\x1b[21~",
            11 => "\x1b[23~",
            12 => "\x1b[24~",
            _ => return None,
        };
        return s(seq);
    }
    if let Some(rest) = k.strip_prefix("C-").or_else(|| k.strip_prefix("^")) {
        let mut c = rest.chars();
        if let (Some(ch), None) = (c.next(), c.next()) {
            let ch = ch.to_ascii_lowercase();
            if ch.is_ascii_lowercase() {
                return Some(vec![ch as u8 - b'a' + 1]);
            }
            return match ch {
                '@' | ' ' => Some(vec![0]),
                '\\' => Some(vec![0x1c]),
                ']' => Some(vec![0x1d]),
                '^' => Some(vec![0x1e]),
                '_' => Some(vec![0x1f]),
                '?' => Some(vec![0x7f]),
                _ => None,
            };
        }
    }
    if let Some(rest) = k.strip_prefix("M-") {
        let mut v = vec![0x1b];
        v.extend(key_bytes(rest).unwrap_or_else(|| rest.as_bytes().to_vec()));
        return Some(v);
    }
    None
}

/// `-l 30%` / `-l 40` / `-p 30` → fraction of the window (absolute cells need the window size).
fn size_fraction(v: &str, total: f64) -> Option<f32> {
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse::<f32>().ok().map(|p| p / 100.0);
    }
    let n: f64 = v.trim().parse().ok()?;
    (total > 0.0).then(|| (n / total) as f32)
}

// ---------- commands ----------

struct Ctx {
    c: Option<Client>,
    st: Option<State>,
    out: Vec<String>,
}

impl Ctx {
    fn client(&mut self) -> Result<&mut Client> {
        if self.c.is_none() {
            self.c = Some(Client::connect().map_err(|_| anyhow!("no server running on {}", client::pipe_name()))?);
        }
        Ok(self.c.as_mut().unwrap())
    }
    fn call(&mut self, m: &str, p: Value) -> Result<Value> {
        let r = self.client()?.call(m, p);
        self.st = None; // state may have changed
        r
    }
    fn state(&mut self) -> Result<&State> {
        if self.st.is_none() {
            let v = self.client()?.call("tmux.state", json!({}))?;
            self.st = Some(State::new(v));
        }
        Ok(self.st.as_ref().unwrap())
    }
    /// Print `fmt` (or `default`) for the pane with id `id`.
    fn print_pane(&mut self, id: &str, fmt: Option<&str>, default: &str) -> Result<()> {
        let st = self.state()?;
        let p = st.by_id(id).ok_or_else(|| anyhow!("new pane vanished"))?.clone();
        let line = st.expand(fmt.unwrap_or(default), &p);
        self.out.push(line);
        Ok(())
    }
}

fn run(args: Vec<String>) -> Result<i32> {
    // global options
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        match args[i].as_str() {
            "-V" => {
                println!("{VERSION}");
                return Ok(0);
            }
            "-L" | "-S" | "-f" | "-T" => i += 1,
            "-c" => bail!("tmux -c is not supported by Hivetty's tmux shim"),
            "-C" | "-CC" => bail!("control mode is not supported by Hivetty's tmux shim"),
            "--" => {
                i += 1;
                break;
            }
            _ => {}
        }
        i += 1;
    }
    let rest: Vec<String> = args[i.min(args.len())..].to_vec();
    if rest.is_empty() {
        bail!("Hivetty's tmux shim can't start an interactive tmux client; you're already in Hivetty");
    }
    let mut ctx = Ctx { c: None, st: None, out: Vec::new() };
    let mut code = 0;
    for cmd in split_commands(rest) {
        code = command(&mut ctx, &cmd)?;
        if code != 0 {
            break;
        }
    }
    let mut so = std::io::stdout().lock();
    for l in &ctx.out {
        let _ = writeln!(so, "{l}");
    }
    Ok(code)
}

fn joined(v: &[String]) -> Option<String> {
    let s = v.join(" ");
    (!s.trim().is_empty()).then_some(s)
}

fn command(ctx: &mut Ctx, cmd: &[String]) -> Result<i32> {
    let name = cmd[0].as_str();
    let a = &cmd[1..];
    match name {
        "split-window" | "splitw" => {
            let o = getopt(a, "bdfhvIPZ", "cetlpF")?;
            let (target, total_w, total_h) = {
                let st = ctx.state()?;
                let t = st.resolve(o.val('t'))?.clone();
                let s = st.surface(&t);
                (t.v["id"].as_str().unwrap_or_default().to_string(), s["width"].as_f64().unwrap_or(0.0), s["height"].as_f64().unwrap_or(0.0))
            };
            let horizontal = o.has('h');
            let dir = match (horizontal, o.has('b')) {
                (true, false) => "right",
                (true, true) => "left",
                (false, false) => "down",
                (false, true) => "up",
            };
            let total = if horizontal { total_w } else { total_h };
            let size = o.val('l').and_then(|v| size_fraction(v, total)).or_else(|| o.val('p').and_then(|p| p.parse::<f32>().ok()).map(|p| p / 100.0));
            let v = ctx.call(
                "pane.split",
                json!({ "pane": target, "dir": dir, "cwd": o.val('c'), "command": joined(&o.args), "size": size, "focus": !o.has('d') }),
            )?;
            let id = v["pane"].as_str().unwrap_or_default().to_string();
            if o.has('P') {
                ctx.print_pane(&id, o.val('F'), "#{session_name}:#{window_index}.#{pane_index}")?;
            }
            Ok(0)
        }
        "new-window" | "neww" => {
            let o = getopt(a, "abdkPS", "cetnF")?;
            let ws = {
                let st = ctx.state()?;
                let i = st.session_for(o.val('t'))?;
                st.ws(i)["id"].as_str().unwrap_or_default().to_string()
            };
            let v = ctx.call("surface.new", json!({ "workspace": ws, "cwd": o.val('c'), "command": joined(&o.args), "title": o.val('n'), "focus": !o.has('d') }))?;
            let id = v["pane"].as_str().unwrap_or_default().to_string();
            if o.has('P') {
                ctx.print_pane(&id, o.val('F'), "#{session_name}:#{window_index}")?;
            }
            Ok(0)
        }
        "new-session" | "new" => {
            let o = getopt(a, "AdDEPX", "cefnsStxyF")?;
            if let Some(name) = o.val('s') {
                let dup = {
                    let st = ctx.state()?;
                    st.find_session(name).is_some_and(|i| st.ws(i)["name"].as_str() == Some(name))
                };
                if dup {
                    if o.has('A') {
                        return Ok(0);
                    }
                    bail!("duplicate session: {name}");
                }
            }
            let v = ctx.call("workspace.new", json!({ "name": o.val('s'), "cwd": o.val('c'), "command": joined(&o.args), "focus": !o.has('d') }))?;
            let id = v["pane"].as_str().unwrap_or_default().to_string();
            if let Some(n) = o.val('n') {
                let sid = {
                    let st = ctx.state()?;
                    st.by_id(&id).map(|p| st.surface(p)["id"].clone())
                };
                if let Some(sid) = sid {
                    ctx.call("surface.rename", json!({ "surface": sid, "title": n }))?;
                }
            }
            if o.has('P') {
                ctx.print_pane(&id, o.val('F'), "#{session_name}:")?;
            }
            Ok(0)
        }
        "send-keys" | "send" => {
            let o = getopt(a, "FHlMRX", "Nt")?;
            if o.has('X') {
                bail!("copy-mode commands are not supported");
            }
            let id = ctx.state()?.resolve(o.val('t'))?.v["id"].clone();
            let mut bytes = keys_to_bytes(&o.args, o.has('l'));
            if let Some(n) = o.val('N').and_then(|n| n.parse::<usize>().ok()) {
                bytes = bytes.repeat(n.max(1));
            }
            if !bytes.is_empty() {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                ctx.call("pane.send", json!({ "pane": id, "text": text, "enter": false }))?;
            }
            Ok(0)
        }
        "kill-pane" | "killp" => {
            let o = getopt(a, "a", "t")?;
            let (target, others) = {
                let st = ctx.state()?;
                let t = st.resolve(o.val('t'))?.clone();
                let others: Vec<Value> = st.panes.iter().filter(|p| p.ws == t.ws && p.surface == t.surface && p.v["id"] != t.v["id"]).map(|p| p.v["id"].clone()).collect();
                (t.v["id"].clone(), others)
            };
            if o.has('a') {
                for p in others {
                    ctx.call("pane.close", json!({ "pane": p }))?;
                }
            } else {
                ctx.call("pane.close", json!({ "pane": target }))?;
            }
            Ok(0)
        }
        "kill-window" | "killw" => {
            let o = getopt(a, "a", "t")?;
            let sid = {
                let st = ctx.state()?;
                let t = st.resolve(o.val('t'))?.clone();
                st.surface(&t)["id"].clone()
            };
            ctx.call("surface.close", json!({ "surface": sid }))?;
            Ok(0)
        }
        "kill-session" => {
            let o = getopt(a, "aC", "t")?;
            let ws = {
                let st = ctx.state()?;
                st.ws(st.session_for(o.val('t'))?)["id"].clone()
            };
            ctx.call("workspace.close", json!({ "workspace": ws }))?;
            Ok(0)
        }
        "kill-server" => bail!("kill-server would close all of Hivetty; refusing"),
        "select-pane" | "selectp" => {
            let o = getopt(a, "DdegLlMmRUZ", "TtP")?;
            let id = ctx.state()?.resolve(o.val('t'))?.v["id"].clone();
            if let Some(dir) = ['L', 'R', 'U', 'D'].into_iter().find(|c| o.has(*c)) {
                let d = match dir {
                    'L' => "left",
                    'R' => "right",
                    'U' => "up",
                    _ => "down",
                };
                ctx.call("pane.focus", json!({ "pane": id }))?;
                ctx.call("pane.focus_dir", json!({ "dir": d }))?;
                return Ok(0);
            }
            if let Some(t) = o.val('T') {
                ctx.call("pane.title", json!({ "pane": id, "title": t }))?;
            }
            // Styling (-P) and titles (-T) alone don't steal focus.
            if o.val('T').is_none() && o.val('P').is_none() && !o.has('d') && !o.has('e') {
                ctx.call("pane.focus", json!({ "pane": id }))?;
            }
            Ok(0)
        }
        "select-window" | "selectw" => {
            let o = getopt(a, "lnpT", "t")?;
            let sid = {
                let st = ctx.state()?;
                let t = st.resolve(o.val('t'))?.clone();
                st.surface(&t)["id"].clone()
            };
            ctx.call("surface.select", json!({ "surface": sid }))?;
            Ok(0)
        }
        "switch-client" | "switchc" | "attach-session" | "attach" => {
            let o = getopt(a, "dElnprxZ", "ctTf")?;
            let ws = {
                let st = ctx.state()?;
                st.ws(st.session_for(o.val('t'))?)["id"].clone()
            };
            ctx.call("workspace.select", json!({ "workspace": ws }))?;
            Ok(0)
        }
        "rename-window" | "renamew" => {
            let o = getopt(a, "", "t")?;
            let sid = {
                let st = ctx.state()?;
                let t = st.resolve(o.val('t'))?.clone();
                st.surface(&t)["id"].clone()
            };
            ctx.call("surface.rename", json!({ "surface": sid, "title": o.args.join(" ") }))?;
            Ok(0)
        }
        "rename-session" | "rename" => {
            let o = getopt(a, "", "t")?;
            let ws = {
                let st = ctx.state()?;
                st.ws(st.session_for(o.val('t'))?)["id"].clone()
            };
            ctx.call("workspace.rename", json!({ "workspace": ws, "name": o.args.join(" ") }))?;
            Ok(0)
        }
        "resize-pane" | "resizep" => {
            let o = getopt(a, "DLMRTUZ", "txy")?;
            let (id, ww, wh) = {
                let st = ctx.state()?;
                let t = st.resolve(o.val('t'))?.clone();
                let s = st.surface(&t);
                (t.v["id"].clone(), s["width"].as_f64().unwrap_or(0.0), s["height"].as_f64().unwrap_or(0.0))
            };
            let width = o.val('x').and_then(|v| size_fraction(v, ww));
            let height = o.val('y').and_then(|v| size_fraction(v, wh));
            ctx.call("pane.resize", json!({ "pane": id, "width": width, "height": height, "zoom": o.has('Z') }))?;
            Ok(0)
        }
        "select-layout" | "selectl" | "next-layout" | "nextl" => {
            let o = getopt(a, "Enop", "t")?;
            let id = ctx.state()?.resolve(o.val('t'))?.v["id"].clone();
            ctx.call("layout.equalize", json!({ "pane": id }))?;
            Ok(0)
        }
        "display-message" | "display" => {
            let o = getopt(a, "aIlNpv", "cdtF")?;
            let p = ctx.state()?.resolve(o.val('t'))?.clone();
            let fmt = o.val('F').map(str::to_string).or_else(|| joined(&o.args)).unwrap_or_default();
            if o.has('p') {
                let line = ctx.state()?.expand(&fmt, &p);
                ctx.out.push(line);
            }
            Ok(0)
        }
        "list-panes" | "lsp" => {
            let o = getopt(a, "as", "tF")?;
            let lines = {
                let st = ctx.state()?;
                let t = st.resolve(o.val('t'))?.clone();
                let fmt = o.val('F').unwrap_or("#{pane_index}: [#{pane_width}x#{pane_height}] #{pane_id}#{?pane_active, (active),}");
                st.panes
                    .iter()
                    .filter(|p| o.has('a') || (p.ws == t.ws && (o.has('s') || p.surface == t.surface)))
                    .filter(|p| p.v["browser"] != true)
                    .map(|p| st.expand(fmt, p))
                    .collect::<Vec<_>>()
            };
            ctx.out.extend(lines);
            Ok(0)
        }
        "list-windows" | "lsw" => {
            let o = getopt(a, "a", "tF")?;
            let lines = {
                let st = ctx.state()?;
                let wsi = if o.has('a') { None } else { Some(st.session_for(o.val('t'))?) };
                let fmt = o.val('F').unwrap_or("#{window_index}: #{window_name}#{?window_active,*,} (#{window_panes} panes) [#{window_width}x#{window_height}]");
                let mut out = Vec::new();
                for (wi, w) in st.workspaces().iter().enumerate() {
                    if wsi.is_some_and(|x| x != wi) {
                        continue;
                    }
                    for si in 0..w["surfaces"].as_array().map_or(0, |a| a.len()) {
                        if let Some(p) = st.active_in_surface(wi, si) {
                            out.push(st.expand(fmt, p));
                        }
                    }
                }
                out
            };
            ctx.out.extend(lines);
            Ok(0)
        }
        "list-sessions" | "ls" => {
            let o = getopt(a, "", "fF")?;
            let lines = {
                let st = ctx.state()?;
                let fmt = o.val('F').unwrap_or("#{session_name}: #{session_windows} windows#{?session_attached, (attached),}");
                (0..st.workspaces().len()).filter_map(|wi| st.active_in_surface(wi, st.active_surface(wi)).map(|p| st.expand(fmt, p))).collect::<Vec<_>>()
            };
            ctx.out.extend(lines);
            Ok(0)
        }
        "has-session" | "has" => {
            let o = getopt(a, "", "t")?;
            let st = ctx.state()?;
            Ok(match o.val('t') {
                Some(t) => {
                    if st.session_for(Some(t)).is_ok() {
                        0
                    } else {
                        eprintln!("can't find session: {t}");
                        1
                    }
                }
                None => st.workspaces().is_empty() as i32,
            })
        }
        "capture-pane" | "capturep" => {
            let o = getopt(a, "aCeJNpPqM", "bEST")?;
            let id = ctx.state()?.resolve(o.val('t'))?.v["id"].clone();
            let back = o.val('S').and_then(|s| if s == "-" { Some(5000) } else { s.parse::<i64>().ok().filter(|n| *n < 0).map(|n| (-n) as usize) }).unwrap_or(0);
            let v = ctx.call("pane.read", json!({ "pane": id, "scrollback": back }))?;
            if o.has('p') {
                let text = v["text"].as_str().unwrap_or_default().trim_end_matches('\n').to_string();
                ctx.out.push(text);
            }
            Ok(0)
        }
        "respawn-pane" | "respawnp" => bail!("respawn-pane is not supported; use Restart on the pane"),
        "wait-for" | "wait" => {
            let o = getopt(a, "LSU", "")?;
            let name = o.args.first().ok_or_else(|| anyhow!("channel name required"))?.clone();
            let mode = if o.has('S') { "signal" } else if o.has('L') { "lock" } else if o.has('U') { "unlock" } else { "wait" };
            ctx.call("tmux.wait", json!({ "name": name, "mode": mode }))?;
            Ok(0)
        }
        "start-server" | "start" => Ok(0),
        // accepted so scripts that configure tmux keep working; they have no effect here
        "set-option" | "set" | "set-window-option" | "setw" | "set-hook" | "bind-key" | "bind" | "unbind-key" | "unbind" | "source-file" | "source"
        | "refresh-client" | "refresh" | "set-environment" | "setenv" | "select-pane-style" | "display-panes" | "displayp" | "detach-client" | "detach"
        | "set-buffer" | "setb" | "delete-buffer" | "deleteb" => Ok(0),
        "show-options" | "show" | "show-window-options" | "showw" | "show-environment" | "showenv" | "list-clients" | "lsc" | "list-keys" | "lsk" | "show-hooks" => {
            Ok(0)
        }
        other => bail!("unknown command: {other} (not supported by Hivetty's tmux shim)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn state() -> State {
        State::new(json!({
            "focused": "b",
            "pid": 7,
            "workspaces": [
                { "id": "w1", "num": 1, "index": 0, "name": "main", "active": true, "surfaces": [
                    { "id": "s1", "num": 1, "index": 0, "title": null, "active": true, "zoomed": false, "width": 200, "height": 50, "panes": [
                        { "id": "a", "num": 1, "index": 0, "active": false, "title": "pwsh", "cwd": "C:\\dev", "pid": 10, "width": 100, "height": 50, "left": 0.0, "top": 0.0, "dead": false, "profile": "pwsh" },
                        { "id": "b", "num": 2, "index": 1, "active": true, "title": null, "cwd": null, "pid": 11, "width": 99, "height": 50, "left": 0.5, "top": 0.0, "dead": false, "profile": "pwsh" }
                    ]},
                    { "id": "s2", "num": 4, "index": 1, "title": "logs", "active": false, "zoomed": false, "width": 80, "height": 24, "panes": [
                        { "id": "c", "num": 5, "index": 0, "active": true, "title": "tail", "width": 80, "height": 24, "profile": "pwsh" }
                    ]}
                ]},
                { "id": "w2", "num": 3, "index": 1, "name": "agents", "active": false, "surfaces": [
                    { "id": "s3", "num": 2, "index": 0, "title": null, "active": true, "width": 80, "height": 24, "panes": [
                        { "id": "d", "num": 3, "index": 0, "active": true, "width": 80, "height": 24, "profile": "pwsh" }
                    ]}
                ]}
            ]
        }))
    }

    #[test]
    fn getopt_combined_and_values() {
        let o = getopt(&s(&["-dPh", "-F#{pane_id}", "-c", "C:\\x", "claude", "--resume"]), "bdfhvIPZ", "cetlpF").unwrap();
        assert!(o.has('d') && o.has('P') && o.has('h'));
        assert_eq!(o.val('F'), Some("#{pane_id}"));
        assert_eq!(o.val('c'), Some("C:\\x"));
        assert_eq!(o.args, s(&["claude", "--resume"]));
        assert!(getopt(&s(&["-q"]), "d", "t").is_err());
        assert!(getopt(&s(&["-t"]), "d", "t").is_err());
    }

    #[test]
    fn command_chains() {
        assert_eq!(split_commands(s(&["split-window", "-h", ";", "select-layout", "tiled"])), vec![s(&["split-window", "-h"]), s(&["select-layout", "tiled"])]);
        assert_eq!(split_commands(s(&["a", "x;", "b"])), vec![s(&["a", "x"]), s(&["b"])]);
    }

    #[test]
    fn targets() {
        std::env::remove_var("TMUX_PANE");
        let st = state();
        let id = |t: Option<&str>| st.resolve(t).unwrap().v["id"].as_str().unwrap().to_string();
        assert_eq!(id(None), "b"); // focused
        assert_eq!(id(Some("%1")), "a");
        assert_eq!(id(Some("@4")), "c");
        assert_eq!(id(Some("agents")), "d");
        assert_eq!(id(Some("$3")), "d");
        assert_eq!(id(Some("main:1")), "c");
        assert_eq!(id(Some("main:0.0")), "a");
        assert_eq!(id(Some(":.0")), "a");
        assert_eq!(id(Some("main:logs")), "c");
        assert!(st.resolve(Some("%99")).is_err());
        assert!(st.resolve(Some("nope:0")).is_err());
        assert_eq!(st.session_for(Some("agents:0")).unwrap(), 1);
    }

    #[test]
    fn formats() {
        let st = state();
        let a = st.by_id("a").unwrap();
        assert_eq!(st.expand("#{pane_id} #{session_name}:#{window_index}.#{pane_index}", a), "%1 main:0.0");
        assert_eq!(st.expand("#D #S #I #P ##", a), "%1 main 0 0 #");
        assert_eq!(st.expand("#{?pane_active,yes,no} #{=2:session_name}", a), "no ma");
        assert_eq!(st.expand("#{window_panes} #{pane_width}x#{pane_height} #{pane_current_path}", a), "2 100x50 C:\\dev");
        assert_eq!(st.expand("#{pane_left}", st.by_id("b").unwrap()), "100");
        assert_eq!(st.expand("#{window_name}", st.by_id("c").unwrap()), "logs");
        assert_eq!(st.expand("#{unknown}x", a), "x");
    }

    #[test]
    fn keys() {
        assert_eq!(keys_to_bytes(&s(&["echo hi", "Enter"]), false), b"echo hi\r".to_vec());
        assert_eq!(keys_to_bytes(&s(&["C-c"]), false), vec![3]);
        assert_eq!(keys_to_bytes(&s(&["M-x", "Up", "F5"]), false), b"\x1bx\x1b[A\x1b[15~".to_vec());
        assert_eq!(keys_to_bytes(&s(&["Enter"]), true), b"Enter".to_vec());
        assert_eq!(size_fraction("30%", 200.0), Some(0.3));
        assert_eq!(size_fraction("50", 200.0), Some(0.25));
        assert_eq!(size_fraction("50", 0.0), None);
    }
}
