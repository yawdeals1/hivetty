//! Scrubs secret-looking values from text before Hivetty writes it to disk (session files,
//! saved scrollback). Errs on the side of redacting: a masked harmless value costs nothing,
//! a leaked token does.

use regex::{Captures, Regex};
use std::sync::OnceLock;

pub const MASK: &str = "«redacted»";

struct Rules {
    assign: Regex,
    tokens: Regex,
    pem: Regex,
    url_cred: Regex,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| Rules {
        // NAME=value / NAME: value / $env:NAME = "value" where NAME looks like a secret.
        assign: Regex::new(
            r#"(?i)\b([A-Z0-9_.-]*(?:TOKEN|SECRET|PASSWORD|PASSWD|API[_-]?KEY|ACCESS[_-]?KEY|PRIVATE[_-]?KEY|CLIENT[_-]?SECRET|CREDENTIALS?|SESSION[_-]?KEY|CONNECTION[_-]?STRING)[A-Z0-9_]*)("?[ \t]*[=:][ \t]*)("[^"\r\n]*"|'[^'\r\n]*'|[^\s"']+)"#,
        )
        .unwrap(),
        tokens: Regex::new(concat!(
            r"\b(?:",
            r"gh[pousr]_[A-Za-z0-9]{30,}",
            r"|github_pat_[A-Za-z0-9_]{20,}",
            r"|sk-ant-[A-Za-z0-9_-]{16,}",
            r"|sk-(?:proj-)?[A-Za-z0-9_-]{20,}",
            r"|xox[abprs]-[A-Za-z0-9-]{10,}",
            r"|AKIA[0-9A-Z]{16}",
            r"|AIza[0-9A-Za-z_-]{35}",
            r"|glpat-[A-Za-z0-9_-]{20,}",
            r"|npm_[A-Za-z0-9]{36}",
            r"|eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
            r")",
            r"|(?i:\bbearer[ \t]+)[A-Za-z0-9._~+/-]{16,}=*",
        ))
        .unwrap(),
        pem: Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)").unwrap(),
        // scheme://user:password@host
        url_cred: Regex::new(r"\b([a-zA-Z][a-zA-Z0-9+.-]*://[^\s:/@]+:)([^\s@/]+)(@)").unwrap(),
    })
}

/// Return `text` with secret-looking values masked.
pub fn redact(text: &str) -> String {
    let r = rules();
    let t = r.pem.replace_all(text, MASK);
    let t = r.url_cred.replace_all(&t, |c: &Captures| format!("{}{MASK}{}", &c[1], &c[3]));
    let t = r.assign.replace_all(&t, |c: &Captures| {
        let v = &c[3];
        // keep obviously empty/placeholder values readable
        if v.is_empty() || v == "\"\"" || v == "''" {
            c[0].to_string()
        } else {
            format!("{}{}{MASK}", &c[1], &c[2])
        }
    });
    let t = r.tokens.replace_all(&t, |c: &Captures| {
        let m = &c[0];
        if m.to_ascii_lowercase().starts_with("bearer") {
            format!("Bearer {MASK}")
        } else {
            MASK.to_string()
        }
    });
    t.into_owned()
}

/// True for environment variable names that look like they hold secrets.
pub fn is_secret_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "APIKEY", "API_KEY", "ACCESS_KEY", "PRIVATE_KEY", "CREDENTIAL", "SESSION_KEY", "CONNECTION_STRING"]
        .iter()
        .any(|k| n.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_assignments() {
        assert_eq!(redact("export GITHUB_TOKEN=abc123"), format!("export GITHUB_TOKEN={MASK}"));
        assert_eq!(redact("$env:OPENAI_API_KEY = \"x y\""), format!("$env:OPENAI_API_KEY = {MASK}"));
        assert_eq!(redact("password: hunter2"), format!("password: {MASK}"));
        assert_eq!(redact("set DB_CONNECTION_STRING=Server=x;Pwd=y"), format!("set DB_CONNECTION_STRING={MASK}"));
        // a prompt with no value stays as is
        assert_eq!(redact("Password:\nok"), "Password:\nok");
        assert_eq!(redact("TOKEN=\"\""), "TOKEN=\"\"");
        assert_eq!(redact(r#"{"api_key": "abc"}"#), format!(r#"{{"api_key": {MASK}}}"#));
    }

    #[test]
    fn masks_known_tokens() {
        let gh = format!("ghp_{}", "a".repeat(36));
        assert_eq!(redact(&format!("token {gh} end")), format!("token {MASK} end"));
        assert!(!redact("curl -H 'Authorization: Bearer abcdefghijklmnop1234'").contains("abcdefghijklmnop1234"));
        assert!(!redact("key sk-ant-api03-ABCDEFGHIJKLMNOPQRST").contains("ABCDEFGHIJ"));
        assert!(!redact("AKIAABCDEFGHIJKLMNOP").contains("AKIA"));
        assert!(!redact("git clone https://me:s3cr3t@github.com/x").contains("s3cr3t"));
        let pem = "a\n-----BEGIN RSA PRIVATE KEY-----\nMIIE\nabc\n-----END RSA PRIVATE KEY-----\nb";
        assert_eq!(redact(pem), format!("a\n{MASK}\nb"));
    }

    #[test]
    fn leaves_normal_text_alone() {
        for s in ["Author: Jane <j@x.dev>", "cargo test -p hivetty-core", "PATH=C:\\bin", "https://example.com/a:b", "skip-this"] {
            assert_eq!(redact(s), s);
        }
    }

    #[test]
    fn secret_names() {
        assert!(is_secret_name("ANTHROPIC_API_KEY"));
        assert!(is_secret_name("gh_token"));
        assert!(!is_secret_name("PATH"));
        assert!(!is_secret_name("HIVETTY_PANE_ID"));
    }
}
