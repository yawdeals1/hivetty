import { useState } from "react";
import { api } from "../lib/api";
import { paneLabel, panesOf, shortPath, useStore, workspaceUnread } from "../state/store";
import type { Model, Workspace } from "../types";
import { openPort, openUrl } from "../lib/open";

const COLORS = ["", "#f5b53d", "#5fa8ff", "#8fd18b", "#ef6b73", "#c98bf5", "#5fd1d1"];

export function Sidebar() {
  const model = useStore((s) => s.model);
  const info = useStore((s) => s.info);
  const [dragId, setDragId] = useState<string | null>(null);
  const [menu, setMenu] = useState(false);

  return (
    <aside className="sidebar" aria-label="Workspaces">
      <div className="sidebar-header">
        <span className="brand">Hivetty</span>
        <div className="new-ws">
          <button title="New workspace (Ctrl+Shift+N)" aria-label="New workspace" onClick={() => api.newWorkspace()}>
            +
          </button>
          <button title="New workspace with profile" aria-label="New workspace with profile" aria-haspopup="menu" aria-expanded={menu} className="caret" onClick={() => setMenu((m) => !m)}>
            ▾
          </button>
          {menu && (
            <div className="menu" onMouseLeave={() => setMenu(false)}>
              {info?.profiles.map((p) => (
                <div
                  key={p.id}
                  className="menu-item"
                  onClick={() => {
                    setMenu(false);
                    api.newWorkspace({ profile: p.id });
                  }}
                >
                  {p.name}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
      <div className="ws-list">
        {model.workspaces.map((w, i) => (
          <WorkspaceItem
            key={w.id}
            ws={w}
            index={i}
            model={model}
            dragging={dragId === w.id}
            onDragStart={() => setDragId(w.id)}
            onDropAt={(to) => {
              if (dragId) api.moveWorkspace(dragId, to);
              setDragId(null);
            }}
          />
        ))}
      </div>
    </aside>
  );
}

function WorkspaceItem({
  ws,
  index,
  model,
  dragging,
  onDragStart,
  onDropAt,
}: {
  ws: Workspace;
  index: number;
  model: Model;
  dragging: boolean;
  onDragStart: () => void;
  onDropAt: (to: number) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [ctx, setCtx] = useState(false);
  const active = model.active_workspace === ws.id;
  const unread = workspaceUnread(model, ws.id);
  const surface = ws.surfaces.find((s) => s.id === ws.active_surface);
  const focused = surface ? model.panes[surface.focused] : undefined;
  const latest = [...model.notifications].reverse().find((n) => n.workspace === ws.id);
  const anyUnreadPane = ws.surfaces.some((s) => panesOf(s.root).some((p) => model.panes[p]?.unread));
  const running = ws.surfaces.some((s) => panesOf(s.root).some((p) => model.panes[p]?.running));
  const allPanes = ws.surfaces.flatMap((s) => panesOf(s.root)).map((p) => model.panes[p]).filter(Boolean);
  const git = focused?.git ?? allPanes.find((p) => p.git)?.git;
  const ports = [...new Set(allPanes.flatMap((p) => p.ports ?? []))].sort((a, b) => a - b);

  return (
    <div
      className={`ws-item ${active ? "active" : ""} ${dragging ? "dragging" : ""} ${anyUnreadPane ? "attention" : ""}`}
      style={{ borderLeftColor: ws.color ?? "transparent" }}
      role="button"
      tabIndex={0}
      aria-current={active ? "true" : undefined}
      aria-label={`Workspace ${index + 1}: ${ws.name}${unread ? `, ${unread} unread` : ""}${running ? ", running" : ""}`}
      onKeyDown={(e) => {
        if (!editing && (e.key === "Enter" || e.key === " ") && e.target === e.currentTarget) {
          e.preventDefault();
          api.activateWorkspace(ws.id);
        }
      }}
      draggable={!editing}
      onDragStart={onDragStart}
      onDragOver={(e) => e.preventDefault()}
      onDrop={() => onDropAt(index)}
      onClick={() => api.activateWorkspace(ws.id)}
      onDoubleClick={() => setEditing(true)}
      onContextMenu={(e) => {
        e.preventDefault();
        setCtx(true);
      }}
      onMouseLeave={() => setCtx(false)}
    >
      <div className="ws-row">
        <span className="ws-index">{index < 8 ? index + 1 : ""}</span>
        {editing ? (
          <input
            autoFocus
            className="ws-rename"
            defaultValue={ws.name}
            onClick={(e) => e.stopPropagation()}
            onBlur={(e) => {
              api.updateWorkspace(ws.id, { name: e.currentTarget.value });
              setEditing(false);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") setEditing(false);
            }}
          />
        ) : (
          <span className="ws-name">{ws.name}</span>
        )}
        {running && <span className="ws-running" title="Command running" />}
        {unread > 0 && <span className="badge">{unread}</span>}
      </div>
      {ws.description && <div className="ws-desc">{ws.description}</div>}
      <div className="ws-meta">
        {focused?.cwd ? shortPath(focused.cwd) : paneLabel(focused)}
        {ws.surfaces.length > 1 && <span className="ws-tabs"> · {ws.surfaces.length} tabs</span>}
      </div>
      {(git || ports.length > 0 || ws.remote) && (
        <div className="ws-chips">
          {ws.remote && (
            <span className="chip remote" title={`SSH: ${ws.remote.host}${ws.remote.port ? `:${ws.remote.port}` : ""}\nBrowser panes here go through an ssh tunnel`}>
              ⇄ {ws.remote.host.split("@").pop()}
            </span>
          )}
          {git && (
            <span className="chip branch" title={git.root}>
              <BranchIcon />
              {git.branch}
              {git.dirty && <span className="dirty" title="Uncommitted changes">●</span>}
            </span>
          )}
          {git?.pr && (
            <span
              className={`chip pr ${git.pr.state.toLowerCase()} ${git.pr.draft ? "draft" : ""}`}
              title={`${git.pr.title}\nChecks: ${git.pr.checks}`}
              onClick={(e) => {
                e.stopPropagation();
                openUrl(git.pr!.url);
              }}
            >
              <span className={`checks ${git.pr.checks}`} />#{git.pr.number}
            </span>
          )}
          {ports.map((port) => (
            <span
              key={port}
              className="chip port"
              title={`Open http://localhost:${port}`}
              onClick={(e) => {
                e.stopPropagation();
                openPort(port);
              }}
            >
              :{port}
            </span>
          ))}
        </div>
      )}
      {latest && <div className={`ws-notif ${latest.read ? "" : "unread"}`}>{latest.title ? `${latest.title}: ` : ""}{latest.body}</div>}
      {ctx && (
        <div className="menu ctx" onClick={(e) => e.stopPropagation()}>
          <div className="menu-item" onClick={() => { setCtx(false); setEditing(true); }}>Rename</div>
          <div
            className="menu-item"
            onClick={() => {
              setCtx(false);
              const d = window.prompt("Workspace description", ws.description ?? "");
              if (d !== null) api.updateWorkspace(ws.id, { description: d });
            }}
          >
            Edit description
          </div>
          <div className="menu-colors">
            {COLORS.map((c) => (
              <span key={c || "none"} className="swatch" style={{ background: c || "transparent" }} onClick={() => api.updateWorkspace(ws.id, { color: c })} />
            ))}
          </div>
          <div className="menu-item danger" onClick={() => api.closeWorkspace(ws.id)}>Close workspace</div>
        </div>
      )}
    </div>
  );
}

function BranchIcon() {
  return (
    <svg width="10" height="10" viewBox="0 0 16 16" aria-hidden="true">
      <path
        fill="currentColor"
        d="M5 3.25a.75.75 0 1 1-1.5 0 .75.75 0 0 1 1.5 0Zm0 2.122a2.25 2.25 0 1 0-1.5 0v5.256a2.25 2.25 0 1 0 1.5 0V9.5h3.25A2.75 2.75 0 0 0 11 6.75v-.378a2.25 2.25 0 1 0-1.5 0v.378c0 .69-.56 1.25-1.25 1.25H5V5.372ZM4.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm6-8.75a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z"
      />
    </svg>
  );
}
