//! Editing a `;`-separated Windows PATH value: the installer uses these (through
//! `hivetty setup-path`) to put Hivetty's `bin` folder on PATH and take it off again.

fn norm(entry: &str) -> String {
    let e = entry.trim().trim_matches('"');
    let e = e.trim_end_matches(['\\', '/']);
    e.replace('/', "\\").to_lowercase()
}

/// Whether `dir` is already one of the entries in `path`.
pub fn contains(path: &str, dir: &str) -> bool {
    let want = norm(dir);
    !want.is_empty() && path.split(';').any(|p| norm(p) == want)
}

/// `path` with `dir` appended, or None when it's already there.
pub fn add(path: &str, dir: &str) -> Option<String> {
    if contains(path, dir) {
        return None;
    }
    let dir = dir.trim_end_matches(['\\', '/']);
    let base = path.trim_end_matches(';');
    Some(if base.is_empty() { dir.to_string() } else { format!("{base};{dir}") })
}

/// `path` without any entry equal to `dir`, or None when it wasn't there.
pub fn remove(path: &str, dir: &str) -> Option<String> {
    if !contains(path, dir) {
        return None;
    }
    let want = norm(dir);
    let kept: Vec<&str> = path.split(';').filter(|p| norm(p) != want && !p.trim().is_empty()).collect();
    Some(kept.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_appends_once() {
        assert_eq!(add("", r"C:\H\bin").as_deref(), Some(r"C:\H\bin"));
        assert_eq!(add(r"C:\a;C:\b;", r"C:\H\bin\").as_deref(), Some(r"C:\a;C:\b;C:\H\bin"));
        assert_eq!(add(r"C:\a;c:\h\BIN\", r"C:\H\bin"), None);
        assert_eq!(add(r#"C:\a;"C:\H\bin""#, r"C:\H\bin"), None);
    }

    #[test]
    fn remove_keeps_other_entries() {
        assert_eq!(remove(r"C:\a;C:\H\bin;%USERPROFILE%\x", r"c:/h/bin").as_deref(), Some(r"C:\a;%USERPROFILE%\x"));
        assert_eq!(remove(r"C:\a", r"C:\H\bin"), None);
        assert_eq!(remove(r"C:\H\bin;C:\H\bin\", r"C:\H\bin").as_deref(), Some(""));
    }

    #[test]
    fn add_then_remove_round_trips() {
        let orig = r"C:\Windows;%LOCALAPPDATA%\Programs\x";
        let added = add(orig, r"C:\Program Files\Hivetty\bin").unwrap();
        assert_eq!(remove(&added, r"C:\Program Files\Hivetty\bin").unwrap(), orig);
    }
}
