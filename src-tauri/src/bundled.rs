//! Helper programs shipped with Hivetty (`tmux.exe`, `hivetty-host.exe`, `hivetty.exe`).
//!
//! The installer puts them next to the app (the CLI goes in `bin\`, the only folder added to
//! PATH). Programs that keep running on their own (the session host) or that go on PATH ahead
//! of everything else (the tmux shim) run from a copy in [`local_bin`] instead, so an update
//! or uninstall never finds the installed files locked by them.

use std::path::{Path, PathBuf};

/// Folder holding the app executable.
pub fn app_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// A shipped program next to the app or in its `bin\` folder.
pub fn find(name: &str) -> Option<PathBuf> {
    let dir = app_dir()?;
    [dir.join(name), dir.join("bin").join(name)].into_iter().find(|p| p.is_file())
}

/// Machine-local folder for copies of shipped programs (`%LOCALAPPDATA%\Hivetty\runtime`;
/// the per-user installer removes it on uninstall).
pub fn local_bin() -> PathBuf {
    if let Some(h) = std::env::var_os("HIVETTY_HOME") {
        return PathBuf::from(h).join("runtime");
    }
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(crate::config::data_dir);
    base.join("Hivetty").join("runtime")
}

fn stale(src: &Path, dst: &Path) -> bool {
    match (std::fs::metadata(src), std::fs::metadata(dst)) {
        (Ok(a), Ok(b)) => a.len() != b.len() || a.modified().ok() > b.modified().ok(),
        _ => true,
    }
}

/// Copy `name` into `<local bin>\<sub>` when the shipped one is newer, and return the copy.
/// If the copy is in use (locked) the existing one is kept.
pub fn installed_copy(name: &str, sub: &str) -> Option<PathBuf> {
    let dir = if sub.is_empty() { local_bin() } else { local_bin().join(sub) };
    let dst = dir.join(name);
    if let Some(src) = find(name) {
        if stale(&src, &dst) {
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = std::fs::copy(&src, &dst) {
                if !dst.is_file() {
                    eprintln!("hivetty: couldn't copy {name} to {}: {e}", dir.display());
                    return None;
                }
            }
        }
    }
    dst.is_file().then_some(dst)
}

/// Remove `<local bin>\<prefix>*` folders other than `keep` (old versions no longer running;
/// folders whose program is still running are locked and stay until next time).
pub fn prune_versions(prefix: &str, keep: &str) {
    let Ok(rd) = std::fs::read_dir(local_bin()) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(prefix) && name != keep && e.path().is_dir() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// PATH for new local terminals: ours, plus the installed CLI's `bin` folder if it isn't on it
/// yet (e.g. Explorer hasn't picked up the installer's PATH change).
pub fn pane_path() -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    let Some(bin) = app_dir().map(|d| d.join("bin")).filter(|b| b.join("hivetty.exe").is_file()) else { return path };
    hivetty_core::pathenv::add(&path, &bin.display().to_string()).unwrap_or(path)
}
