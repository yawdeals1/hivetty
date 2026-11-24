import { useEffect, useRef, useState } from "react";
import type { BrowserProfile, ImportReport, Pane } from "../types";
import { api } from "../lib/api";
import { useStore } from "../state/store";

export function BrowserPane({ pane, focused, multi }: { pane: Pane; focused: boolean; multi: boolean }) {
  const slot = useRef<HTMLDivElement>(null);
  const url = pane.kind.type === "browser" ? pane.kind.url : "";
  const [draft, setDraft] = useState(url);
  const [editing, setEditing] = useState(false);
  const [suggests, setSuggests] = useState<{ url: string; title: string }[]>([]);
  const [sel, setSel] = useState(-1);
  const [importing, setImporting] = useState(false);
  // Native webviews always draw above the HTML UI, so hide them while any overlay is open.
  const overlay = useStore((s) => s.overlay !== null || s.menuOpen);
  const flash = useStore((s) => s.flashPane === pane.id);
  const menuOpen = suggests.length > 0 || importing;

  useEffect(() => {
    if (!editing) setDraft(url);
  }, [url, editing]);

  // Address-bar suggestions from imported history.
  useEffect(() => {
    if (!editing || !draft.trim() || draft === url) {
      setSuggests([]);
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      api.browserSuggest(draft).then((r) => {
        if (live) {
          setSuggests(r.slice(0, 8));
          setSel(-1);
        }
      });
    }, 80);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [draft, editing, url]);

  // Native webviews draw above HTML, so hide while an overlay OR our own dropdown/menu is open.
  const hideWebview = overlay || menuOpen;

  // Keep the native webview glued to the placeholder's rect.
  useEffect(() => {
    let raf = 0;
    let last = "";
    const tick = () => {
      const el = slot.current;
      if (el) {
        const r = el.getBoundingClientRect();
        const visible = !hideWebview && r.width > 0 && r.height > 0;
        const key = `${Math.round(r.left)},${Math.round(r.top)},${Math.round(r.width)},${Math.round(r.height)},${visible}`;
        if (key !== last) {
          last = key;
          api.browserSetBounds(pane.id, r.left, r.top, r.width, r.height, visible);
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      api.browserSetBounds(pane.id, 0, 0, 0, 0, false);
    };
  }, [pane.id, hideWebview]);

  const go = (target: string) => {
    setSuggests([]);
    api.browserNavigate(pane.id, target).then(() => api.browserFocus(pane.id));
  };

  const cls = ["pane", "browser-pane", focused && multi ? "focused" : "", pane.unread ? "unread" : "", flash ? "flash" : ""].join(" ");

  return (
    <div className={cls} onMouseDown={() => !focused && api.focusPane(pane.id)}>
      <div className="browser-bar">
        <button title="Back" onClick={() => api.browserHistory(pane.id, "back")}>←</button>
        <button title="Forward" onClick={() => api.browserHistory(pane.id, "forward")}>→</button>
        <button title="Reload" onClick={() => api.browserHistory(pane.id, "reload")}>⟳</button>
        <div className="browser-url-wrap">
          <input
            className="browser-url"
            value={draft}
            spellCheck={false}
            onFocus={(e) => {
              setEditing(true);
              e.currentTarget.select();
            }}
            onBlur={() => setTimeout(() => setEditing(false), 120)}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                go(sel >= 0 && suggests[sel] ? suggests[sel].url : draft);
                e.currentTarget.blur();
              } else if (e.key === "Escape") {
                if (suggests.length) setSuggests([]);
                else {
                  setDraft(url);
                  e.currentTarget.blur();
                }
              } else if (e.key === "ArrowDown" && suggests.length) {
                e.preventDefault();
                setSel((s) => Math.min(suggests.length - 1, s + 1));
              } else if (e.key === "ArrowUp" && suggests.length) {
                e.preventDefault();
                setSel((s) => Math.max(-1, s - 1));
              }
            }}
          />
          {suggests.length > 0 && (
            <div className="url-suggests">
              {suggests.map((s, i) => (
                <div
                  key={s.url + i}
                  className={`url-suggest ${i === sel ? "sel" : ""}`}
                  onMouseEnter={() => setSel(i)}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    go(s.url);
                  }}
                >
                  <span className="us-title">{s.title || s.url}</span>
                  <span className="us-url">{s.url}</span>
                </div>
              ))}
            </div>
          )}
        </div>
        <button title="Import from Chrome/Edge" className={importing ? "on" : ""} onClick={() => setImporting((v) => !v)}>⬇</button>
        <button title="Developer tools" onClick={() => api.browserDevtools(pane.id)}>⚙</button>
      </div>
      {importing && <ImportMenu pane={pane.id} onClose={() => setImporting(false)} />}
      <div ref={slot} className="browser-slot">
        {hideWebview && <div className="browser-hidden">{pane.title ?? url}</div>}
      </div>
    </div>
  );
}

function ImportMenu({ pane, onClose }: { pane: string; onClose: () => void }) {
  const [profiles, setProfiles] = useState<BrowserProfile[] | null>(null);
  const [report, setReport] = useState<ImportReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [opts, setOpts] = useState({ cookies: true, history: true });

  useEffect(() => {
    api.browserProfiles().then(setProfiles).catch(() => setProfiles([]));
  }, []);

  const run = (p: BrowserProfile) => {
    setBusy(p.id);
    setReport(null);
    api
      .browserImport(pane, p.id, opts)
      .then(setReport)
      .catch((e) => setReport({ profile: p.id, browser: p.browser, cookies: { error: String(e) } }))
      .finally(() => setBusy(null));
  };

  return (
    <div className="import-menu" onMouseDown={(e) => e.stopPropagation()}>
      <div className="import-head">
        <span>Import from a browser</span>
        <button className="import-x" onClick={onClose}>×</button>
      </div>
      <div className="import-opts">
        <label className="fif-check"><input type="checkbox" checked={opts.history} onChange={() => setOpts((o) => ({ ...o, history: !o.history }))} /> History (address-bar suggestions)</label>
        <label className="fif-check"><input type="checkbox" checked={opts.cookies} onChange={() => setOpts((o) => ({ ...o, cookies: !o.cookies }))} /> Cookies (keep logged-in sessions)</label>
      </div>
      {profiles === null && <div className="set-hint">Looking for profiles…</div>}
      {profiles?.length === 0 && <div className="set-hint">No Chrome, Edge or Brave profiles found for your user.</div>}
      {profiles?.map((p) => (
        <button key={p.id} className="import-profile" disabled={busy !== null || (!opts.cookies && !opts.history)} onClick={() => run(p)}>
          <span>{p.name}</span>
          <span className="us-url">{p.browser}{busy === p.id ? " · importing…" : ""}</span>
        </button>
      ))}
      {report && (
        <div className="import-report">
          {report.history && <div>{report.history.error ? `History: ${report.history.error}` : `History: ${report.history.imported ?? 0} entries imported`}</div>}
          {report.cookies &&
            (report.cookies.error ? (
              <div>Cookies: {report.cookies.error}</div>
            ) : (
              <div>
                Cookies: {report.cookies.imported ?? 0} imported
                {(report.cookies.app_bound ?? 0) > 0 && <span className="dim"> · {report.cookies.app_bound} skipped (Chrome app-bound encryption — reload the page after signing in instead)</span>}
                {(report.cookies.failed ?? 0) > 0 && <span className="dim"> · {report.cookies.failed} failed</span>}
              </div>
            ))}
          {report.cookies && !report.cookies.error && (report.cookies.imported ?? 0) > 0 && <div className="dim">Reload the page to use the imported session.</div>}
        </div>
      )}
    </div>
  );
}
