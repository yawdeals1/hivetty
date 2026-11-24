//! In-app updates (tauri-plugin-updater) with stable and nightly channels.
//!
//! The manifest URL is `updates.endpoint` from hivetty.json, or the one baked in at build time
//! (`HIVETTY_UPDATE_ENDPOINT`); `{{channel}}` in it becomes `stable` or `nightly`. Packages are
//! verified against the public key in tauri.conf.json before anything installs, and nothing
//! installs without the user saying so.

use crate::app::AppState;
use parking_lot::Mutex;
use serde::Serialize;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Built-in manifest URL for release builds (set by scripts/release.ps1).
const BUILTIN_ENDPOINT: Option<&str> = option_env!("HIVETTY_UPDATE_ENDPOINT");
const FIRST_CHECK: Duration = Duration::from_secs(20);
const RECHECK: Duration = Duration::from_secs(6 * 3600);

#[derive(Clone, Serialize, Default)]
pub struct Status {
    /// unconfigured | idle | checking | none | available | downloading | installing | error
    pub state: &'static str,
    pub channel: &'static str,
    /// Running version (the bundle's, so nightly builds report their nightly version).
    pub current: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub date: Option<String>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct UpdaterState {
    pending: Mutex<Option<Update>>,
    status: Mutex<Status>,
}

fn endpoint(app: &AppHandle) -> Option<(String, &'static str)> {
    let st = app.state::<AppState>();
    let cfg = st.config.lock().config.updates.clone();
    let tpl = cfg.endpoint.filter(|e| !e.trim().is_empty()).or_else(|| BUILTIN_ENDPOINT.map(String::from))?;
    let ch = cfg.channel.as_str();
    Some((tpl.replace("{{channel}}", ch), ch))
}

fn set(app: &AppHandle, f: impl FnOnce(&mut Status)) -> Status {
    let us = app.state::<UpdaterState>();
    let snapshot = {
        let mut s = us.status.lock();
        f(&mut s);
        s.current = app.package_info().version.to_string();
        s.clone()
    };
    let _ = app.emit("update-status", &snapshot);
    snapshot
}

pub fn status(app: &AppHandle) -> Status {
    let configured = endpoint(app);
    let us = app.state::<UpdaterState>();
    let mut s = us.status.lock().clone();
    s.current = app.package_info().version.to_string();
    match configured {
        None => s.state = "unconfigured",
        Some((_, ch)) => {
            if s.state.is_empty() || s.state == "unconfigured" {
                s.state = "idle";
            }
            s.channel = ch;
        }
    }
    s
}

/// Look for an update on the configured channel.
pub async fn check(app: &AppHandle) -> Result<Status, String> {
    let Some((url, ch)) = endpoint(app) else {
        return Ok(set(app, |s| *s = Status { state: "unconfigured", ..Default::default() }));
    };
    set(app, |s| {
        s.state = "checking";
        s.channel = ch;
        s.error = None;
    });
    let result = async {
        let url: tauri::Url = url.parse().map_err(|e| format!("bad update URL {url}: {e}"))?;
        let updater = app.updater_builder().endpoints(vec![url]).map_err(|e| e.to_string())?.build().map_err(|e| e.to_string())?;
        updater.check().await.map_err(|e| e.to_string())
    }
    .await;
    match result {
        Ok(Some(u)) => {
            let (version, notes, date) = (u.version.clone(), u.body.clone(), u.date.map(|d| d.to_string()));
            *app.state::<UpdaterState>().pending.lock() = Some(u);
            Ok(set(app, |s| {
                s.state = "available";
                s.version = Some(version);
                s.notes = notes;
                s.date = date;
            }))
        }
        Ok(None) => {
            *app.state::<UpdaterState>().pending.lock() = None;
            Ok(set(app, |s| {
                s.state = "none";
                s.version = None;
                s.notes = None;
            }))
        }
        Err(e) => Ok(set(app, |s| {
            s.state = "error";
            s.error = Some(e);
        })),
    }
}

#[derive(Clone, Serialize)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Download and install the update found by [`check`]. Saves the session first, because the
/// installer closes Hivetty (shells in the session host keep running).
pub async fn install(app: &AppHandle, progress: Channel<Progress>) -> Result<(), String> {
    let update = app.state::<UpdaterState>().pending.lock().take().ok_or("no update to install; check again")?;
    if app.state::<AppState>().config.lock().config.session.restore {
        if let Err(e) = crate::session::save(app) {
            eprintln!("hivetty: saving session before update failed: {e:#}");
        }
    }
    set(app, |s| s.state = "downloading");
    let mut downloaded = 0u64;
    let result = async {
        // download() verifies the package signature against the public key before returning
        let bytes = update
            .download(
                |chunk, total| {
                    downloaded += chunk as u64;
                    let _ = progress.send(Progress { downloaded, total });
                },
                || {},
            )
            .await
            .map_err(|e| e.to_string())?;
        let _ = app.emit("update-status", Status { state: "installing", ..status(app) });
        if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            // MSI (per-machine install): the plugin runs msiexec and exits.
            update.install(bytes).map_err(|e| e.to_string())
        } else {
            run_nsis_update(app, &update.version, &bytes)
        }
    }
    .await;
    match result {
        Ok(()) => app.restart(),
        Err(e) => {
            set(app, |s| {
                s.state = "error";
                s.error = Some(e.clone());
            });
            Err(e)
        }
    }
}

/// Start the NSIS installer in update mode and end this process at once.
///
/// The plugin's own path starts the installer and then calls `exit()`, which with WebView2 loaded
/// can take long enough that the installer's "is Hivetty running?" check catches the exiting
/// process and aborts ("insufficient permissions to close Hivetty"). Instead the installer gets
/// our pid (`/WAITPID`, see windows/installer-hooks.nsh) and waits for us to be gone, and we
/// terminate immediately (the session was saved; hosted shells keep running).
fn run_nsis_update(app: &AppHandle, version: &str, bytes: &[u8]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let dir = std::env::temp_dir().join(format!("hivetty-update-{version}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let exe = dir.join(format!("Hivetty_{version}_x64-setup.exe"));
    std::fs::write(&exe, bytes).map_err(|e| format!("writing {}: {e}", exe.display()))?;
    let args = ["/P".to_string(), "/UPDATE".into(), "/R".into(), format!("/WAITPID={}", std::process::id())];
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    // Leave any job object we're in, so ending this process can't take the installer with it.
    std::process::Command::new(&exe)
        .args(&args)
        .current_dir(&dir)
        .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| std::process::Command::new(&exe).args(&args).current_dir(&dir).spawn())
        .map_err(|e| format!("starting the installer: {e}"))?;
    if let Some(h) = app.state::<AppState>().ptys.existing_host() {
        h.flush();
    }
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), 0);
    }
    Ok(())
}

/// Periodic background checks (when `updates.check` is on). Found updates show in the UI;
/// nothing installs on its own.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let enabled = app.state::<AppState>().config.lock().config.updates.check;
            let busy = matches!(status(&app).state, "downloading" | "installing" | "available");
            if enabled && !busy && endpoint(&app).is_some() {
                let _ = check(&app).await;
            }
            tokio::time::sleep(RECHECK).await;
        }
    });
}
