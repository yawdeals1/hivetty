import { api } from "../lib/api";
import { panesOf, surfaceTitle, useStore } from "../state/store";
import type { Workspace } from "../types";

export function TabBar({ ws }: { ws: Workspace }) {
  const model = useStore((s) => s.model);
  return (
    <div className="tabbar" role="tablist" aria-label="Tabs">
      {ws.surfaces.map((s, i) => {
        const unread = panesOf(s.root).some((p) => model.panes[p]?.unread);
        return (
          <div
            key={s.id}
            className={`tab ${s.id === ws.active_surface ? "active" : ""} ${unread ? "unread" : ""}`}
            role="tab"
            tabIndex={s.id === ws.active_surface ? 0 : -1}
            aria-selected={s.id === ws.active_surface}
            aria-label={`${surfaceTitle(model, s)}${unread ? ", needs attention" : ""}`}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                api.activateSurface(ws.id, s.id);
              }
            }}
            onClick={() => api.activateSurface(ws.id, s.id)}
            onAuxClick={(e) => e.button === 1 && api.closeSurface(ws.id, s.id)}
            onDoubleClick={() => {
              const t = window.prompt("Tab title", s.title ?? "");
              if (t !== null) api.renameSurface(s.id, t);
            }}
            title={surfaceTitle(model, s)}
          >
            {i < 8 && <span className="tab-index">{i + 1}</span>}
            <span className="tab-title">{surfaceTitle(model, s)}</span>
            {unread && <span className="dot" />}
            <button
              className="tab-close"
              tabIndex={-1}
              aria-label="Close tab"
              onClick={(e) => {
                e.stopPropagation();
                api.closeSurface(ws.id, s.id);
              }}
            >
              ×
            </button>
          </div>
        );
      })}
      <button className="tab-new" title="New tab (Ctrl+Shift+T)" aria-label="New tab" onClick={() => api.newSurface({ workspace: ws.id })}>
        +
      </button>
    </div>
  );
}
