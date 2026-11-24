//! JSON-RPC 2.0 framing used on the Hivetty named pipe. One JSON object per line.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ENV_PIPE: &str = "HIVETTY_PIPE";
pub const ENV_WORKSPACE: &str = "HIVETTY_WORKSPACE_ID";
pub const ENV_SURFACE: &str = "HIVETTY_SURFACE_ID";
pub const ENV_PANE: &str = "HIVETTY_PANE_ID";

/// Default pipe name for the current user. `user` should be a stable per-user token (e.g. username).
pub fn pipe_name(user: &str) -> String {
    let safe: String = user.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!(r"\\.\pipe\hivetty-{safe}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id, method: method.into(), params }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
    }
    pub fn err(id: u64, code: i64, message: impl Into<String>) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: None, error: Some(RpcError { code, message: message.into() }) }
    }
}

pub mod codes {
    pub const PARSE: i64 = -32700;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const NOT_FOUND: i64 = 404;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let r = Request::new(7, "notify", serde_json::json!({"body":"hi"}));
        let line = serde_json::to_string(&r).unwrap();
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.method, "notify");
        let resp = serde_json::to_string(&Response::err(7, codes::NOT_FOUND, "no pane")).unwrap();
        assert!(!resp.contains("result"));
    }

    #[test]
    fn pipe_name_sanitized() {
        assert_eq!(pipe_name("Jo Doe"), r"\\.\pipe\hivetty-Jo_Doe");
    }
}
