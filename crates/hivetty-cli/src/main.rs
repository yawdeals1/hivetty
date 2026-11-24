//! `hivetty` — control a running Hivetty from any shell or agent.

mod browser_cmd;
mod client;
mod hooks_cmd;
mod host_cmd;
mod setup_path;

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use client::Client;
use hivetty_core::protocol::{ENV_PANE, ENV_WORKSPACE};
use serde_json::{json, Value};
use std::io::IsTerminal;

#[derive(Parser)]
#[command(name = "hivetty", version, about = "Control Hivetty workspaces, panes and notifications")]
struct Cli {
    /// Print raw JSON results
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum DirArg {
    Right,
    Down,
    Left,
    Up,
}

impl DirArg {
    fn as_str(self) -> &'static str {
        match self {
            DirArg::Right => "right",
            DirArg::Down => "down",
            DirArg::Left => "left",
            DirArg::Up => "up",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum HookAgent {
    Claude,
    Codex,
    Gemini,
    Copilot,
    Cursor,
    Opencode,
}

impl From<HookAgent> for hivetty_core::hooks::Agent {
    fn from(a: HookAgent) -> Self {
        use hivetty_core::hooks::Agent;
        match a {
            HookAgent::Claude => Agent::Claude,
            HookAgent::Codex => Agent::Codex,
            HookAgent::Gemini => Agent::Gemini,
            HookAgent::Copilot => Agent::Copilot,
            HookAgent::Cursor => Agent::Cursor,
            HookAgent::Opencode => Agent::OpenCode,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Send a notification to a pane (defaults to the pane this runs in)
    Notify {
        /// Notification text
        body: Vec<String>,
        #[arg(short, long)]
        title: Option<String>,
        #[arg(long, env = ENV_PANE)]
        pane: Option<String>,
        /// Read a coding-agent hook payload (stdin, or the last argument for Codex's legacy notify) instead of BODY
        #[arg(long, value_name = "AGENT")]
        from_hook: Option<HookAgent>,
    },
    /// List workspaces, tabs and panes
    #[command(alias = "ls")]
    List,
    /// Open a new workspace
    #[command(alias = "new")]
    NewWorkspace {
        #[arg(short, long)]
        name: Option<String>,
        #[arg(short, long)]
        profile: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        /// Command to run in the new terminal
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Open a new tab in a workspace
    NewTab {
        #[arg(short, long)]
        workspace: Option<String>,
        #[arg(short, long)]
        profile: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Split a pane
    Split {
        #[arg(value_enum, default_value = "right")]
        dir: DirArg,
        #[arg(long, env = ENV_PANE)]
        pane: Option<String>,
        #[arg(short, long)]
        profile: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        /// Open a browser pane at this URL instead of a terminal
        #[arg(long)]
        url: Option<String>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Type text into a pane (presses Enter unless --no-enter)
    Send {
        text: Vec<String>,
        #[arg(long)]
        pane: Option<String>,
        #[arg(long)]
        no_enter: bool,
    },
    /// Print a pane's screen contents
    Read {
        #[arg(long)]
        pane: Option<String>,
        /// Scroll back this many lines before reading
        #[arg(long, default_value_t = 0)]
        scrollback: usize,
    },
    /// Focus a pane
    Focus { pane: String },
    /// Close a pane (defaults to the focused pane)
    Close { pane: Option<String> },
    /// Switch to a workspace by number, name or id
    Select { workspace: String },
    /// Rename a workspace (defaults to this one)
    Rename {
        name: String,
        #[arg(short, long, env = ENV_WORKSPACE)]
        workspace: Option<String>,
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Jump to the most recent unread notification
    Jump,
    /// Manage coding-agent hooks that forward notifications to Hivetty
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
    /// Print shell completions
    Completion { shell: clap_complete::Shell },
    /// Print ids of the current pane/workspace (from the environment)
    Identify,
    /// Check that Hivetty is reachable
    Ping,
    /// Dump the full app state as JSON
    State,
    /// Show or change settings (hivetty.json)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Save the session now (it is also saved automatically and on exit)
    SaveSession,
    /// List custom commands for the current project
    Commands {
        #[arg(long, env = ENV_PANE)]
        pane: Option<String>,
    },
    /// Run a custom command by name (from hivetty.json or a project's hivetty.json)
    Run {
        name: Vec<String>,
        #[arg(long, env = ENV_PANE)]
        pane: Option<String>,
    },
    /// Open a workspace on a remote machine over ssh (uses the system OpenSSH client)
    Ssh {
        /// Destination: host, user@host, user@host:port or an ~/.ssh/config alias
        destination: String,
        #[arg(short, long)]
        port: Option<u16>,
        /// Private key file
        #[arg(short, long)]
        identity: Option<String>,
        /// Extra ssh option, e.g. -o ForwardAgent=yes (repeatable)
        #[arg(short = 'o', long = "option")]
        options: Vec<String>,
        /// Workspace name (default: the host name)
        #[arg(short, long)]
        name: Option<String>,
        /// Remote directory to start in
        #[arg(long)]
        cwd: Option<String>,
        /// Command to run on the remote host (you get a shell after it exits)
        #[arg(short, long)]
        command: Option<String>,
        /// Command to run, given after `--`
        #[arg(last = true)]
        rest: Vec<String>,
    },
    /// Upload files to an ssh pane's remote directory (like dropping them on it)
    Upload {
        files: Vec<String>,
        #[arg(long, env = ENV_PANE)]
        pane: Option<String>,
        /// Remote directory (default: the pane's current directory)
        #[arg(long)]
        dir: Option<String>,
    },
    /// Inspect or stop the detached session host that keeps shells running
    #[command(alias = "local-tmux")]
    Host {
        #[command(subcommand)]
        action: host_cmd::HostCmd,
    },
    /// Drive a browser pane: open, snapshot, click, fill, eval, screenshot, ...
    Browser {
        /// Target browser pane (defaults to the focused/nearest one)
        #[arg(long, global = true)]
        pane: Option<String>,
        #[command(subcommand)]
        action: browser_cmd::BrowserCmd,
    },
    /// Check for, or install, a Hivetty update (channel: `hivetty config set updates.channel nightly`)
    Update {
        #[arg(value_enum, default_value = "check")]
        action: UpdateAction,
    },
    /// Add or remove a folder on the user's PATH (used by the installer)
    #[command(hide = true)]
    SetupPath {
        dir: String,
        #[arg(long, conflicts_with = "remove", required_unless_present = "remove")]
        add: bool,
        #[arg(long)]
        remove: bool,
        /// Edit the machine-wide PATH instead (needs admin)
        #[arg(long)]
        machine: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum UpdateAction {
    /// Show the last check's result without checking
    Status,
    /// Look for an update on the configured channel
    Check,
    /// Download and install the available update (restarts Hivetty)
    Install,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the settings file path
    Path,
    /// Print the effective settings
    Get {
        /// Dotted key, e.g. font.size (default: everything)
        key: Option<String>,
    },
    /// Change a setting; VALUE is JSON (bare words are taken as strings)
    Set { key: String, value: String },
    /// Remove a setting so its default applies
    Unset { key: String },
    /// Re-read the settings file and report problems
    Reload,
}

#[derive(Subcommand)]
enum HooksAction {
    /// Install hooks (all detected agents unless --agent)
    Setup {
        #[arg(long)]
        agent: Option<HookAgent>,
    },
    /// Remove Hivetty hooks
    Remove {
        #[arg(long)]
        agent: Option<HookAgent>,
    },
    /// Show which agents have Hivetty hooks installed
    Status,
}

fn joined(v: &[String]) -> Option<String> {
    let s = v.join(" ");
    (!s.trim().is_empty()).then_some(s)
}

fn main() {
    let cli = Cli::parse();
    // Hook mode must never break the agent: swallow all errors, exit 0.
    if let Cmd::Notify { from_hook: Some(agent), pane, .. } = &cli.cmd {
        let _ = hooks_cmd::from_hook((*agent).into(), pane.clone());
        // Some agents (Gemini CLI, Codex Stop) require JSON on stdout; `{}` means "no decision".
        println!("{{}}");
        return;
    }
    if let Err(e) = run(cli) {
        eprintln!("hivetty: {e:#}");
        std::process::exit(1);
    }
}

fn print_update(v: &Value) {
    let cur = v["current"].as_str().unwrap_or("?");
    let ch = v["channel"].as_str().unwrap_or("stable");
    match v["state"].as_str().unwrap_or("") {
        "available" => println!("Update available: {cur} -> {} ({ch}). Run `hivetty update install`.", v["version"].as_str().unwrap_or("?")),
        "none" => println!("Hivetty {cur} is up to date ({ch})"),
        "unconfigured" => println!("Hivetty {cur}: this build has no update server (set updates.endpoint)"),
        "error" => println!("Update check failed: {}", v["error"].as_str().unwrap_or("unknown error")),
        s => println!("Hivetty {cur} ({ch}): {s}"),
    }
}

fn print(json_out: bool, v: &Value, human: impl FnOnce(&Value)) {
    if json_out {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    } else {
        human(v);
    }
}

fn run(cli: Cli) -> Result<()> {
    let j = cli.json;
    match cli.cmd {
        Cmd::Completion { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "hivetty", &mut std::io::stdout());
            Ok(())
        }
        Cmd::Identify => {
            let v = json!({
                "pane": std::env::var(ENV_PANE).ok(),
                "workspace": std::env::var(ENV_WORKSPACE).ok(),
                "pipe": client::pipe_name(),
            });
            print(j, &v, |v| {
                for k in ["pane", "workspace", "pipe"] {
                    println!("{k}: {}", v[k].as_str().unwrap_or("-"));
                }
            });
            Ok(())
        }
        Cmd::Hooks { action } => match action {
            HooksAction::Setup { agent } => hooks_cmd::setup(agent.map(Into::into)),
            HooksAction::Remove { agent } => hooks_cmd::remove(agent.map(Into::into)),
            HooksAction::Status => hooks_cmd::status(),
        },
        Cmd::Host { action } => host_cmd::run(action, j),
        Cmd::SetupPath { dir, add, machine, .. } => setup_path::run(&dir, add, machine),
        Cmd::Config { action: ConfigAction::Path } => {
            let base = match std::env::var_os("HIVETTY_HOME") {
                Some(h) => std::path::PathBuf::from(h),
                None => std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Hivetty"),
            };
            println!("{}", base.join("hivetty.json").display());
            Ok(())
        }
        cmd => {
            let mut c = Client::connect()?;
            match cmd {
                Cmd::Config { action } => match action {
                    ConfigAction::Get { key } => {
                        let v = c.call("config.get", json!({}))?;
                        let mut cur = &v["config"];
                        if let Some(k) = &key {
                            for part in k.split('.') {
                                cur = &cur[part];
                            }
                        }
                        println!("{}", serde_json::to_string_pretty(cur)?);
                    }
                    ConfigAction::Set { key, value } => {
                        let value: Value = serde_json::from_str(&value).unwrap_or(Value::String(value));
                        let v = c.call("config.set", json!({ "key": key, "value": value }))?;
                        print(j, &v, print_warnings);
                    }
                    ConfigAction::Unset { key } => {
                        let v = c.call("config.set", json!({ "key": key, "remove": true }))?;
                        print(j, &v, print_warnings);
                    }
                    ConfigAction::Reload => {
                        let v = c.call("config.reload", json!({}))?;
                        print(j, &v, |v| {
                            if let Some(e) = v["error"].as_str() {
                                println!("error: {e} (previous settings kept)");
                            } else {
                                println!("reloaded {}", v["path"].as_str().unwrap_or_default());
                            }
                            print_warnings(v);
                        });
                    }
                    ConfigAction::Path => unreachable!(),
                },
                Cmd::Update { action } => {
                    let method = match action {
                        UpdateAction::Status => "update.status",
                        UpdateAction::Check => "update.check",
                        UpdateAction::Install => {
                            let st = c.call("update.check", json!({}))?;
                            if st["state"] != "available" {
                                print(j, &st, print_update);
                                return Ok(());
                            }
                            "update.install"
                        }
                    };
                    let v = c.call(method, json!({}))?;
                    if v["started"] == true {
                        print(j, &v, |_| println!("Installing; Hivetty will restart"));
                    } else {
                        print(j, &v, print_update);
                    }
                }
                Cmd::SaveSession => {
                    let v = c.call("session.save", json!({}))?;
                    print(j, &v, |v| println!("saved {}", v["path"].as_str().unwrap_or_default()));
                }
                Cmd::Commands { pane } => {
                    let v = c.call("commands.list", json!({ "pane": pane }))?;
                    print(j, &v, |v| {
                        for cmd in v["commands"].as_array().into_iter().flatten() {
                            let src = cmd["source"].as_str().unwrap_or("user settings");
                            println!(
                                "{:<24} {:<12} {}   ({src})",
                                cmd["name"].as_str().unwrap_or_default(),
                                cmd["target"].as_str().unwrap_or_default(),
                                cmd["command"].as_str().unwrap_or_default()
                            );
                        }
                        print_warnings(v);
                    });
                }
                Cmd::Run { name, pane } => {
                    let name = joined(&name).ok_or_else(|| anyhow::anyhow!("command name required"))?;
                    let v = c.call("commands.run", json!({ "name": name, "pane": pane }))?;
                    print(j, &v, |_| {});
                }
                Cmd::Browser { pane, action } => browser_cmd::run(&mut c, action, pane, j)?,
                Cmd::Ssh { destination, port, identity, options, name, cwd, command, rest } => {
                    // ssh -o takes the value separately; accept both "-o X=Y" and bare "X=Y"
                    let options: Vec<String> = options.into_iter().flat_map(|o| if o.starts_with('-') { vec![o] } else { vec!["-o".to_string(), o] }).collect();
                    let command = command.or_else(|| joined(&rest));
                    let v = c.call("workspace.ssh", json!({ "host": destination, "port": port, "identity": identity, "options": options, "name": name, "cwd": cwd, "command": command }))?;
                    print(j, &v, |v| println!("{}", v["pane"].as_str().unwrap_or_default()));
                }
                Cmd::Upload { files, pane, dir } => {
                    if files.is_empty() {
                        bail!("no files given");
                    }
                    let paths: Vec<String> = files.iter().map(|f| std::fs::canonicalize(f).map(|p| p.display().to_string().trim_start_matches(r"\\?\").to_string()).unwrap_or_else(|_| f.clone())).collect();
                    let v = c.call("pane.upload", json!({ "pane": pane, "paths": paths, "dir": dir }))?;
                    print(j, &v, |_| eprintln!("upload started; progress shows on the pane"));
                }
                Cmd::State => {
                    let v = c.call("state", json!({}))?;
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                Cmd::Ping => {
                    let v = c.call("ping", json!({}))?;
                    print(j, &v, |v| println!("Hivetty {} is running", v["version"].as_str().unwrap_or("?")));
                }
                Cmd::Notify { body, title, pane, .. } => {
                    let body = match joined(&body) {
                        Some(b) => b,
                        None if !std::io::stdin().is_terminal() => std::io::read_to_string(std::io::stdin())?.trim().to_string(),
                        None => bail!("notification text required"),
                    };
                    let v = c.call("notify", json!({ "pane": pane, "title": title, "body": body }))?;
                    print(j, &v, |_| {});
                }
                Cmd::List => {
                    let v = c.call("list", json!({}))?;
                    print(j, &v, print_list);
                }
                Cmd::NewWorkspace { name, profile, cwd, command } => {
                    let v = c.call("workspace.new", json!({ "name": name, "profile": profile, "cwd": cwd, "command": joined(&command) }))?;
                    print(j, &v, |v| println!("{}", v["pane"].as_str().unwrap_or_default()));
                }
                Cmd::NewTab { workspace, profile, cwd, command } => {
                    let v = c.call("surface.new", json!({ "workspace": workspace, "profile": profile, "cwd": cwd, "command": joined(&command) }))?;
                    print(j, &v, |v| println!("{}", v["pane"].as_str().unwrap_or_default()));
                }
                Cmd::Split { dir, pane, profile, cwd, url, command } => {
                    let v = c.call("pane.split", json!({ "pane": pane, "dir": dir.as_str(), "profile": profile, "cwd": cwd, "url": url, "command": joined(&command) }))?;
                    print(j, &v, |v| println!("{}", v["pane"].as_str().unwrap_or_default()));
                }
                Cmd::Send { text, pane, no_enter } => {
                    let v = c.call("pane.send", json!({ "pane": pane, "text": text.join(" "), "enter": !no_enter }))?;
                    print(j, &v, |_| {});
                }
                Cmd::Read { pane, scrollback } => {
                    let v = c.call("pane.read", json!({ "pane": pane, "scrollback": scrollback }))?;
                    print(j, &v, |v| print!("{}", v["text"].as_str().unwrap_or_default()));
                }
                Cmd::Focus { pane } => {
                    let v = c.call("pane.focus", json!({ "pane": pane }))?;
                    print(j, &v, |_| {});
                }
                Cmd::Close { pane } => {
                    let v = c.call("pane.close", json!({ "pane": pane }))?;
                    print(j, &v, |_| {});
                }
                Cmd::Select { workspace } => {
                    let v = c.call("workspace.select", json!({ "workspace": workspace }))?;
                    print(j, &v, |_| {});
                }
                Cmd::Rename { name, workspace, description } => {
                    let v = c.call("workspace.rename", json!({ "workspace": workspace, "name": name, "description": description }))?;
                    print(j, &v, |_| {});
                }
                Cmd::Jump => {
                    let v = c.call("notifications.jump", json!({}))?;
                    print(j, &v, |v| if v["pane"].is_null() { println!("No unread notifications") });
                }
                Cmd::Completion { .. } | Cmd::Identify | Cmd::Hooks { .. } | Cmd::Host { .. } | Cmd::SetupPath { .. } => unreachable!(),
            }
            Ok(())
        }
    }
}

fn print_list(v: &Value) {
    for w in v.as_array().into_iter().flatten() {
        let unread = w["unread"].as_u64().unwrap_or(0);
        println!(
            "{}{} {}{}",
            if w["active"].as_bool() == Some(true) { "*" } else { " " },
            w["index"],
            w["name"].as_str().unwrap_or_default(),
            if unread > 0 { format!("  ({unread} unread)") } else { String::new() }
        );
        for (si, s) in w["surfaces"].as_array().into_iter().flatten().enumerate() {
            for p in s["panes"].as_array().into_iter().flatten() {
                let id = p["id"].as_str().unwrap_or_default();
                let focused = s["focused"].as_str() == Some(id);
                let label = p["title"].as_str().or(p["cwd"].as_str()).unwrap_or("");
                let mut flags = String::new();
                if let Some(b) = p["git"]["branch"].as_str() {
                    flags.push_str(&format!(" ({b}{})", if p["git"]["dirty"].as_bool() == Some(true) { "*" } else { "" }));
                }
                if let Some(n) = p["git"]["pr"]["number"].as_u64() {
                    flags.push_str(&format!(" PR#{n} {}", p["git"]["pr"]["checks"].as_str().unwrap_or("")));
                }
                for port in p["ports"].as_array().into_iter().flatten() {
                    flags.push_str(&format!(" :{port}"));
                }
                if p["unread"].as_bool() == Some(true) {
                    flags.push_str(" [unread]");
                }
                if p["running"].as_bool() == Some(true) {
                    flags.push_str(" [running]");
                }
                if p["exited"].as_bool() == Some(true) {
                    flags.push_str(" [exited]");
                }
                println!("    tab {} {} {id}  {label}{flags}", si + 1, if focused { ">" } else { " " });
            }
        }
    }
}

fn print_warnings(v: &Value) {
    for w in v["warnings"].as_array().into_iter().flatten() {
        eprintln!("warning: {}", w.as_str().unwrap_or_default());
    }
}
