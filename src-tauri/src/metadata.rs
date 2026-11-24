//! Background watcher that keeps per-pane sidebar metadata fresh: git branch/dirty state,
//! GitHub PR status (via `gh`), and TCP ports listened on by the pane's process tree.

use crate::app::{emit_state, AppState};
use hivetty_core::layout::PaneId;
use hivetty_core::model::{GitInfo, PrInfo};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const TICK: Duration = Duration::from_secs(2);
const DIRTY_TTL: Duration = Duration::from_secs(5);
const PR_TTL: Duration = Duration::from_secs(90);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Default)]
struct Cache {
    dirty: HashMap<String, (Instant, bool)>,
    pr: Arc<Mutex<HashMap<(String, String), (Instant, Option<PrInfo>)>>>,
    pr_inflight: Arc<Mutex<HashSet<(String, String)>>>,
    gh: Option<bool>,
}

pub fn start(app: AppHandle) {
    if std::env::var("HIVETTY_NO_METADATA").is_ok() {
        return;
    }
    std::thread::Builder::new()
        .name("metadata".into())
        .spawn(move || {
            let mut cache = Cache::default();
            loop {
                tick(&app, &mut cache);
                std::thread::sleep(TICK);
            }
        })
        .expect("spawn metadata thread");
}

fn tick(app: &AppHandle, cache: &mut Cache) {
    let st = app.state::<AppState>();
    let panes: Vec<(PaneId, Option<String>)> = st.model.lock().panes.values().map(|p| (p.id, p.local_cwd.clone())).collect();
    if panes.is_empty() {
        return;
    }
    let pids: HashMap<PaneId, u32> = st.ptys.pids().into_iter().collect();

    // --- git (per unique cwd)
    let mut git_by_cwd: HashMap<String, Option<GitInfo>> = HashMap::new();
    for (_, cwd) in &panes {
        if let Some(cwd) = cwd {
            if !git_by_cwd.contains_key(cwd) {
                let info = git_info(cwd, cache);
                git_by_cwd.insert(cwd.clone(), info);
            }
        }
    }

    // --- ports
    let ports_by_pane = if pids.is_empty() { HashMap::new() } else { ports_for(&pids) };

    // --- apply
    let mut changed = false;
    {
        let mut m = st.model.lock();
        for (id, cwd) in &panes {
            let Some(p) = m.panes.get_mut(id) else { continue };
            let git = cwd.as_ref().and_then(|c| git_by_cwd.get(c).cloned().flatten());
            if p.git != git {
                p.git = git;
                changed = true;
            }
            let ports = ports_by_pane.get(id).cloned().unwrap_or_default();
            if p.ports != ports {
                p.ports = ports;
                changed = true;
            }
        }
    }
    if changed {
        emit_state(app);
    }
}

// ---------- git ----------

/// Walk up from `cwd` to find the directory containing `.git` (dir or worktree file).
fn find_git(cwd: &str) -> Option<(PathBuf, PathBuf)> {
    let mut dir = Some(Path::new(cwd));
    while let Some(d) = dir {
        let dotgit = d.join(".git");
        if dotgit.is_dir() {
            return Some((d.to_path_buf(), dotgit));
        }
        if dotgit.is_file() {
            // worktree / submodule: "gitdir: <path>"
            let text = std::fs::read_to_string(&dotgit).ok()?;
            let gd = text.trim().strip_prefix("gitdir:")?.trim();
            let gd = PathBuf::from(gd);
            let gd = if gd.is_absolute() { gd } else { d.join(gd) };
            return Some((d.to_path_buf(), gd));
        }
        dir = d.parent();
    }
    None
}

pub fn read_head(gitdir: &Path) -> Option<(String, bool)> {
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        Some((r.strip_prefix("refs/heads/").unwrap_or(r).to_string(), false))
    } else {
        Some((head.chars().take(7).collect(), true))
    }
}

fn git_info(cwd: &str, cache: &mut Cache) -> Option<GitInfo> {
    let (root, gitdir) = find_git(cwd)?;
    let (branch, detached) = read_head(&gitdir)?;
    let root_s = root.display().to_string();

    let dirty = match cache.dirty.get(&root_s) {
        Some((at, d)) if at.elapsed() < DIRTY_TTL => *d,
        _ => {
            let d = Command::new("git")
                .args(["--no-optional-locks", "status", "--porcelain", "--untracked-files=no"])
                .current_dir(&root)
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .map(|o| o.status.success() && !o.stdout.is_empty())
                .unwrap_or(false);
            cache.dirty.insert(root_s.clone(), (Instant::now(), d));
            d
        }
    };

    let pr = if detached { None } else { pr_info(&root_s, &branch, cache) };
    Some(GitInfo { root: root_s, branch, detached, dirty, pr })
}

/// Cached PR lookup; refreshes asynchronously so the tick never waits on the network.
fn pr_info(root: &str, branch: &str, cache: &mut Cache) -> Option<PrInfo> {
    if matches!(branch, "main" | "master" | "trunk" | "develop") {
        return None;
    }
    let gh = *cache.gh.get_or_insert_with(|| which::which("gh").is_ok());
    if !gh {
        return None;
    }
    let key = (root.to_string(), branch.to_string());
    let (current, stale) = match cache.pr.lock().get(&key) {
        Some((at, pr)) => (pr.clone(), at.elapsed() >= PR_TTL),
        None => (None, true),
    };
    if stale && cache.pr_inflight.lock().insert(key.clone()) {
        let store = cache.pr.clone();
        let inflight = cache.pr_inflight.clone();
        std::thread::spawn(move || {
            let pr = fetch_pr(&key.0, &key.1);
            store.lock().insert(key.clone(), (Instant::now(), pr));
            inflight.lock().remove(&key);
        });
    }
    current
}

fn fetch_pr(root: &str, branch: &str) -> Option<PrInfo> {
    let out = Command::new("gh")
        .args(["pr", "view", branch, "--json", "number,title,url,state,isDraft,statusCheckRollup"])
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_pr(&serde_json::from_slice(&out.stdout).ok()?)
}

pub fn parse_pr(v: &serde_json::Value) -> Option<PrInfo> {
    let checks = v.get("statusCheckRollup").and_then(|c| c.as_array()).map(|items| {
        let mut pending = false;
        for it in items {
            let concl = it.get("conclusion").and_then(|c| c.as_str()).unwrap_or("");
            let state = it.get("state").and_then(|c| c.as_str()).unwrap_or("");
            let status = it.get("status").and_then(|c| c.as_str()).unwrap_or("");
            let failed = matches!(concl, "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE") || matches!(state, "FAILURE" | "ERROR");
            if failed {
                return "failing";
            }
            if (status != "" && status != "COMPLETED") || state == "PENDING" || state == "EXPECTED" {
                pending = true;
            }
        }
        if items.is_empty() {
            "none"
        } else if pending {
            "pending"
        } else {
            "passing"
        }
    });
    Some(PrInfo {
        number: v.get("number")?.as_u64()?,
        title: v.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
        url: v.get("url").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
        state: v.get("state").and_then(|t| t.as_str()).unwrap_or("OPEN").to_string(),
        draft: v.get("isDraft").and_then(|t| t.as_bool()).unwrap_or(false),
        checks: checks.unwrap_or("none").to_string(),
    })
}

// ---------- ports ----------

fn ports_for(pane_pids: &HashMap<PaneId, u32>) -> HashMap<PaneId, Vec<u16>> {
    let listeners = listening_ports();
    if listeners.is_empty() {
        return HashMap::new();
    }
    let children = process_children();
    let mut out = HashMap::new();
    for (pane, root) in pane_pids {
        let mut seen = HashSet::new();
        let mut stack = vec![*root];
        let mut ports = Vec::new();
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(ps) = listeners.get(&pid) {
                ports.extend_from_slice(ps);
            }
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids);
            }
        }
        ports.sort_unstable();
        ports.dedup();
        if !ports.is_empty() {
            out.insert(*pane, ports);
        }
    }
    out
}

/// pid -> listening TCP ports (IPv4 + IPv6).
fn listening_ports() -> HashMap<u32, Vec<u16>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    let mut out: HashMap<u32, Vec<u16>> = HashMap::new();
    for af in [AF_INET as u32, AF_INET6 as u32] {
        let mut size: u32 = 0;
        unsafe { GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, af, TCP_TABLE_OWNER_PID_LISTENER, 0) };
        if size == 0 {
            continue;
        }
        // u32-aligned buffer
        let mut buf = vec![0u32; (size as usize).div_ceil(4) + 4];
        size = (buf.len() * 4) as u32;
        let rc = unsafe { GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut size, 0, af, TCP_TABLE_OWNER_PID_LISTENER, 0) };
        if rc != 0 {
            continue;
        }
        let n = buf[0] as usize;
        let base = unsafe { buf.as_ptr().add(1) };
        for i in 0..n {
            let (pid, port) = unsafe {
                if af == AF_INET as u32 {
                    let row = &*(base as *const MIB_TCPROW_OWNER_PID).add(i);
                    (row.dwOwningPid, row.dwLocalPort)
                } else {
                    let row = &*(base as *const MIB_TCP6ROW_OWNER_PID).add(i);
                    (row.dwOwningPid, row.dwLocalPort)
                }
            };
            let port = u16::from_be(port as u16);
            if pid > 4 && port != 0 {
                out.entry(pid).or_default().push(port);
            }
        }
    }
    out
}

/// parent pid -> child pids
fn process_children() -> HashMap<u32, Vec<u32>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return map;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                if e.th32ParentProcessID != e.th32ProcessID {
                    map.entry(e.th32ParentProcessID).or_default().push(e.th32ProcessID);
                }
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pr_checks_rollup() {
        let base = json!({"number": 12, "title": "Add x", "url": "u", "state": "OPEN", "isDraft": false});
        let with = |rollup: serde_json::Value| {
            let mut v = base.clone();
            v["statusCheckRollup"] = rollup;
            parse_pr(&v).unwrap().checks
        };
        assert_eq!(with(json!([{"status":"COMPLETED","conclusion":"SUCCESS"}])), "passing");
        assert_eq!(with(json!([{"status":"IN_PROGRESS","conclusion":""},{"status":"COMPLETED","conclusion":"SUCCESS"}])), "pending");
        assert_eq!(with(json!([{"status":"COMPLETED","conclusion":"FAILURE"}])), "failing");
        assert_eq!(with(json!([{"state":"PENDING"}])), "pending");
        assert_eq!(with(json!([])), "none");
        assert_eq!(parse_pr(&base).unwrap().number, 12);
    }

    #[test]
    fn head_parsing() {
        let d = std::env::temp_dir().join(format!("hivetty-head-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(read_head(&d), Some(("feature/x".into(), false)));
        std::fs::write(d.join("HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(read_head(&d), Some(("0123456".into(), true)));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn listening_ports_smoke() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let map = listening_ports();
        assert!(map.get(&std::process::id()).is_some_and(|p| p.contains(&port)), "own listener {port} not found");
        let kids = process_children();
        assert!(!kids.is_empty());
    }
}
