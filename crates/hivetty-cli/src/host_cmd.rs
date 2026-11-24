//! `hivetty host` (alias `local-tmux`): inspect and control the detached session host directly.

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use hivetty_core::hostproto::{self, Frame};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};

#[derive(Subcommand)]
pub enum HostCmd {
    /// Is the session host running, and how many shells does it hold?
    Status,
    /// List shells held by the session host
    #[command(alias = "ls")]
    List,
    /// End one hosted shell (by pane id or a unique prefix)
    Kill { pane: String },
    /// Stop the session host, ending every shell it holds
    Stop,
}

struct Conn {
    f: File,
    next: u64,
}

impl Conn {
    fn open() -> Result<Option<Conn>> {
        let name = hostproto::host_pipe_name(&std::env::var("USERNAME").unwrap_or_else(|_| "user".into()));
        match OpenOptions::new().read(true).write(true).open(&name) {
            Ok(f) => Ok(Some(Conn { f, next: 1 })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("connecting to {name}")),
        }
    }

    fn call(&mut self, op: &str, mut params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        params["id"] = json!(id);
        params["op"] = json!(op);
        hostproto::write_frame(&mut self.f, &hostproto::encode_control(&params))?;
        loop {
            match hostproto::read_frame(&mut self.f)? {
                None => bail!("session host closed the connection"),
                Some(Frame::Control(v)) if v["id"] == id => {
                    if let Some(e) = v["error"].as_str() {
                        bail!("{e}");
                    }
                    return Ok(v);
                }
                Some(_) => {}
            }
        }
    }
}

pub fn run(cmd: HostCmd, json_out: bool) -> Result<()> {
    let Some(mut c) = Conn::open()? else {
        if json_out {
            println!("{}", json!({ "running": false }));
        } else {
            println!("Session host is not running (turn on Settings → Session → Keep shells running).");
        }
        return Ok(());
    };
    match cmd {
        HostCmd::Status => {
            let hello = c.call("hello", json!({}))?;
            let list = c.call("list", json!({}))?;
            let sessions = list["sessions"].as_array().cloned().unwrap_or_default();
            let live = sessions.iter().filter(|s| s["exited"] != true).count();
            if json_out {
                println!("{}", serde_json::to_string_pretty(&json!({ "running": true, "pid": hello["pid"], "version": hello["version"], "sessions": sessions.len(), "live": live }))?);
            } else {
                println!("Session host {} (pid {}) holds {} shells ({live} running)", hello["version"].as_str().unwrap_or("?"), hello["pid"], sessions.len());
            }
        }
        HostCmd::List => {
            let list = c.call("list", json!({}))?;
            let sessions = list["sessions"].as_array().cloned().unwrap_or_default();
            if json_out {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
            } else if sessions.is_empty() {
                println!("No hosted shells");
            } else {
                for s in sessions {
                    let state = if s["exited"] == true { format!("exited {}", s["code"]) } else if s["attached"] == true { "attached".into() } else { "detached".into() };
                    println!(
                        "{}  pid {:<7} {:<10} {:<12} {}",
                        s["pane"].as_str().unwrap_or_default(),
                        s["pid"],
                        state,
                        s["meta"]["profile"].as_str().unwrap_or("-"),
                        s["meta"]["cwd"].as_str().unwrap_or("")
                    );
                }
            }
        }
        HostCmd::Kill { pane } => {
            let list = c.call("list", json!({}))?;
            let ids: Vec<String> = list["sessions"].as_array().into_iter().flatten().filter_map(|s| s["pane"].as_str().map(str::to_string)).filter(|id| id.starts_with(&pane)).collect();
            let id = match ids.as_slice() {
                [one] => one.clone(),
                [] => return Err(anyhow!("no hosted shell matches {pane}")),
                _ => bail!("{pane} matches {} shells; give more of the id", ids.len()),
            };
            c.call("kill", json!({ "pane": id }))?;
            if !json_out {
                println!("killed {id}");
            }
        }
        HostCmd::Stop => {
            c.call("shutdown", json!({}))?;
            if !json_out {
                println!("Session host stopped");
            }
        }
    }
    Ok(())
}
