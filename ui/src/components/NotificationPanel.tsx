import { api } from "../lib/api";
import { useStore } from "../state/store";

function ago(ts: number) {
  const s = Math.max(0, Math.round((Date.now() - ts) / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.round(s / 60)}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}

export function NotificationPanel() {
  const model = useStore((s) => s.model);
  const list = [...model.notifications].reverse();
  const wsName = (id: string) => model.workspaces.find((w) => w.id === id)?.name ?? "closed";
  return (
    <aside className="notif-panel" aria-label="Notifications">
      <div className="notif-header">
        <span>Notifications</span>
        <div>
          <button onClick={() => api.jumpLatestUnread()} title="Jump to latest unread (Ctrl+Shift+U)">Jump</button>
          <button onClick={() => api.markAllRead()}>Read all</button>
          <button onClick={() => api.clearNotifications()}>Clear</button>
        </div>
      </div>
      <div className="notif-list">
        {list.length === 0 && <div className="notif-empty">No notifications yet. Agents that emit OSC 9/99/777 or run <code>hivetty notify</code> show up here.</div>}
        {list.map((n) => (
          <div
            key={n.id}
            className={`notif ${n.read ? "" : "unread"}`}
            role="button"
            tabIndex={0}
            aria-label={`${n.read ? "" : "Unread: "}${wsName(n.workspace)}: ${n.title ? `${n.title}, ` : ""}${n.body}`}
            onClick={() => api.focusPane(n.pane)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                api.focusPane(n.pane);
              }
            }}
          >
            <div className="notif-top">
              <span className="notif-ws">{wsName(n.workspace)}</span>
              <span className="notif-time">{ago(n.ts)}</span>
            </div>
            {n.title && <div className="notif-title">{n.title}</div>}
            <div className="notif-body">{n.body}</div>
          </div>
        ))}
      </div>
    </aside>
  );
}
