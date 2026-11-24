//! Wire format between Hivetty and its detached session host (`hivetty-host.exe`), which owns
//! the ConPTYs when persistent sessions are on so shells outlive the app.
//!
//! Frames: `u32 LE length` (of everything after it) | `u8 kind` | payload.
//! - [`KIND_CONTROL`]: a JSON object. Requests carry `"id"` and `"op"`; replies echo `"id"` with
//!   `"ok": true` plus fields, or `"error"`. Unsolicited events carry `"event"`.
//! - [`KIND_DATA`]: 16-byte pane uuid, then raw bytes (input to the host, output from it).

use serde_json::Value;
use std::io::{Read, Write};
use uuid::Uuid;

pub const KIND_CONTROL: u8 = 0;
pub const KIND_DATA: u8 = 1;
pub const MAX_FRAME: usize = 64 * 1024 * 1024;
pub const ENV_HOST_PIPE: &str = "HIVETTY_HOST_PIPE";

/// Host pipe name for the current user (overridable with `HIVETTY_HOST_PIPE`).
pub fn host_pipe_name(user: &str) -> String {
    if let Ok(p) = std::env::var(ENV_HOST_PIPE) {
        if !p.is_empty() {
            return p;
        }
    }
    let safe: String = user.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!(r"\\.\pipe\hivetty-host-{safe}")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Control(Value),
    Data(Uuid, Vec<u8>),
}

pub fn encode_control(v: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(v).unwrap_or_default();
    let mut out = Vec::with_capacity(5 + body.len());
    out.extend_from_slice(&((body.len() + 1) as u32).to_le_bytes());
    out.push(KIND_CONTROL);
    out.extend_from_slice(&body);
    out
}

pub fn encode_data(pane: Uuid, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(21 + data.len());
    out.extend_from_slice(&((data.len() + 17) as u32).to_le_bytes());
    out.push(KIND_DATA);
    out.extend_from_slice(pane.as_bytes());
    out.extend_from_slice(data);
    out
}

/// Decode one frame body (after the length prefix).
pub fn decode_body(body: &[u8]) -> Result<Frame, String> {
    let (&kind, rest) = body.split_first().ok_or("empty frame")?;
    match kind {
        KIND_CONTROL => serde_json::from_slice(rest).map(Frame::Control).map_err(|e| format!("bad control frame: {e}")),
        KIND_DATA => {
            if rest.len() < 16 {
                return Err("short data frame".into());
            }
            let id = Uuid::from_slice(&rest[..16]).map_err(|e| e.to_string())?;
            Ok(Frame::Data(id, rest[16..].to_vec()))
        }
        k => Err(format!("unknown frame kind {k}")),
    }
}

/// Parse the length prefix; errors on absurd sizes.
pub fn frame_len(prefix: [u8; 4]) -> Result<usize, String> {
    let n = u32::from_le_bytes(prefix) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(format!("bad frame length {n}"));
    }
    Ok(n)
}

/// Blocking read of one frame (CLI side). `Ok(None)` on a clean EOF.
pub fn read_frame(r: &mut impl Read) -> std::io::Result<Option<Frame>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = frame_len(len).map_err(std::io::Error::other)?;
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    decode_body(&body).map(Some).map_err(std::io::Error::other)
}

pub fn write_frame(w: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    w.write_all(bytes)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let id = Uuid::new_v4();
        let mut buf = encode_control(&serde_json::json!({"id": 1, "op": "list"}));
        buf.extend(encode_data(id, b"hello\x1b[0m"));
        buf.extend(encode_data(id, b""));
        let mut r = &buf[..];
        assert_eq!(read_frame(&mut r).unwrap(), Some(Frame::Control(serde_json::json!({"id": 1, "op": "list"}))));
        assert_eq!(read_frame(&mut r).unwrap(), Some(Frame::Data(id, b"hello\x1b[0m".to_vec())));
        assert_eq!(read_frame(&mut r).unwrap(), Some(Frame::Data(id, vec![])));
        assert_eq!(read_frame(&mut r).unwrap(), None);
    }

    #[test]
    fn rejects_garbage() {
        assert!(frame_len([0, 0, 0, 0]).is_err());
        assert!(frame_len([0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(decode_body(&[9, 1, 2]).is_err());
        assert!(decode_body(&[KIND_DATA, 1, 2]).is_err());
    }

    #[test]
    fn pipe_names() {
        if std::env::var_os(ENV_HOST_PIPE).is_none() {
            assert_eq!(host_pipe_name("Jo Doe"), r"\\.\pipe\hivetty-host-Jo_Doe");
        }
    }
}
