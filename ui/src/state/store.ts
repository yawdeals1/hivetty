import { create } from "zustand";
import type { ConfigState, CustomCommand, Id, Info, Model, Pane, PaneStatus, Surface, UpdateStatus, Workspace } from "../types";

export type Overlay = null | "palette" | "rename" | "find" | "settings" | "findFiles";

interface UiState {
  model: Model;
  info: Info | null;
  config: ConfigState | null;
  /** user + project custom commands for the focused pane (refreshed when the palette opens) */
  commands: CustomCommand[];
  sidebarOpen: boolean;
  notificationsOpen: boolean;
  overlay: Overlay;
  fontSize: number;
  flashPane: Id | null;
  /** A dropdown/context menu is open (native browser webviews must hide). */
  menuOpen: boolean;
  /** transient per-pane status lines (uploads) */
  paneStatus: Record<Id, PaneStatus>;
  update: UpdateStatus | null;
  /** download progress 0..1 while an update downloads */
  updateProgress: number | null;
  setModel: (m: Model) => void;
  setInfo: (i: Info) => void;
  setConfig: (c: ConfigState) => void;
  set: (p: Partial<UiState>) => void;
}

export const useStore = create<UiState>((set) => ({
  model: { workspaces: [], active_workspace: null, panes: {}, notifications: [] },
  info: null,
  config: null,
  commands: [],
  sidebarOpen: true,
  notificationsOpen: false,
  overlay: null,
  fontSize: 14,
  flashPane: null,
  menuOpen: false,
  paneStatus: {},
  update: null,
  updateProgress: null,
  setModel: (model) => set({ model }),
  setInfo: (info) => set({ info }),
  setConfig: (config) => set({ config, fontSize: config.config.font.size }),
  set: (p) => set(p),
}));

export function activeWorkspace(m: Model): Workspace | undefined {
  return m.workspaces.find((w) => w.id === m.active_workspace);
}

export function activeSurface(m: Model): Surface | undefined {
  const w = activeWorkspace(m);
  return w?.surfaces.find((s) => s.id === w.active_surface);
}

export function focusedPane(m: Model): Pane | undefined {
  const s = activeSurface(m);
  return s ? m.panes[s.focused] : undefined;
}

export function panesOf(node: Surface["root"]): Id[] {
  return node.type === "leaf" ? [node.pane] : [...panesOf(node.a), ...panesOf(node.b)];
}

export function workspaceUnread(m: Model, ws: Id): number {
  return m.notifications.filter((n) => n.workspace === ws && !n.read).length;
}

export function surfaceTitle(m: Model, s: Surface): string {
  if (s.title) return s.title;
  const p = m.panes[s.focused];
  return paneLabel(p) ?? "Terminal";
}

export function paneLabel(p?: Pane): string | undefined {
  if (!p) return undefined;
  if (p.title) return p.title;
  if (p.kind.type === "browser") return p.kind.url;
  if (p.cwd) return basename(p.cwd);
  return p.kind.profile;
}

export function basename(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

export function shortPath(path: string): string {
  return path.replace(/^C:\\Users\\[^\\]+/i, "~");
}
