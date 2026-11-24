use anyhow::{anyhow, bail, Context, Result};
use hivetty_core::protocol::{pipe_name as default_pipe, Request, Response, ENV_PIPE};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

pub fn pipe_name() -> String {
    std::env::var(ENV_PIPE).unwrap_or_else(|_| default_pipe(&std::env::var("USERNAME").unwrap_or_else(|_| "user".into())))
}

pub struct Client {
    w: File,
    r: BufReader<File>,
    next: u64,
}

const ERROR_PIPE_BUSY: i32 = 231;

impl Client {
    pub fn connect() -> Result<Self> {
        Self::connect_timeout(Duration::from_secs(3))
    }

    pub fn connect_timeout(timeout: Duration) -> Result<Self> {
        let name = pipe_name();
        let start = Instant::now();
        loop {
            match OpenOptions::new().read(true).write(true).open(&name) {
                Ok(f) => {
                    let r = BufReader::new(f.try_clone()?);
                    return Ok(Client { w: f, r, next: 1 });
                }
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && start.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!("Hivetty is not running (no pipe at {name})"),
                Err(e) => return Err(e).with_context(|| format!("connecting to {name}")),
            }
        }
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        let mut line = serde_json::to_vec(&Request::new(id, method, params))?;
        line.push(b'\n');
        self.w.write_all(&line)?;
        self.w.flush()?;
        let mut buf = String::new();
        if self.r.read_line(&mut buf)? == 0 {
            bail!("Hivetty closed the connection");
        }
        let resp: Response = serde_json::from_str(&buf)?;
        if let Some(e) = resp.error {
            return Err(anyhow!("{}", e.message));
        }
        Ok(resp.result.unwrap_or(Value::Null))
    }
}
