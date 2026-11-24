//! Pure helpers for importing a *local* Chromium profile (Chrome, Edge, Brave, ...) that belongs
//! to the current user into Hivetty's own browser pane: cookie value decryption, timestamp
//! conversion, mapping cookies to CDP `Network.setCookies` parameters, and ranking history.
//!
//! The Windows-specific parts (DPAPI key unwrap, locating profiles, reading the SQLite files) live
//! in the app. This is opt-in and user-initiated; nothing here reaches the network.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::Aes256Gcm;
use serde_json::{json, Value};

/// Seconds between 1601-01-01 (Chromium/Windows epoch) and 1970-01-01.
const EPOCH_DIFF: i64 = 11_644_473_600;

/// Chromium timestamps are microseconds since 1601. Returns Unix seconds, or None for 0 (unset).
pub fn chrome_time_to_unix(us: i64) -> Option<f64> {
    (us > 0).then(|| us as f64 / 1_000_000.0 - EPOCH_DIFF as f64)
}

/// Outcome of decrypting one cookie value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decrypt {
    /// Decrypted plaintext.
    Ok(String),
    /// Legacy per-user DPAPI blob (very old profiles); the app layer unwraps it with DPAPI.
    NeedsDpapi,
    /// App-bound encryption ("v20", Chrome 127+): only the browser itself can decrypt it. We do
    /// NOT attempt to defeat this.
    AppBound,
    Failed,
}

/// Decrypt a cookie's `encrypted_value`. `key` is the AES-256 key read from the profile's
/// `Local State` and already unwrapped with the current user's DPAPI. Cookie DB schema v24+
/// prefixes the plaintext with a 32-byte host hash; pass `strip_host_hash` for those.
pub fn decrypt_cookie(key: Option<&[u8]>, blob: &[u8], strip_host_hash: bool) -> Decrypt {
    if blob.is_empty() {
        return Decrypt::Ok(String::new());
    }
    let prefix = &blob[..blob.len().min(3)];
    if prefix == b"v20" {
        return Decrypt::AppBound;
    }
    if prefix != b"v10" && prefix != b"v11" {
        return Decrypt::NeedsDpapi;
    }
    let Some(key) = key else { return Decrypt::Failed };
    // v10/v11 layout: "v1x" | 12-byte nonce | ciphertext | 16-byte GCM tag.
    if blob.len() < 3 + 12 + 16 {
        return Decrypt::Failed;
    }
    let (nonce, ct) = blob[3..].split_at(12);
    let Ok(cipher) = Aes256Gcm::new_from_slice(key) else { return Decrypt::Failed };
    let Ok(nonce) = nonce.try_into() else { return Decrypt::Failed };
    match cipher.decrypt(nonce, ct) {
        Ok(mut plain) => {
            if strip_host_hash && plain.len() >= 32 {
                plain.drain(..32);
            }
            String::from_utf8(plain).map(Decrypt::Ok).unwrap_or(Decrypt::Failed)
        }
        Err(_) => Decrypt::Failed,
    }
}

/// A cookie row from a Chromium `Cookies` database (value already decrypted).
#[derive(Debug, Clone, PartialEq)]
pub struct Cookie {
    pub host_key: String,
    pub name: String,
    pub value: String,
    pub path: String,
    /// microseconds since 1601; 0 = session cookie.
    pub expires_utc: i64,
    pub secure: bool,
    pub http_only: bool,
    /// -1 unspecified, 0 none, 1 lax, 2 strict.
    pub same_site: i64,
}

/// A host filter: each entry matches that host and its subdomains.
pub fn host_matches(host_key: &str, domains: &[String]) -> bool {
    if domains.is_empty() {
        return true;
    }
    let host = host_key.trim_start_matches('.').to_ascii_lowercase();
    domains.iter().any(|d| {
        let d = d.trim().trim_start_matches('.').to_ascii_lowercase();
        !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
    })
}

impl Cookie {
    /// Parameters for one CDP `Network.setCookie` call (the app sends these to its own WebView2).
    pub fn to_cdp(&self) -> Value {
        let same_site = match self.same_site {
            0 => Some("None"),
            1 => Some("Lax"),
            2 => Some("Strict"),
            _ => None,
        };
        // Chromium stores the leading-dot domain; CDP wants the domain with the dot for host-wide
        // cookies and the bare host otherwise. Passing host_key through preserves that.
        let mut c = json!({
            "name": self.name,
            "value": self.value,
            "domain": self.host_key,
            "path": if self.path.is_empty() { "/".to_string() } else { self.path.clone() },
            "secure": self.secure,
            "httpOnly": self.http_only,
        });
        if let Some(ss) = same_site {
            c["sameSite"] = json!(ss);
        }
        if let Some(exp) = chrome_time_to_unix(self.expires_utc) {
            c["expires"] = json!(exp);
        }
        c
    }
}

/// A history entry from a Chromium `History` database.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    /// microseconds since 1601.
    pub last_visit: i64,
}

/// Rank history for an address-bar suggestion list: most-visited and most-recent first, deduped
/// by URL, skipping blank/`chrome://`/`edge://` internal pages. Returns at most `limit`.
pub fn rank_history(mut entries: Vec<HistoryEntry>, limit: usize) -> Vec<HistoryEntry> {
    entries.retain(|e| {
        let u = e.url.trim();
        !u.is_empty() && !u.starts_with("chrome://") && !u.starts_with("edge://") && !u.starts_with("about:") && !u.starts_with("chrome-extension://")
    });
    // Score blends frequency with recency so a page you open often but not today still ranks.
    entries.sort_by(|a, b| {
        let score = |e: &HistoryEntry| (e.visit_count.max(0) as f64).ln_1p() * 2.0 + e.last_visit as f64 / 1e14;
        score(b).partial_cmp(&score(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut seen = std::collections::HashSet::new();
    entries.retain(|e| seen.insert(e.url.trim_end_matches('/').to_string()));
    entries.truncate(limit);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        // 13335206400000000 us since 1601 == 2024-01-01T00:00:00Z == 1704067200 unix.
        assert_eq!(chrome_time_to_unix(13_348_540_800_000_000), Some(1_704_067_200.0));
        assert_eq!(chrome_time_to_unix(0), None);
    }

    #[test]
    fn rejects_app_bound_and_legacy() {
        assert_eq!(decrypt_cookie(Some(&[0u8; 32]), b"v20blahblah", false), Decrypt::AppBound);
        assert_eq!(decrypt_cookie(Some(&[0u8; 32]), &[1, 2, 3, 4], false), Decrypt::NeedsDpapi);
        assert_eq!(decrypt_cookie(None, b"v10xxxxxxxxxxxxxxxxxxxxxxxxxxxx", false), Decrypt::Failed);
        assert_eq!(decrypt_cookie(Some(&[0u8; 32]), b"", false), Decrypt::Ok(String::new()));
        // too short to hold nonce+tag
        assert_eq!(decrypt_cookie(Some(&[0u8; 32]), b"v10short", false), Decrypt::Failed);
    }

    #[test]
    fn aes_gcm_roundtrip() {
        // Encrypt a value the way Chromium does, then check we recover it (incl. host-hash strip).
        let key = [7u8; 32];
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = [9u8; 12];
        let mut plain = vec![0xABu8; 32]; // fake host hash
        plain.extend_from_slice(b"session=secret-token");
        let ct = cipher.encrypt((&nonce).into(), plain.as_slice()).unwrap();
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ct);
        assert_eq!(decrypt_cookie(Some(&key), &blob, true), Decrypt::Ok("session=secret-token".into()));
        // wrong key fails cleanly
        assert_eq!(decrypt_cookie(Some(&[8u8; 32]), &blob, true), Decrypt::Failed);
    }

    #[test]
    fn host_filter() {
        assert!(host_matches("example.com", &[]));
        assert!(host_matches(".example.com", &["example.com".into()]));
        assert!(host_matches("app.example.com", &["example.com".into()]));
        assert!(!host_matches("evil.com", &["example.com".into()]));
        assert!(!host_matches("notexample.com", &["example.com".into()]));
    }

    #[test]
    fn cookie_to_cdp() {
        let c = Cookie {
            host_key: ".example.com".into(),
            name: "sid".into(),
            value: "abc".into(),
            path: "/".into(),
            expires_utc: 13_348_540_800_000_000,
            secure: true,
            http_only: true,
            same_site: 1,
        };
        let v = c.to_cdp();
        assert_eq!(v["domain"], ".example.com");
        assert_eq!(v["sameSite"], "Lax");
        assert_eq!(v["secure"], true);
        assert_eq!(v["expires"], 1_704_067_200.0);
        // session cookie (no expiry), unspecified same-site
        let mut s = c.clone();
        s.expires_utc = 0;
        s.same_site = -1;
        let v = s.to_cdp();
        assert!(v.get("expires").is_none());
        assert!(v.get("sameSite").is_none());
    }

    #[test]
    fn history_ranking() {
        let e = |url: &str, visits, last| HistoryEntry { url: url.into(), title: url.into(), visit_count: visits, last_visit: last };
        let ranked = rank_history(
            vec![
                e("https://a.com/", 1, 10),
                e("https://b.com/", 50, 5),
                e("chrome://settings", 100, 99),
                e("https://b.com", 50, 5), // dup of b.com
                e("", 1, 1),
            ],
            10,
        );
        assert_eq!(ranked.iter().map(|h| h.url.as_str()).collect::<Vec<_>>(), ["https://b.com/", "https://a.com/"]);
    }
}
