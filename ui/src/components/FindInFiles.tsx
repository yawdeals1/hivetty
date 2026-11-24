import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../lib/api";
import { focusedPane, shortPath, useStore } from "../state/store";
import { closeOverlay } from "./Overlays";
import type { FileHits, FindOpts } from "../types";

const RENDER_CAP = 2000;

interface Status {
  state: "idle" | "searching" | "done" | "error";
  files?: number;
  matches?: number;
  truncated?: boolean;
  error?: string;
}

let remembered: Partial<FindOpts> = {};

function Highlight({ text, col, len }: { text: string; col: number; len: number }) {
  const chars = Array.from(text);
  const lead = chars.slice(0, col).join("");
  // trim long leading indentation so the match is visible
  const shown = lead.length > 40 ? lead.replace(/^\s+/, "") : lead;
  return (
    <>
      {shown}
      <mark>{chars.slice(col, col + len).join("")}</mark>
      {chars.slice(col + len).join("")}
    </>
  );
}

export function FindInFiles() {
  const pane = useStore((s) => focusedPane(s.model));
  const defaultRoot = pane?.git?.root ?? pane?.local_cwd ?? "";
  const [opts, setOpts] = useState<FindOpts>({
    root: remembered.root ?? defaultRoot,
    query: remembered.query ?? "",
    regex: remembered.regex ?? false,
    case_sensitive: remembered.case_sensitive ?? false,
    whole_word: remembered.whole_word ?? false,
    include: remembered.include ?? "",
    all_files: remembered.all_files ?? false,
  });
  const [files, setFiles] = useState<FileHits[]>([]);
  const [status, setStatus] = useState<Status>({ state: "idle" });
  const [sel, setSel] = useState(0);
  const token = useRef(0);
  const list = useRef<HTMLDivElement>(null);
  const query = useRef<HTMLInputElement>(null);

  // flat list of (file, hit) for keyboard navigation
  const flat = useMemo(() => files.flatMap((f) => f.hits.map((h) => ({ f, h }))), [files]);

  useEffect(() => {
    remembered = opts;
    const t = ++token.current;
    if (!opts.query || !opts.root.trim()) {
      api.findCancel();
      setFiles([]);
      setStatus({ state: "idle" });
      return;
    }
    const timer = setTimeout(() => {
      setFiles([]);
      setSel(0);
      setStatus({ state: "searching" });
      const acc: FileHits[] = [];
      let raf = 0;
      const flush = () => {
        raf = 0;
        if (t === token.current) setFiles([...acc].sort((a, b) => a.rel.localeCompare(b.rel)));
      };
      api
        .findStart(opts, (m) => {
          if (t !== token.current) return;
          if (m.done) {
            if (raf) cancelAnimationFrame(raf);
            flush();
            setStatus(m.error ? { state: "error", error: m.error } : { state: "done", files: m.files_searched, matches: m.matches, truncated: m.truncated });
          } else {
            acc.push(...m.files);
            if (!raf) raf = requestAnimationFrame(flush);
          }
        })
        .catch((e) => t === token.current && setStatus({ state: "error", error: String(e) }));
    }, 220);
    return () => clearTimeout(timer);
  }, [opts]);

  useEffect(() => () => void api.findCancel(), []);

  useEffect(() => {
    list.current?.querySelector(".fif-hit.sel")?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  const open = (i: number) => {
    const r = flat[i];
    if (!r) return;
    closeOverlay();
    void api.openFileAt(r.f.path, r.h.line, r.h.col + 1, pane?.id);
  };
  const insert = (i: number) => {
    const r = flat[i];
    if (!r || !pane || pane.kind.type !== "terminal") return;
    const p = /\s/.test(r.f.path) ? `"${r.f.path}"` : r.f.path;
    closeOverlay();
    void api.ptyWrite(pane.id, `${p}:${r.h.line} `);
  };

  const set = (p: Partial<FindOpts>) => setOpts((o) => ({ ...o, ...p }));
  const toggle = (k: "regex" | "case_sensitive" | "whole_word" | "all_files") => setOpts((o) => ({ ...o, [k]: !o[k] }));

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") closeOverlay();
    else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => Math.min(flat.length - 1, s + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => Math.max(0, s - 1));
    } else if (e.key === "Enter" && (e.target as HTMLElement).tagName === "INPUT") {
      e.preventDefault();
      if (e.altKey) insert(sel);
      else open(sel);
    } else if (e.altKey && e.key.toLowerCase() === "c") {
      e.preventDefault();
      toggle("case_sensitive");
    } else if (e.altKey && e.key.toLowerCase() === "w") {
      e.preventDefault();
      toggle("whole_word");
    } else if (e.altKey && e.key.toLowerCase() === "r") {
      e.preventDefault();
      toggle("regex");
    }
  };

  let index = 0;
  let rendered = 0;
  const summary =
    status.state === "searching"
      ? `Searching… ${flat.length} so far`
      : status.state === "error"
        ? status.error
        : status.state === "done"
          ? `${status.matches ?? 0} ${status.matches === 1 ? "match" : "matches"} in ${files.length} ${files.length === 1 ? "file" : "files"}${status.truncated ? " (stopped at 5000)" : ""} · ${status.files ?? 0} files searched`
          : "Type to search. Enter opens, Alt+Enter types the path into the terminal.";

  return (
    <div className="overlay-backdrop" onMouseDown={closeOverlay}>
      <div className="fif" onMouseDown={(e) => e.stopPropagation()} onKeyDown={onKey} role="dialog" aria-label="Find in files">
        <div className="fif-row">
          <input ref={query} autoFocus className="fif-query" placeholder="Find in files" value={opts.query} spellCheck={false} onChange={(e) => set({ query: e.target.value })} aria-label="Search text" />
          <button className={opts.case_sensitive ? "on" : ""} title="Match case (Alt+C)" aria-pressed={opts.case_sensitive} onClick={() => toggle("case_sensitive")}>Aa</button>
          <button className={opts.whole_word ? "on" : ""} title="Whole word (Alt+W)" aria-pressed={opts.whole_word} onClick={() => toggle("whole_word")}>ab</button>
          <button className={opts.regex ? "on" : ""} title="Regular expression (Alt+R)" aria-pressed={opts.regex} onClick={() => toggle("regex")}>.*</button>
        </div>
        <div className="fif-row small">
          <label className="fif-field grow">
            <span>In</span>
            <input value={opts.root} spellCheck={false} onChange={(e) => set({ root: e.target.value })} placeholder="Folder" />
          </label>
          {defaultRoot && opts.root !== defaultRoot && (
            <button title="Search the focused pane's project" onClick={() => set({ root: defaultRoot })}>
              This pane
            </button>
          )}
          <label className="fif-field">
            <span>Files</span>
            <input value={opts.include} spellCheck={false} onChange={(e) => set({ include: e.target.value })} placeholder="*.rs, !tests/**" />
          </label>
          <label className="fif-check" title="Include hidden and git-ignored files">
            <input type="checkbox" checked={opts.all_files} onChange={() => toggle("all_files")} /> Ignored files
          </label>
        </div>
        <div className={`fif-status ${status.state}`}>{summary}</div>
        <div className="fif-results" ref={list}>
          {files.map((f) => {
            if (rendered >= RENDER_CAP) return null;
            return (
              <Fragment key={f.path}>
                <div className="fif-file" title={f.path}>
                  <span className="fif-name">{f.rel.split(/[\\/]/).pop()}</span>
                  <span className="fif-dir">{shortPath(f.rel.replace(/[\\/][^\\/]*$/, ""))}</span>
                  <span className="fif-n">{f.hits.length}</span>
                </div>
                {f.hits.map((h) => {
                  const i = index++;
                  if (rendered++ >= RENDER_CAP) return null;
                  return (
                    <div
                      key={`${h.line}:${h.col}`}
                      className={`fif-hit ${i === sel ? "sel" : ""}`}
                      onMouseEnter={() => setSel(i)}
                      onClick={(e) => (e.altKey ? insert(i) : open(i))}
                    >
                      <span className="fif-line">{h.line}</span>
                      <span className="fif-text">
                        <Highlight text={h.text} col={h.col} len={h.len} />
                      </span>
                    </div>
                  );
                })}
              </Fragment>
            );
          })}
          {flat.length > RENDER_CAP && <div className="palette-hint">Showing the first {RENDER_CAP} results — narrow the search to see more.</div>}
        </div>
      </div>
    </div>
  );
}
