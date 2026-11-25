import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { chordsFor, prettyChord } from "../lib/keymap";
import { useStore } from "../state/store";
import type { Notification } from "../types";

function ago(ts: number) {
  const s = Math.max(0, Math.round((Date.now() - ts) / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.round(s / 60)}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}

/** Agents often send Markdown; show it as plain text. */
function plain(text: string) {
  return text
    .replace(/\*\*(.+?)\*\*/g, "$1")
    .replace(/__(.+?)__/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/^\s*[-*]\s+/gm, "• ")
    .replace(/^#+\s+/gm, "");
}

/** Newest first; back-to-back notifications from the same pane and sender become one entry. */
function group(list: Notification[]): Notification[][] {
  const out: Notification[][] = [];
  for (const n of [...list].reverse()) {
    const last = out[out.length - 1];
    if (last && last[0].pane === n.pane && last[0].title === n.title) last.push(n);
    else out.push([n]);
  }
  return out;
}

export function closeNotifications() {
  useStore.getState().set({ notificationsOpen: false });
}

export function NotificationPanel() {
  const model = useStore((s) => s.model);
  const ref = useRef<HTMLElement>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const groups = group(model.notifications);
  const unread = model.notifications.filter((n) => !n.read).length;
  const wsName = (id: string) => model.workspaces.find((w) => w.id === id)?.name ?? "Closed workspace";
  const toggle = chordsFor("notifications.toggle")[0];
  const jump = chordsFor("notifications.jumpUnread")[0];

  useEffect(() => {
    // Take focus when opened so Escape closes the panel right away.
    if (!ref.current?.contains(document.activeElement)) ref.current?.focus({ preventScroll: true });
  }, []);

  const open = (pane: string) => (e: React.KeyboardEvent | React.MouseEvent) => {
    if ("key" in e && e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    api.focusPane(pane);
  };

  return (
    <aside
      ref={ref}
      className="notif-panel"
      aria-label="Notifications"
      tabIndex={-1}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          closeNotifications();
        }
      }}
    >
      <div className="notif-header">
        <h2 className="notif-heading">
          Notifications
          {unread > 0 && <span className="notif-count">{unread} unread</span>}
        </h2>
        <button className="notif-close" onClick={closeNotifications} title={`Close (Esc${toggle ? ` or ${prettyChord(toggle)}` : ""})`} aria-label="Close notifications">
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
            <path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>
      </div>
      {groups.length > 0 && (
        <div className="notif-actions">
          <button onClick={() => api.jumpLatestUnread()} disabled={!unread} title={jump ? `Jump to latest unread (${prettyChord(jump)})` : undefined}>
            Go to latest unread
          </button>
          <button onClick={() => api.markAllRead()} disabled={!unread}>
            Mark all read
          </button>
          <button className="notif-clear" onClick={() => api.clearNotifications()}>
            Clear all
          </button>
        </div>
      )}
      <div className="notif-list">
        {groups.length === 0 && (
          <div className="notif-empty">
            <p>Nothing yet.</p>
            <p>When an agent finishes a task or asks for input, it shows up here. Scripts can post here too with <code>hivetty notify</code>.</p>
          </div>
        )}
        {groups.map((g) => {
          const [n, ...earlier] = g;
          const isUnread = g.some((x) => !x.read);
          const isOpen = expanded.has(n.id);
          return (
            <div key={n.id} className={`notif ${isUnread ? "unread" : ""}`}>
              <div
                className="notif-main"
                role="button"
                tabIndex={0}
                aria-label={`${isUnread ? "Unread: " : ""}${wsName(n.workspace)}: ${n.title ? `${n.title}, ` : ""}${n.body}`}
                onClick={open(n.pane)}
                onKeyDown={open(n.pane)}
              >
                <div className="notif-top">
                  <span className="notif-ws">{wsName(n.workspace)}</span>
                  <span className="notif-time">{ago(n.ts)}</span>
                </div>
                {n.title && <div className="notif-title">{n.title}</div>}
                <div className="notif-body">{plain(n.body)}</div>
              </div>
              {earlier.length > 0 && (
                <>
                  <button
                    className="notif-more"
                    aria-expanded={isOpen}
                    onClick={() =>
                      setExpanded((s) => {
                        const next = new Set(s);
                        if (next.has(n.id)) next.delete(n.id);
                        else next.add(n.id);
                        return next;
                      })
                    }
                  >
                    {isOpen ? "Hide" : "Show"} {earlier.length} earlier
                  </button>
                  {isOpen && (
                    <ol className="notif-earlier">
                      {earlier.map((x) => (
                        <li key={x.id} className={x.read ? "" : "unread"}>
                          <span className="notif-body">{plain(x.body)}</span>
                          <span className="notif-time">{ago(x.ts)}</span>
                        </li>
                      ))}
                    </ol>
                  )}
                </>
              )}
            </div>
          );
        })}
      </div>
    </aside>
  );
}
