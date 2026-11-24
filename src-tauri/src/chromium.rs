//! Import the current user's own local Chromium profiles (Edge, Chrome, Brave, ...) into a
//! Hivetty browser pane: address-bar history and, where the profile still uses user-decryptable
//! cookie encryption, cookies. Opt-in and user-initiated.
//!
//! Only the current Windows user's profiles under %LOCALAPPDATA% are read, and DPAPI only ever
//! unwraps keys for the user running Hivetty. Cookies encrypted with Chrome's app-bound scheme
//! ("v20") are reported as skipped, not bypassed. Nothing here touches the network.

use crate::app::AppState;
use crate::browser::BResult;
use base64::Engine;
use hivetty_core::chromium::{self, Cookie, Decrypt, HistoryEntry};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// A Chromium browser + profile the current user has on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct BrowserProfile {
    /// Stable id: "<browser>/<profile dir>", e.g. "edge/Default".
    pub id: String,
    /// "Microsoft Edge", "Google Chrome", ...
    pub browser: String,
    /// Human profile name from the browser's Local State, else the directory name.
    pub name: String,
    #[serde(skip)]
    user_data: PathBuf,
    #[serde(skip)]
    profile_dir: String,
}

struct Vendor {
    browser: &'static str,
    /// subpath under %LOCALAPPDATA%
    rel: &'static str,
}

const VENDORS: &[Vendor] = &[
    Vendor { browser: "Microsoft Edge", rel: r"Microsoft\Edge\User Data" },
    Vendor { browser: "Google Chrome", rel: r"Google\Chrome\User Data" },
    Vendor { browser: "Brave", rel: r"BraveSoftware\Brave-Browser\User Data" },
    Vendor { browser: "Vivaldi", rel: r"Vivaldi\User Data" },
];

fn local_appdata() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

fn vendor_slug(browser: &str) -> &'static str {
    match browser {
        "Microsoft Edge" => "edge",
        "Google Chrome" => "chrome",
        "Brave" => "brave",
        "Vivaldi" => "vivaldi",
        _ => "other",
    }
}

/// Discover the current user's Chromium profiles (those with a `History` database).
pub fn list_profiles() -> Vec<BrowserProfile> {
    let Some(base) = local_appdata() else { return vec![] };
    let mut out = Vec::new();
    for v in VENDORS {
        let user_data = base.join(v.rel);
        if !user_data.is_dir() {
            continue;
        }
        let names = profile_names(&user_data);
        // "Default" plus "Profile N" directories, in a stable order.
        let mut dirs: Vec<String> = std::fs::read_dir(&user_data)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| (n == "Default" || n.starts_with("Profile ")) && user_data.join(n).join("History").is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let name = names.get(&dir).cloned().unwrap_or_else(|| dir.clone());
            out.push(BrowserProfile {
                id: format!("{}/{dir}", vendor_slug(v.browser)),
                browser: v.browser.to_string(),
                name,
                user_data: user_data.clone(),
                profile_dir: dir,
            });
        }
    }
    out
}

/// Map profile dir -> display name, from the browser's top-level `Local State`.
fn profile_names(user_data: &Path) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    if let Ok(text) = std::fs::read_to_string(user_data.join("Local State")) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(cache) = v.pointer("/profile/info_cache").and_then(Value::as_object) {
                for (dir, info) in cache {
                    if let Some(name) = info.get("name").and_then(Value::as_str) {
                        m.insert(dir.clone(), name.to_string());
                    }
                }
            }
        }
    }
    m
}

fn find_profile(id: &str) -> BResult<BrowserProfile> {
    list_profiles().into_iter().find(|p| p.id == id).ok_or_else(|| format!("no such browser profile: {id}"))
}

/// Read the AES key from `Local State` (`os_crypt.encrypted_key`) and unwrap it with DPAPI.
/// Returns None when the key isn't present or can't be unwrapped for this user.
fn cookie_key(user_data: &Path) -> Option<Vec<u8>> {
    let text = std::fs::read_to_string(user_data.join("Local State")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let b64 = v.pointer("/os_crypt/encrypted_key")?.as_str()?;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    // Strip the "DPAPI" prefix, then unprotect.
    let blob = raw.strip_prefix(b"DPAPI")?;
    dpapi_unprotect(blob)
}

/// DPAPI `CryptUnprotectData` for the current user. Returns None on failure (e.g. another user's
/// blob), never panics.
fn dpapi_unprotect(blob: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    use windows_sys::Win32::Foundation::LocalFree;
    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = CryptUnprotectData(&in_blob, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut out);
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let data = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as *mut _);
        Some(data)
    }
}

/// Copy a locked SQLite DB to a temp file (the browser keeps it open) and open it read-only.
fn open_db_copy(src: &Path) -> BResult<(rusqlite::Connection, PathBuf)> {
    if !src.is_file() {
        return Err(format!("not found: {}", src.display()));
    }
    let tmp = std::env::temp_dir().join(format!("hivetty-chromium-{}-{}.db", std::process::id(), fastname(src)));
    std::fs::copy(src, &tmp).map_err(|e| format!("copy {}: {e}", src.display()))?;
    // Some browsers keep a -wal sidecar with unflushed rows; copy it too when present.
    let wal = src.with_extension("db-wal");
    let _ = wal.is_file().then(|| std::fs::copy(&wal, tmp.with_extension("db-wal")));
    let conn = rusqlite::Connection::open_with_flags(&tmp, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open db: {e}"))?;
    Ok((conn, tmp))
}

fn fastname(p: &Path) -> String {
    p.components().rev().take(2).map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("-").chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

fn cleanup(tmp: &Path) {
    let _ = std::fs::remove_file(tmp);
    let _ = std::fs::remove_file(tmp.with_extension("db-wal"));
}

#[derive(Debug, Default, Serialize)]
pub struct CookieImport {
    pub imported: usize,
    /// Cookies skipped because they use app-bound encryption (Chrome 127+).
    pub app_bound: usize,
    pub failed: usize,
}

/// Decrypt the cookies in `profile` matching `domains` (empty = all) and return CDP
/// `Network.setCookie` parameter objects plus a summary.
pub fn read_cookies(profile: &BrowserProfile, domains: &[String]) -> BResult<(Vec<Value>, CookieImport)> {
    let key = cookie_key(&profile.user_data);
    let db = profile.user_data.join(&profile.profile_dir).join("Network").join("Cookies");
    let db = if db.is_file() { db } else { profile.user_data.join(&profile.profile_dir).join("Cookies") };
    let (conn, tmp) = open_db_copy(&db)?;
    let result = read_cookies_from(&conn, key.as_deref(), domains);
    cleanup(&tmp);
    result
}

fn read_cookies_from(conn: &rusqlite::Connection, key: Option<&[u8]>, domains: &[String]) -> BResult<(Vec<Value>, CookieImport)> {
    // Schema v24+ prefixes decrypted values with a 32-byte host hash.
    let version: i64 = conn.query_row("SELECT value FROM meta WHERE key='version'", [], |r| r.get::<_, String>(0)).ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let strip_host_hash = version >= 24;
    let mut stmt = conn
        .prepare("SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite FROM cookies")
        .map_err(|e| format!("query cookies: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })
        .map_err(|e| format!("read cookies: {e}"))?;

    let mut out = Vec::new();
    let mut sum = CookieImport::default();
    for row in rows.flatten() {
        let (host_key, name, plain, enc, path, expires_utc, secure, http_only, same_site) = row;
        if !chromium::host_matches(&host_key, domains) {
            continue;
        }
        let value = if !plain.is_empty() {
            plain
        } else {
            match chromium::decrypt_cookie(key, &enc, strip_host_hash) {
                Decrypt::Ok(v) => v,
                Decrypt::NeedsDpapi => match dpapi_unprotect(&enc) {
                    Some(b) => String::from_utf8_lossy(&b).into_owned(),
                    None => {
                        sum.failed += 1;
                        continue;
                    }
                },
                Decrypt::AppBound => {
                    sum.app_bound += 1;
                    continue;
                }
                Decrypt::Failed => {
                    sum.failed += 1;
                    continue;
                }
            }
        };
        let cookie = Cookie { host_key, name, value, path, expires_utc, secure: secure != 0, http_only: http_only != 0, same_site };
        out.push(cookie.to_cdp());
        sum.imported += 1;
    }
    Ok((out, sum))
}

/// Read ranked address-bar history from `profile`.
pub fn read_history(profile: &BrowserProfile, limit: usize) -> BResult<Vec<HistoryEntry>> {
    let (conn, tmp) = open_db_copy(&profile.user_data.join(&profile.profile_dir).join("History"))?;
    let result = (|| -> BResult<Vec<HistoryEntry>> {
        let mut stmt = conn
            .prepare("SELECT url, title, visit_count, last_visit_time FROM urls WHERE hidden = 0 ORDER BY visit_count DESC LIMIT 4000")
            .map_err(|e| format!("query history: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(HistoryEntry {
                    url: r.get(0)?,
                    title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    visit_count: r.get(2)?,
                    last_visit: r.get(3)?,
                })
            })
            .map_err(|e| format!("read history: {e}"))?;
        Ok(chromium::rank_history(rows.flatten().collect(), limit))
    })();
    cleanup(&tmp);
    result
}

/// Import history and/or cookies from `profile_id` into browser pane `pane`. History is kept in
/// the app for address-bar suggestions; cookies are injected into the pane's WebView2. Returns a
/// per-part summary so the UI can report partial success (e.g. history ok, cookies app-bound).
pub fn import(app: &AppHandle, pane: hivetty_core::layout::PaneId, profile_id: &str, cookies: bool, history: bool, domains: &[String]) -> BResult<Value> {
    let profile = find_profile(profile_id)?;
    let mut report = json!({ "profile": profile.id, "browser": profile.browser });

    if history {
        match read_history(&profile, 3000) {
            Ok(entries) => {
                let n = entries.len();
                *app.state::<AppState>().history.lock() = entries.into_iter().map(|h| (h.url, h.title)).collect();
                report["history"] = json!({ "imported": n });
            }
            Err(e) => report["history"] = json!({ "error": e }),
        }
    }

    if cookies {
        match read_cookies(&profile, domains) {
            Ok((cdp, sum)) => {
                let applied = apply_cookies(app, pane, &cdp);
                report["cookies"] = json!({
                    "imported": sum.imported,
                    "app_bound": sum.app_bound,
                    "failed": sum.failed,
                    "applied": applied.is_ok(),
                });
                if let Err(e) = applied {
                    report["cookies"]["error"] = json!(e);
                }
            }
            Err(e) => report["cookies"] = json!({ "error": e }),
        }
    }
    Ok(report)
}

/// Up to `limit` address-bar suggestions whose URL or title contains `query` (case-insensitive).
pub fn suggest(app: &AppHandle, query: &str, limit: usize) -> Vec<Value> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return vec![];
    }
    app.state::<AppState>()
        .history
        .lock()
        .iter()
        .filter(|(url, title)| url.to_ascii_lowercase().contains(&q) || title.to_ascii_lowercase().contains(&q))
        .take(limit)
        .map(|(url, title)| json!({ "url": url, "title": title }))
        .collect()
}

/// Push cookies into the pane's WebView2 via CDP `Network.setCookies`, so a logged-in session
/// carries over. The webview must have Network enabled (snapshot/automation does this lazily;
/// we enable it here too).
pub fn apply_cookies(app: &AppHandle, pane: hivetty_core::layout::PaneId, cdp_cookies: &[Value]) -> BResult<()> {
    if cdp_cookies.is_empty() {
        return Ok(());
    }
    let st = app.state::<AppState>();
    let _ = st.browsers.cdp(app, pane, "Network.enable", json!({}));
    st.browsers.cdp(app, pane, "Network.setCookies", json!({ "cookies": cdp_cookies })).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_cookies_roundtrip() {
        // Build a tiny Cookies DB like Chromium's and read it back (plaintext values path).
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE meta(key TEXT, value TEXT);
             INSERT INTO meta VALUES('version','20');
             CREATE TABLE cookies(host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB, path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER, samesite INTEGER);
             INSERT INTO cookies VALUES('.example.com','sid','plainval',x'',' /',13348540800000000,1,1,1);
             INSERT INTO cookies VALUES('other.com','x','y',x'',' /',0,0,0,-1);",
        )
        .unwrap();
        let (cookies, sum) = read_cookies_from(&conn, None, &["example.com".into()]).unwrap();
        assert_eq!(sum.imported, 1);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0]["name"], "sid");
        assert_eq!(cookies[0]["value"], "plainval");
        assert_eq!(cookies[0]["sameSite"], "Lax");
    }

    #[test]
    fn counts_app_bound_as_skipped() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE meta(key TEXT, value TEXT);
             INSERT INTO meta VALUES('version','24');
             CREATE TABLE cookies(host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB, path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER, samesite INTEGER);
             INSERT INTO cookies VALUES('a.com','n','',x'7632306162636465',' /',0,1,0,0);",
        )
        .unwrap();
        let (cookies, sum) = read_cookies_from(&conn, Some(&[0u8; 32]), &[]).unwrap();
        assert!(cookies.is_empty());
        assert_eq!(sum.app_bound, 1);
        assert_eq!(sum.imported, 0);
    }
}
