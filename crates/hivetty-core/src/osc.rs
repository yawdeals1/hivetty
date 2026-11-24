//! Streaming tap over PTY output that extracts events Hivetty cares about
//! (titles, cwd, prompt marks, agent notifications, bell). Bytes still flow to xterm.js
//! unchanged; this only observes them. Safe to feed arbitrarily split chunks.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TermEvent {
    Title { title: String },
    Cwd { path: String },
    Notify { title: Option<String>, body: String },
    PromptStart,
    CommandStart,
    CommandExecuted,
    CommandFinished { exit_code: Option<i32> },
    Progress { state: u8, percent: u8 },
    Bell,
    /// DSR 6 (`CSI 6 n`): the host must reply with the cursor position. ConPTY sends this at
    /// startup and blocks output until answered.
    CursorPositionRequest,
}

#[derive(Default)]
struct Collector {
    events: Vec<TermEvent>,
    /// kitty OSC 99 chunks keyed by notification id
    kitty: HashMap<String, (Option<String>, String)>,
}

pub struct OscTap {
    parser: vte::Parser,
    c: Collector,
}

impl Default for OscTap {
    fn default() -> Self {
        Self { parser: vte::Parser::new(), c: Collector::default() }
    }
}

impl OscTap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw output bytes; returns any events completed by this chunk.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TermEvent> {
        self.parser.advance(&mut self.c, bytes);
        std::mem::take(&mut self.c.events)
    }
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn join(params: &[&[u8]]) -> String {
    params.iter().map(|p| s(p)).collect::<Vec<_>>().join(";")
}

/// Convert an OSC 7 `file://host/path` URI to a local path (Windows-aware).
pub fn file_uri_to_path(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    // drop host
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => rest,
    };
    let decoded = percent_decode(path);
    let b = decoded.as_bytes();
    // /C:/foo -> C:\foo
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return decoded[1..].replace('/', "\\");
    }
    // /c/foo (msys) -> C:\foo
    if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/') {
        let drive = (b[1] as char).to_ascii_uppercase();
        let tail = if b.len() > 2 { &decoded[2..] } else { "/" };
        return format!("{drive}:{}", tail.replace('/', "\\"));
    }
    decoded
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl vte::Perform for Collector {
    fn execute(&mut self, byte: u8) {
        if byte == 0x07 {
            self.events.push(TermEvent::Bell);
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], _ignore: bool, action: char) {
        if action == 'n' && intermediates.is_empty() {
            let first = params.iter().next().and_then(|p| p.first().copied());
            if first == Some(6) {
                self.events.push(TermEvent::CursorPositionRequest);
            }
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let Some(code) = params.first() else { return };
        match *code {
            b"0" | b"2" => self.events.push(TermEvent::Title { title: join(&params[1..]) }),
            b"7" => {
                let uri = join(&params[1..]);
                if !uri.is_empty() {
                    self.events.push(TermEvent::Cwd { path: file_uri_to_path(&uri) });
                }
            }
            b"9" => self.osc9(&params[1..]),
            b"777" => {
                // OSC 777 ; notify ; title ; body
                if params.get(1).map(|p| *p == b"notify").unwrap_or(false) {
                    let title = params.get(2).map(|t| s(t)).filter(|t| !t.is_empty());
                    let body = if params.len() > 3 { join(&params[3..]) } else { String::new() };
                    self.events.push(TermEvent::Notify { title, body });
                }
            }
            b"99" => self.osc99(&params[1..]),
            b"133" => {
                let kind = params.get(1).map(|p| s(p)).unwrap_or_default();
                let ev = match kind.as_str() {
                    "A" => TermEvent::PromptStart,
                    "B" => TermEvent::CommandStart,
                    "C" => TermEvent::CommandExecuted,
                    "D" => TermEvent::CommandFinished {
                        exit_code: params.get(2).and_then(|p| s(p).parse().ok()),
                    },
                    _ => return,
                };
                self.events.push(ev);
            }
            _ => {}
        }
    }
}

impl Collector {
    fn osc9(&mut self, rest: &[&[u8]]) {
        // ConEmu/Windows Terminal extensions: 9;4 progress, 9;9 cwd. Everything else is a notification (iTerm2 style).
        match rest.first().copied() {
            Some(b"4") => {
                let state = rest.get(1).and_then(|p| s(p).parse().ok()).unwrap_or(0);
                let percent = rest.get(2).and_then(|p| s(p).parse().ok()).unwrap_or(0);
                self.events.push(TermEvent::Progress { state, percent });
            }
            Some(b"9") => {
                let p = join(&rest[1..]);
                let p = p.trim_matches('"').to_string();
                if !p.is_empty() {
                    self.events.push(TermEvent::Cwd { path: p });
                }
            }
            _ => {
                let body = join(rest);
                if !body.is_empty() {
                    self.events.push(TermEvent::Notify { title: None, body });
                }
            }
        }
    }

    fn osc99(&mut self, rest: &[&[u8]]) {
        // kitty: OSC 99 ; key=val:key=val ; payload
        let meta = rest.first().map(|m| s(m)).unwrap_or_default();
        let payload = if rest.len() > 1 { join(&rest[1..]) } else { String::new() };
        let mut id = String::new();
        let mut done = true;
        let mut part = "title".to_string();
        let mut base64 = false;
        for kv in meta.split(':') {
            if let Some((k, v)) = kv.split_once('=') {
                match k {
                    "i" => id = v.to_string(),
                    "d" => done = v != "0",
                    "p" => part = v.to_string(),
                    "e" => base64 = v == "1",
                    _ => {}
                }
            }
        }
        let payload = if base64 { b64_decode(&payload).unwrap_or(payload) } else { payload };
        let entry = self.kitty.entry(id.clone()).or_default();
        match part.as_str() {
            "title" => entry.0 = Some(entry.0.take().unwrap_or_default() + &payload),
            "body" => entry.1.push_str(&payload),
            _ => {}
        }
        if done {
            let (title, body) = self.kitty.remove(&id).unwrap_or_default();
            // kitty: if only a title is given, it is shown as the notification text
            let (title, body) = if body.is_empty() { (None, title.unwrap_or_default()) } else { (title, body) };
            if !body.is_empty() {
                self.events.push(TermEvent::Notify { title, body });
            }
        }
    }
}

fn b64_decode(input: &str) -> Option<String> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in input.bytes().filter(|c| *c != b'=') {
        let v = T.iter().position(|t| *t == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(input: &[u8]) -> Vec<TermEvent> {
        OscTap::new().feed(input)
    }

    #[test]
    fn osc9_notification_with_semicolons() {
        assert_eq!(
            all(b"\x1b]9;Claude needs input; approve?\x07"),
            vec![TermEvent::Notify { title: None, body: "Claude needs input; approve?".into() }]
        );
    }

    #[test]
    fn osc9_progress_and_cwd_not_notifications() {
        assert_eq!(all(b"\x1b]9;4;1;50\x1b\\"), vec![TermEvent::Progress { state: 1, percent: 50 }]);
        assert_eq!(all(b"\x1b]9;9;\"C:\\dev\"\x07"), vec![TermEvent::Cwd { path: "C:\\dev".into() }]);
    }

    #[test]
    fn osc777() {
        assert_eq!(
            all(b"\x1b]777;notify;Codex;Task done\x07"),
            vec![TermEvent::Notify { title: Some("Codex".into()), body: "Task done".into() }]
        );
    }

    #[test]
    fn osc99_chunked_and_base64() {
        let ev = all(b"\x1b]99;i=1:d=0;Hello\x1b\\\x1b]99;i=1:p=body:e=1;d29ybGQ=\x1b\\");
        assert_eq!(ev, vec![TermEvent::Notify { title: Some("Hello".into()), body: "world".into() }]);
        assert_eq!(all(b"\x1b]99;;Just title\x1b\\"), vec![TermEvent::Notify { title: None, body: "Just title".into() }]);
    }

    #[test]
    fn split_across_chunks() {
        let mut tap = OscTap::new();
        assert!(tap.feed(b"hello \x1b]0;my ti").is_empty());
        assert_eq!(tap.feed(b"tle\x07world"), vec![TermEvent::Title { title: "my title".into() }]);
    }

    #[test]
    fn prompt_marks_and_bell() {
        assert_eq!(
            all(b"\x1b]133;A\x07$ \x1b]133;D;2\x07\x07"),
            vec![TermEvent::PromptStart, TermEvent::CommandFinished { exit_code: Some(2) }, TermEvent::Bell]
        );
    }

    #[test]
    fn cursor_position_request() {
        let mut tap = OscTap::new();
        assert!(tap.feed(b"\x1b[").is_empty());
        assert_eq!(tap.feed(b"6n"), vec![TermEvent::CursorPositionRequest]);
        assert!(tap.feed(b"\x1b[5n\x1b[?6n").is_empty());
    }

    #[test]
    fn osc7_paths() {
        assert_eq!(file_uri_to_path("file://host/C:/Users/me/My%20Dir"), "C:\\Users\\me\\My Dir");
        assert_eq!(file_uri_to_path("file://host/c/dev/x"), "C:\\dev\\x");
        assert_eq!(file_uri_to_path("file:///home/me"), "/home/me");
    }
}
