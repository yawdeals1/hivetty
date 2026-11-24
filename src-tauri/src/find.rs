//! Find in Files: ripgrep's engine (`ignore` walker + `grep` searcher) over a folder, honoring
//! .gitignore. Results stream to the UI in per-file batches; starting a new search cancels the
//! previous one.

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{sinks::Lossy, BinaryDetection, SearcherBuilder};
use ignore::{overrides::OverrideBuilder, WalkBuilder, WalkState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;

const MAX_HITS: usize = 5000;
const MAX_LINE: usize = 400;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FindOpts {
    pub root: String,
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Comma-separated globs; prefix with ! to exclude (e.g. "*.rs, !target/**").
    pub include: String,
    /// Also search hidden and git-ignored files.
    pub all_files: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hit {
    pub line: u64,
    /// 0-based char column of the match within `text`
    pub col: usize,
    /// match length in chars
    pub len: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileHits {
    pub path: String,
    pub rel: String,
    pub hits: Vec<Hit>,
}

#[derive(Default)]
pub struct FindManager {
    generation: AtomicU64,
}

impl FindManager {
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Start a search; returns its id. Messages: {id, files:[FileHits]} … then {id, done, ...}.
    pub fn start(self: &Arc<Self>, opts: FindOpts, ch: Channel<serde_json::Value>) -> Result<u64, String> {
        let id = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let root = PathBuf::from(opts.root.trim());
        if !root.is_dir() {
            return Err(format!("not a folder: {}", root.display()));
        }
        if opts.query.is_empty() {
            return Err("empty query".into());
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("find".into())
            .spawn(move || {
                let live = || me.generation.load(Ordering::SeqCst) == id;
                let res = search(&root, &opts, &live, &mut |batch| {
                    let _ = ch.send(json!({ "id": id, "files": [batch] }));
                });
                if !live() {
                    return;
                }
                let _ = ch.send(match res {
                    Ok(s) => json!({ "id": id, "done": true, "files_searched": s.files, "matches": s.hits, "truncated": s.truncated }),
                    Err(e) => json!({ "id": id, "done": true, "error": e }),
                });
            })
            .map_err(|e| e.to_string())?;
        Ok(id)
    }
}

#[derive(Debug, Default)]
pub struct Summary {
    pub files: usize,
    pub hits: usize,
    pub truncated: bool,
}

fn clip(line: &str, col: usize, len: usize) -> (String, usize) {
    let chars: Vec<char> = line.trim_end_matches(['\r', '\n']).chars().collect();
    if chars.len() <= MAX_LINE {
        return (chars.into_iter().collect(), col);
    }
    let start = col.saturating_sub(60).min(chars.len());
    let end = (start + MAX_LINE).max(col + len).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    let mut c = col - start;
    if start > 0 {
        s.insert(0, '…');
        c += 1;
    }
    if end < chars.len() {
        s.push('…');
    }
    (s, c)
}

/// Run a search synchronously, calling `emit` once per file with matches.
pub fn search(root: &Path, o: &FindOpts, live: &(dyn Fn() -> bool + Sync), emit: &mut (dyn FnMut(FileHits) + Send)) -> Result<Summary, String> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!o.case_sensitive)
        .word(o.whole_word)
        .fixed_strings(!o.regex)
        .build(&o.query)
        .map_err(|e| e.to_string())?;

    let mut wb = WalkBuilder::new(root);
    wb.hidden(!o.all_files).git_ignore(!o.all_files).git_global(!o.all_files).git_exclude(!o.all_files).ignore(!o.all_files).parents(true).require_git(false);
    wb.filter_entry(|e| e.file_name() != ".git");
    let globs: Vec<&str> = o.include.split(',').map(str::trim).filter(|g| !g.is_empty()).collect();
    if !globs.is_empty() {
        let mut ob = OverrideBuilder::new(root);
        for g in globs {
            ob.add(g).map_err(|e| format!("bad glob `{g}`: {e}"))?;
        }
        wb.overrides(ob.build().map_err(|e| e.to_string())?);
    }
    wb.threads(std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(4));

    let files = AtomicUsize::new(0);
    let hits = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    let emit = parking_lot::Mutex::new(emit);
    wb.build_parallel().run(|| {
        let matcher = matcher.clone();
        let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
        let (files, hits, truncated, emit) = (&files, &hits, &truncated, &emit);
        Box::new(move |entry| {
            if !live() || truncated.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            files.fetch_add(1, Ordering::Relaxed);
            let path = entry.path();
            let mut found = Vec::new();
            let _ = searcher.search_path(
                &matcher,
                path,
                Lossy(|lnum, line| {
                    if !live() {
                        return Ok(false);
                    }
                    let mut start = 0;
                    let bytes = line.as_bytes();
                    // every match on the line, as its own hit (columns differ)
                    while let Ok(Some(m)) = matcher.find_at(bytes, start) {
                        if m.end() == m.start() {
                            break;
                        }
                        let col = line[..m.start()].chars().count();
                        let len = line[m.start()..m.end()].chars().count();
                        let (text, c) = clip(line, col, len);
                        found.push(Hit { line: lnum, col: c, len, text });
                        if hits.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_HITS {
                            truncated.store(true, Ordering::Relaxed);
                            return Ok(false);
                        }
                        start = m.end();
                    }
                    Ok(true)
                }),
            );
            if !found.is_empty() && live() {
                let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
                (emit.lock())(FileHits { path: path.display().to_string(), rel, hits: found });
            }
            WalkState::Continue
        })
    });
    Ok(Summary { files: files.into_inner(), hits: hits.into_inner(), truncated: truncated.into_inner() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> PathBuf {
        let d = std::env::temp_dir().join(format!("hivetty-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::write(d.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(d.join("src/a.rs"), "fn main() {\n    let needle = 1; // Needle needle\n}\n").unwrap();
        std::fs::write(d.join("src/b.txt"), "no match here\nneedles\n").unwrap();
        std::fs::write(d.join("target/out.rs"), "needle\n").unwrap();
        std::fs::write(d.join("bin.dat"), b"needle\x00\x01").unwrap();
        d
    }

    fn run(root: &Path, o: FindOpts) -> (Vec<FileHits>, Summary) {
        let mut out = Vec::new();
        let s = search(root, &o, &|| true, &mut |f| out.push(f)).unwrap();
        out.sort_by(|a, b| a.rel.cmp(&b.rel));
        (out, s)
    }

    #[test]
    fn finds_respecting_gitignore() {
        let d = tree();
        let (files, s) = run(&d, FindOpts { query: "needle".into(), ..Default::default() });
        let rels: Vec<_> = files.iter().map(|f| f.rel.replace('\\', "/")).collect();
        assert_eq!(rels, ["src/a.rs", "src/b.txt"], "target/ is ignored, binaries skipped");
        assert_eq!(files[0].hits.len(), 3, "case-insensitive, every match on the line");
        assert_eq!(files[0].hits[0], Hit { line: 2, col: 8, len: 6, text: "    let needle = 1; // Needle needle".into() });
        assert!(!s.truncated);

        let (files, _) = run(&d, FindOpts { query: "needle".into(), case_sensitive: true, whole_word: true, ..Default::default() });
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].hits.len(), 2);

        let (files, _) = run(&d, FindOpts { query: r"need\w+s".into(), regex: true, ..Default::default() });
        assert_eq!(files.len(), 1);
        assert!(files[0].rel.ends_with("b.txt"));

        let (files, _) = run(&d, FindOpts { query: "needle".into(), include: "*.txt".into(), ..Default::default() });
        assert_eq!(files.len(), 1);
        let (files, _) = run(&d, FindOpts { query: "needle".into(), include: "!*.txt".into(), ..Default::default() });
        assert_eq!(files.len(), 1);
        assert!(files[0].rel.ends_with("a.rs"));

        let (files, _) = run(&d, FindOpts { query: "needle".into(), all_files: true, ..Default::default() });
        assert!(files.iter().any(|f| f.rel.contains("target")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_regex_is_an_error() {
        let r = search(&std::env::temp_dir(), &FindOpts { query: "(".into(), regex: true, ..Default::default() }, &|| true, &mut |_| {});
        assert!(r.is_err());
    }

    #[test]
    fn clips_long_lines() {
        let line = format!("{}MATCH{}", "a".repeat(1000), "b".repeat(1000));
        let (t, c) = clip(&line, 1000, 5);
        assert!(t.chars().count() <= MAX_LINE + 2);
        assert_eq!(t.chars().skip(c).take(5).collect::<String>(), "MATCH");
    }
}
