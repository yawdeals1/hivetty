//! `hivetty browser ...` — drive a browser pane (agent-friendly).

use crate::client::Client;
use anyhow::Result;
use clap::Subcommand;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub enum BrowserCmd {
    /// Open a URL in a browser pane (splits one beside the current pane if needed)
    Open { url: String },
    /// Navigate the browser pane to a URL
    #[command(alias = "goto")]
    Navigate { url: String },
    Back,
    Forward,
    Reload,
    /// Print the page's accessibility tree with element refs (e1, e2, ...)
    Snapshot {
        /// Only interactive elements
        #[arg(short, long)]
        interactive: bool,
    },
    /// Click an element by ref (e5 / @e5) or CSS selector
    Click { target: String },
    /// Replace an input's value
    Fill { target: String, text: Vec<String> },
    /// Type text into the focused element
    Type { text: Vec<String> },
    /// Press a key (Enter, Tab, Escape, ArrowDown, a, ...)
    Press { key: String },
    /// Evaluate JavaScript and print the result
    Eval { expr: Vec<String> },
    /// Print the page's visible text
    Text,
    /// Print the current URL and title
    Url,
    /// Show captured console messages
    Console {
        #[arg(long)]
        clear: bool,
    },
    /// Show captured network responses
    Network {
        #[arg(long)]
        clear: bool,
    },
    /// Save a PNG screenshot of the page
    Screenshot {
        #[arg(default_value = "screenshot.png")]
        path: String,
    },
    /// Wait for text or a selector to appear (or just sleep with --ms)
    Wait {
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        selector: Option<String>,
        /// Timeout (or sleep duration when no condition) in milliseconds
        #[arg(long)]
        ms: Option<u64>,
    },
    /// Open WebView2 developer tools for the pane
    Devtools,
    /// List the current user's Chrome/Edge profiles that can be imported
    Profiles,
    /// Import history and cookies from one of your Chrome/Edge profiles into this pane.
    /// Cookies carry over logged-in sessions; modern Chrome's app-bound cookies can't be read.
    Import {
        /// Profile id from `browser profiles`, e.g. edge/Default
        profile: String,
        /// Import only these domains' cookies (repeatable); default: all
        #[arg(long)]
        domain: Vec<String>,
        /// Skip cookies (history only)
        #[arg(long)]
        no_cookies: bool,
        /// Skip history (cookies only)
        #[arg(long)]
        no_history: bool,
    },
}

pub fn run(c: &mut Client, cmd: BrowserCmd, pane: Option<String>, json_out: bool) -> Result<()> {
    let (method, mut params): (&str, Value) = match cmd {
        BrowserCmd::Open { url } => ("open", json!({ "url": url, "near": std::env::var(hivetty_core::protocol::ENV_PANE).ok() })),
        BrowserCmd::Navigate { url } => ("navigate", json!({ "url": url })),
        BrowserCmd::Back => ("back", json!({})),
        BrowserCmd::Forward => ("forward", json!({})),
        BrowserCmd::Reload => ("reload", json!({})),
        BrowserCmd::Snapshot { interactive } => ("snapshot", json!({ "interactive": interactive })),
        BrowserCmd::Click { target } => ("click", json!({ "target": target })),
        BrowserCmd::Fill { target, text } => ("fill", json!({ "target": target, "text": text.join(" ") })),
        BrowserCmd::Type { text } => ("type", json!({ "text": text.join(" ") })),
        BrowserCmd::Press { key } => ("press", json!({ "key": key })),
        BrowserCmd::Eval { expr } => ("eval", json!({ "expr": expr.join(" ") })),
        BrowserCmd::Text => ("text", json!({})),
        BrowserCmd::Url => ("url", json!({})),
        BrowserCmd::Console { clear } => ("console", json!({ "clear": clear })),
        BrowserCmd::Network { clear } => ("network", json!({ "clear": clear })),
        BrowserCmd::Screenshot { path } => {
            let abs = std::path::absolute(&path).map(|p| p.display().to_string()).unwrap_or(path);
            ("screenshot", json!({ "path": abs }))
        }
        BrowserCmd::Wait { text, selector, ms } => ("wait", json!({ "text": text, "target": selector, "ms": ms })),
        BrowserCmd::Devtools => ("devtools", json!({})),
        BrowserCmd::Profiles => ("profiles", json!({})),
        BrowserCmd::Import { profile, domain, no_cookies, no_history } => {
            ("import", json!({ "profile": profile, "domains": domain, "cookies": !no_cookies, "history": !no_history }))
        }
    };
    if let Some(p) = pane {
        params["pane"] = json!(p);
    }
    let v = c.call(&format!("browser.{method}"), params)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    match method {
        "snapshot" => {
            if let Some(u) = v["url"].as_str() {
                println!("# {u}");
            }
            print!("{}", v["snapshot"].as_str().unwrap_or_default());
        }
        "text" => println!("{}", v["text"].as_str().unwrap_or_default()),
        "url" => println!("{}\n{}", v["url"].as_str().unwrap_or_default(), v["title"].as_str().unwrap_or_default()),
        "eval" => match &v["value"] {
            Value::String(s) => println!("{s}"),
            Value::Null => {}
            other => println!("{}", serde_json::to_string_pretty(other)?),
        },
        "console" => {
            for e in v["entries"].as_array().into_iter().flatten() {
                println!("[{}] {}", e["level"].as_str().unwrap_or("log"), e["text"].as_str().unwrap_or_default());
            }
        }
        "network" => {
            for e in v["entries"].as_array().into_iter().flatten() {
                if let Some(err) = e["error"].as_str() {
                    println!("FAIL {} {}", err, e["type"].as_str().unwrap_or_default());
                } else {
                    println!("{} {} {}", e["status"], e["type"].as_str().unwrap_or_default(), e["url"].as_str().unwrap_or_default());
                }
            }
        }
        "screenshot" => println!("{}", v["path"].as_str().unwrap_or_default()),
        "open" => println!("{}", v["pane"].as_str().unwrap_or_default()),
        "profiles" => {
            for p in v.as_array().into_iter().flatten() {
                println!("{:<16} {} ({})", p["id"].as_str().unwrap_or_default(), p["name"].as_str().unwrap_or_default(), p["browser"].as_str().unwrap_or_default());
            }
        }
        "import" => {
            if let Some(h) = v.get("history").filter(|h| !h.is_null()) {
                match h.get("error").and_then(|e| e.as_str()) {
                    Some(e) => println!("history: {e}"),
                    None => println!("history: imported {} entries", h["imported"].as_u64().unwrap_or(0)),
                }
            }
            if let Some(c) = v.get("cookies").filter(|c| !c.is_null()) {
                match c.get("error").and_then(|e| e.as_str()) {
                    Some(e) => println!("cookies: {e}"),
                    None => {
                        print!("cookies: imported {}", c["imported"].as_u64().unwrap_or(0));
                        if c["app_bound"].as_u64().unwrap_or(0) > 0 {
                            print!(", {} skipped (app-bound encryption)", c["app_bound"].as_u64().unwrap_or(0));
                        }
                        if c["failed"].as_u64().unwrap_or(0) > 0 {
                            print!(", {} failed", c["failed"].as_u64().unwrap_or(0));
                        }
                        println!();
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}
