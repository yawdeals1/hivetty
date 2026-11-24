import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { chordsFor, prettyChord } from "../lib/keymap";
import { panesOf, paneLabel, useStore } from "../state/store";
import type { Id, Model } from "../types";

interface Item {
  pane: Id;
  workspace: string;
  tab: string;
  title: string | null;
  body: string;
  ts: number | null;
  count: number;
}

function ago(ts: number) {
  const s = Math.max(0, Math.round((Date.now() - ts) / 1000));
  if (s < 60) return "now";
  if (s < 3600) return `${Math.round(s / 60)}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}

/** One row per session (pane) that finished or needs the user, newest first. */
function sessionsNeedingYou(m: Model): Item[] {
  const where = new Map<Id, { workspace: string; tab: string }>();
  for (const w of m.workspaces) {
    for (const s of w.surfaces) {
      for (const p of panesOf(s.root)) where.set(p, { workspace: w.name, tab: s.title ?? paneLabel(m.panes[p]) ?? "Terminal" });
    }
  }
  const byPane = new Map<Id, Item>();
  for (const n of m.notifications) {
    if (n.read || !where.has(n.pane)) continue;
    const prev = byPane.get(n.pane);
    byPane.set(n.pane, { pane: n.pane, ...where.get(n.pane)!, title: n.title, body: n.body, ts: n.ts, count: (prev?.count ?? 0) + 1 });
  }
  // a bell (no notification) also marks a pane
  for (const [id, p] of Object.entries(m.panes)) {
    if (p.unread && !byPane.has(id) && where.has(id)) byPane.set(id, { pane: id, ...where.get(id)!, title: null, body: "Terminal bell", ts: null, count: 1 });
  }
  return [...byPane.values()].sort((a, b) => (b.ts ?? 0) - (a.ts ?? 0));
}

export function AttentionMenu() {
  const model = useStore((s) => s.model);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ bottom: number; left: number } | null>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const pop = useRef<HTMLDivElement>(null);
  const items = sessionsNeedingYou(model);

  const close = () => setOpen(false);

  // Native browser webviews draw above HTML: hide them while the popup is open.
  useEffect(() => {
    useStore.getState().set({ menuOpen: open });
    return () => useStore.getState().set({ menuOpen: false });
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !btn.current) return;
    const r = btn.current.getBoundingClientRect();
    // Bottom-left corner of the sidebar: open upward, aligned to the button's left edge.
    setPos({ bottom: window.innerHeight - r.top + 6, left: Math.max(8, r.left) });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!pop.current?.contains(t) && !btn.current?.contains(t)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close();
        btn.current?.focus();
      }
    };
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", close);
    };
  }, [open]);

  // Focus the first row when opened from the keyboard or mouse, so arrows/Enter work.
  useEffect(() => {
    if (open) pop.current?.querySelector<HTMLElement>(".att-item")?.focus();
  }, [open, pos]);

  const go = (pane: Id) => {
    close();
    api.focusPane(pane);
  };

  const onListKey = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const rows = [...(pop.current?.querySelectorAll<HTMLElement>(".att-item") ?? [])];
    const i = rows.indexOf(document.activeElement as HTMLElement);
    rows[(i + (e.key === "ArrowDown" ? 1 : -1) + rows.length) % rows.length]?.focus();
  };

  const n = items.length;
  const jump = chordsFor("notifications.jumpUnread")[0];

  return (
    <>
      <button
        ref={btn}
        className={`att-btn ${n ? "has" : ""} ${open ? "open" : ""}`}
        title={n ? `${n} session${n === 1 ? "" : "s"} need${n === 1 ? "s" : ""} you` : "No sessions need you"}
        aria-label={n ? `${n} session${n === 1 ? "" : "s"} finished or need${n === 1 ? "s" : ""} attention` : "Sessions: none need attention"}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
          <path d="M8 1.5a4.5 4.5 0 0 0-4.5 4.5v2.6L2.2 11h11.6l-1.3-2.4V6A4.5 4.5 0 0 0 8 1.5Z" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round" />
          <path d="M6.2 12.8a1.9 1.9 0 0 0 3.6 0" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
        </svg>
        <span className="att-label">{n ? "Needs you" : "Agents"}</span>
        {n > 0 && <span className="att-count">{n > 9 ? "9+" : n}</span>}
      </button>
      {open && pos && (
        <div ref={pop} className="att-pop" role="dialog" aria-label="Sessions that need you" style={{ bottom: pos.bottom, left: pos.left }} onKeyDown={onListKey}>
          <div className="att-head">
            <span>{n ? "Needs you" : "All caught up"}</span>
            {n > 0 && (
              <button
                className="att-link"
                onClick={() => {
                  api.markAllRead();
                  close();
                }}
              >
                Mark all read
              </button>
            )}
          </div>
          {n === 0 ? (
            <div className="att-empty">When an agent finishes or asks for input, its session shows up here.</div>
          ) : (
            <div className="att-list">
              {items.map((it) => (
                <button key={it.pane} className="att-item" onClick={() => go(it.pane)}>
                  <span className="att-row">
                    <span className="att-where">
                      {it.workspace} <span className="att-sep">›</span> {it.tab}
                    </span>
                    {it.ts !== null && <span className="att-time">{ago(it.ts)}</span>}
                  </span>
                  <span className="att-body">
                    {it.title && <b>{it.title}: </b>}
                    {it.body}
                    {it.count > 1 && <span className="att-more"> +{it.count - 1}</span>}
                  </span>
                </button>
              ))}
            </div>
          )}
          <div className="att-foot">
            <button
              className="att-link"
              onClick={() => {
                close();
                useStore.getState().set({ notificationsOpen: true });
              }}
            >
              All notifications
            </button>
            {jump && <span className="att-hint">Jump to latest: {prettyChord(jump)}</span>}
          </div>
        </div>
      )}
    </>
  );
}
