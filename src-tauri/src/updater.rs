//! In-app updates (tauri-plugin-updater) with stable and nightly channels.
//!
//! Releases are published on GitHub (see .github/workflows/release.yml): every published release
//! carries a `latest.json`, so stable builds read the newest release's
//! (`releases/latest/download/latest.json`) and nightly builds read the rolling `nightly`
//! pre-release's. `updates.endpoint` in hivetty.json overrides both (`{{channel}}` in it becomes
//! `stable` or `nightly`), and builds can bake in other URLs with HIVETTY_UPDATE_ENDPOINT /
//! HIVETTY_UPDATE_ENDPOINT_NIGHTLY. Packages are verified against the public key in
//! tauri.conf.json before anything installs, and nothing installs without the user saying so.

use crate::app::AppState;
use parking_lot::Mutex;
use serde::Serialize;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const STABLE_ENDPOINT: &str = match option_env!("HIVETTY_UPDATE_ENDPOINT") {
    Some(u) => u,
    None => "https://github.com/yawdeals1/hivetty/releases/latest/download/latest.json",
};
const NIGHTLY_ENDPOINT: &str = match option_env!("HIVETTY_UPDATE_ENDPOINT_NIGHTLY") {
    Some(u) => u,
    None => "https://github.com/yawdeals1/hivetty/releases/download/nightly/latest.json",
};
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
    let ch = cfg.channel.as_str();
    let tpl = match cfg.endpoint.filter(|e| !e.trim().is_empty()) {
        Some(e) => e,
        None if ch == "nightly" => NIGHTLY_ENDPOINT.to_string(),
        None => STABLE_ENDPOINT.to_string(),
    };
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
/// The installer is per-machine, so it's started through ShellExecuteEx (which shows the UAC
/// prompt; CreateProcess can't). If the user declines, Hivetty keeps running.
///
/// The plugin's own path starts the installer and then calls `exit()`, which with WebView2 loaded
/// can take long enough that the installer's "is Hivetty running?" check catches the exiting
/// process and aborts ("insufficient permissions to close Hivetty"). Instead the installer gets
/// our pid (`/WAITPID`, see windows/installer-hooks.nsh) and waits for us to be gone, and we
/// terminate immediately (the session was saved; hosted shells keep running).
fn run_nsis_update(app: &AppHandle, version: &str, bytes: &[u8]) -> Result<(), String> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    let dir = std::env::temp_dir().join(format!("hivetty-update-{version}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let exe = dir.join(format!("Hivetty_{version}_x64-setup.exe"));
    std::fs::write(&exe, bytes).map_err(|e| format!("writing {}: {e}", exe.display()))?;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let file = wide(&exe.display().to_string());
    let params = wide(&format!("/P /UPDATE /R /WAITPID={}", std::process::id()));
    let cwd = wide(&dir.display().to_string());
    let verb = wide("open");
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.lpDirectory = cwd.as_ptr();
    info.nShow = 1; // SW_SHOWNORMAL
    // Returns once the installer is running, or fails if the UAC prompt was declined.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let err = std::io::Error::last_os_error();
        return Err(if err.raw_os_error() == Some(1223) { "update cancelled (administrator permission was declined)".into() } else { format!("starting the installer: {err}") });
    }
    if !info.hProcess.is_null() {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(info.hProcess) };
    }
    if let Some(h) = app.state::<AppState>().ptys.existing_host() {
        h.flush();
    }
    unsafe {
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
