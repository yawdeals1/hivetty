import { lazy, Suspense, useEffect, useState } from "react";

import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api } from "./lib/api";
import { activeWorkspace, focusedPane, panesOf, useStore } from "./state/store";
import { actionFor, setBindings } from "./lib/keymap";
import { commandActionId, refreshCommands, runAction } from "./lib/actions";
import { applySettings, disposeMissing, flushScrollback, pickTheme, refitAll, settingsFromConfig } from "./lib/terminals";
import { Sidebar } from "./components/Sidebar";
import { TabBar } from "./components/TabBar";
import { SurfaceView } from "./components/PaneTree";
import { NotificationPanel } from "./components/NotificationPanel";
import { CommandPalette, FindBar, RenameDialog } from "./components/Overlays";
import { UpdateBanner } from "./components/UpdateBanner";
import type { ConfigState, Id, Info, Model, PaneStatus, UpdateStatus } from "./types";
// Rarely opened: loaded on first use to keep startup lean.
const Settings = lazy(() => import("./components/Settings").then((m) => ({ default: m.Settings })));
const FindInFiles = lazy(() => import("./components/FindInFiles").then((m) => ({ default: m.FindInFiles })));

/** ms after navigation start when the app bundle began executing (startup timing) */
const SCRIPT_START = performance.now();
/** ms after navigation start when the main bundle finished downloading */
function bundleLoadedAt() {
  const r = performance.getEntriesByType("resource").find((e) => /\/assets\/index-.*\.js/.test(e.name)) as PerformanceResourceTiming | undefined;
  return r ? r.responseEnd : -1;
}

const statusTimers = new Map<Id, ReturnType<typeof setTimeout>>();
function showPaneStatus(pane: Id, s: PaneStatus) {
  const { set, paneStatus } = useStore.getState();
  set({ paneStatus: { ...paneStatus, [pane]: s } });
  clearTimeout(statusTimers.get(pane));
  if (s.kind !== "busy") {
    statusTimers.set(
      pane,
      setTimeout(() => {
        const cur = { ...useStore.getState().paneStatus };
        delete cur[pane];
        useStore.getState().set({ paneStatus: cur });
      }, s.kind === "error" ? 8000 : 3000),
    );
  }
}

const SCROLLBACK_FLUSH_MS = 15_000;

function applyConfig(cs: ConfigState) {
  const s = useStore.getState();
  s.setConfig(cs);
  applySettings(settingsFromConfig(cs.config, cs.builtin_themes));
  const extra: Record<string, string> = {};
  for (const c of cs.config.commands) if (c.keybinding) extra[c.keybinding] = commandActionId({ ...c, source: null });
  setBindings(cs.config.keybindings, extra);
  const t = pickTheme(cs.config, cs.builtin_themes);
  if (t) document.documentElement.style.setProperty("--term-bg", t.background);
  void refreshCommands();
}

export default function App() {
  const model = useStore((s) => s.model);
  const sidebarOpen = useStore((s) => s.sidebarOpen);
  const notificationsOpen = useStore((s) => s.notificationsOpen);
  const overlay = useStore((s) => s.overlay);
  const configError = useStore((s) => s.config?.error ?? null);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const ws = activeWorkspace(model);
  const surface = ws?.surfaces.find((s) => s.id === ws.active_surface);
  const focusedCwd = focusedPane(model)?.cwd ?? null;

  useEffect(() => {
    const { setModel, setInfo } = useStore.getState();
    api.getConfig().then(applyConfig);
    api.getInfo().then(setInfo);
    api.getState().then((m) => {
      setModel(m);
      // startup timing: report once the first real frame is painted
      requestAnimationFrame(() => requestAnimationFrame(() => void api.uiReady(SCRIPT_START, performance.now(), bundleLoadedAt())));
    });
    const win = getCurrentWindow();
    const un = [
      listen<Model>("state", (e) => setModel(e.payload)),
      listen<ConfigState>("config", (e) => applyConfig(e.payload)),
      listen<Info>("info", (e) => setInfo(e.payload)),
      listen<UpdateStatus>("update-status", (e) => useStore.getState().set({ update: e.payload })),
      listen<{ pane: Id } & PaneStatus>("pane-status", (e) => showPaneStatus(e.payload.pane, { text: e.payload.text, kind: e.payload.kind })),
      // Files dropped on a terminal: paste their paths (ssh panes upload them first).
      getCurrentWebview().onDragDropEvent((e) => {
        if (e.payload.type !== "drop" || !e.payload.paths.length) return;
        const dpr = window.devicePixelRatio || 1;
        const el = document.elementFromPoint(e.payload.position.x / dpr, e.payload.position.y / dpr);
        const pane = (el?.closest("[data-pane-id]") as HTMLElement | null)?.dataset.paneId ?? focusedPane(useStore.getState().model)?.id;
        if (pane) api.paneDropFiles(pane, e.payload.paths).catch((err) => showPaneStatus(pane, { text: String(err), kind: "error" }));
      }),
      win.onFocusChanged(({ payload }) => {
        api.setAppFocused(payload);
        if (!payload) void flushScrollback();
      }),
      // On close the backend asks for the latest scrollback before it writes the session.
      listen("session-flush", async () => {
        await flushScrollback(true);
        await api.sessionFlushed();
      }),
    ];
    const timer = setInterval(() => void flushScrollback(), SCROLLBACK_FLUSH_MS);
    // Moving to a monitor with another scale changes cell sizes without resizing any element.
    let dprQuery: MediaQueryList | null = null;
    const onDpr = () => {
      refitAll();
      watchDpr();
    };
    const watchDpr = () => {
      dprQuery?.removeEventListener("change", onDpr);
      dprQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      dprQuery.addEventListener("change", onDpr);
    };
    watchDpr();
    const onKey = (ev: KeyboardEvent) => {
      const a = actionFor(ev);
      if (!a) return;
      ev.preventDefault();
      ev.stopPropagation();
      runAction(a);
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      clearInterval(timer);
      dprQuery?.removeEventListener("change", onDpr);
      window.removeEventListener("keydown", onKey, true);
      un.forEach((p) => p.then((f) => f()));
    };
  }, []);

  // Project commands depend on where the focused pane is.
  useEffect(() => {
    void refreshCommands();
  }, [focusedCwd]);

  // Dispose terminals for panes that no longer exist.
  useEffect(() => {
    const live = new Set(model.workspaces.flatMap((w) => w.surfaces.flatMap((s) => panesOf(s.root))));
    disposeMissing(live);
  }, [model]);

  // Window title + taskbar attention
  useEffect(() => {
    const unread = model.notifications.filter((n) => !n.read).length;
    const t = ws ? `${unread ? `(${unread}) ` : ""}${ws.name} — Hivetty` : "Hivetty";
    if (document.title !== t) {
      document.title = t;
      getCurrentWindow().setTitle(t).catch(() => {});
    }
  }, [model, ws]);

  return (
    <div className={`app ${sidebarOpen ? "" : "no-sidebar"} ${notificationsOpen ? "with-notifs" : ""}`}>
      {sidebarOpen && <Sidebar />}
      <main className="main">
        {configError && configError !== dismissed && (
          <div className="config-banner" role="alert">
            <span>
              <b>hivetty.json has an error</b> — previous settings kept. {configError}
            </span>
            <button onClick={() => api.configOpen()}>Open file</button>
            <button onClick={() => setDismissed(configError)}>Dismiss</button>
          </div>
        )}
        <UpdateBanner />
        {ws && <TabBar ws={ws} />}
        <div className="surface-host">
          {surface ? (
            <SurfaceView key={surface.id} surface={surface} />
          ) : (
            <div className="empty">
              <p>No workspaces open.</p>
              <button onClick={() => api.newWorkspace()}>New workspace (Ctrl+Shift+N)</button>
            </div>
          )}
          {overlay === "find" && <FindBar />}
        </div>
      </main>
      {notificationsOpen && <NotificationPanel />}
      {overlay === "palette" && <CommandPalette />}
      {overlay === "rename" && <RenameDialog />}
      <Suspense fallback={null}>
        {overlay === "settings" && <Settings />}
        {overlay === "findFiles" && <FindInFiles />}
      </Suspense>
      <Announcer />
    </div>
  );
}

/** Screen-reader announcement of the newest notification (visually hidden). */
function Announcer() {
  const latest = useStore((s) => {
    const n = s.model.notifications[s.model.notifications.length - 1];
    return n && !n.read ? n : undefined;
  });
  const wsName = useStore((s) => s.model.workspaces.find((w) => w.id === latest?.workspace)?.name);
  return (
    <div className="sr-only" role="status" aria-live="polite">
      {latest ? `${wsName ?? "Workspace"}: ${latest.title ? `${latest.title}. ` : ""}${latest.body}` : ""}
    </div>
  );
}
