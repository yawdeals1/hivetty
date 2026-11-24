import { api } from "./api";
import { activeSurface, activeWorkspace, focusedPane, useStore } from "../state/store";
import { clearScrollback, setFontSize } from "./terminals";
import type { CustomCommand, Dir } from "../types";

export interface Action {
  id: string;
  title: string;
  /** secondary text in the palette */
  detail?: string;
  run: () => void | Promise<unknown>;
}

const st = () => useStore.getState();

let fontSave: ReturnType<typeof setTimeout> | undefined;
function font(delta: number | null) {
  const size = delta === null ? 14 : Math.min(32, Math.max(8, st().fontSize + delta));
  st().set({ fontSize: size });
  setFontSize(size);
  // Persist to hivetty.json, debounced so holding Ctrl+= doesn't rewrite the file per step.
  clearTimeout(fontSave);
  fontSave = setTimeout(() => void api.configSet(["font", "size"], delta === null ? undefined : size).catch(() => {}), 400);
}

export const commandActionId = (c: CustomCommand) => `command.${c.source ?? "user"}.${c.name}`;

const targetLabel: Record<string, string> = {
  current: "in this pane",
  "split-right": "split right",
  "split-down": "split down",
  tab: "new tab",
  workspace: "new workspace",
};

export function runCustomCommand(c: CustomCommand) {
  const pane = focusedPane(st().model)?.id;
  return api.runCommand(c, pane);
}

/** Re-read user + project commands for the focused pane. */
export async function refreshCommands() {
  const pane = focusedPane(st().model)?.id;
  try {
    const l = await api.projectCommands(pane);
    st().set({ commands: l.commands });
  } catch {
    /* keep previous */
  }
}

function flash() {
  const p = focusedPane(st().model);
  if (!p) return;
  st().set({ flashPane: p.id });
  setTimeout(() => st().set({ flashPane: null }), 600);
}

const dirs: [Dir, string][] = [
  ["left", "Left"],
  ["right", "Right"],
  ["up", "Up"],
  ["down", "Down"],
];

export function buildActions(): Action[] {
  const list: Action[] = [
    { id: "workspace.new", title: "New Workspace", run: () => api.newWorkspace() },
    {
      id: "workspace.ssh",
      title: "New SSH Workspace…",
      detail: "ssh user@host",
      run: () => {
        const host = window.prompt("SSH destination (user@host, host:port or an ~/.ssh/config alias)", "");
        if (host && host.trim()) return api.newSshWorkspace(host.trim()).catch((e) => window.alert(String(e)));
      },
    },
    { id: "surface.new", title: "New Tab", run: () => api.newSurface() },
    { id: "pane.close", title: "Close Pane", run: () => { const p = focusedPane(st().model); if (p) return api.closePane(p.id); } },
    { id: "pane.splitRight", title: "Split Right", run: () => api.split("right") },
    { id: "pane.splitDown", title: "Split Down", run: () => api.split("down") },
    { id: "pane.zoom", title: "Toggle Pane Zoom", run: () => api.toggleZoom() },
    { id: "pane.flash", title: "Flash Focused Pane", run: flash },
    { id: "pane.equalize", title: "Equalize Splits", run: () => api.equalize() },
    ...dirs.map(([d, n]) => ({ id: `pane.focus${n}`, title: `Focus Pane ${n}`, run: () => api.focusDir(d) })),
    ...dirs.map(([d, n]) => ({ id: `pane.resize${n}`, title: `Resize Pane ${n}`, run: () => api.resizePane(d) })),
    { id: "surface.next", title: "Next Tab", run: () => api.cycleSurface(1) },
    { id: "surface.prev", title: "Previous Tab", run: () => api.cycleSurface(-1) },
    {
      id: "surface.close",
      title: "Close Tab",
      run: () => {
        const w = activeWorkspace(st().model);
        const s = activeSurface(st().model);
        if (w && s) return api.closeSurface(w.id, s.id);
      },
    },
    { id: "workspace.next", title: "Next Workspace", run: () => api.cycleWorkspace(1) },
    { id: "workspace.prev", title: "Previous Workspace", run: () => api.cycleWorkspace(-1) },
    { id: "workspace.rename", title: "Rename Workspace", run: () => st().set({ overlay: "rename" }) },
    {
      id: "workspace.close",
      title: "Close Workspace",
      run: () => {
        const w = activeWorkspace(st().model);
        if (w) return api.closeWorkspace(w.id);
      },
    },
    { id: "workspace.gotoLast", title: "Go to Last Workspace", run: () => api.activateWorkspaceIndex(-1) },
    { id: "surface.gotoLast", title: "Go to Last Tab", run: () => gotoSurface(-1) },
    { id: "palette.open", title: "Command Palette", run: () => st().set({ overlay: "palette" }) },
    { id: "notifications.toggle", title: "Toggle Notifications Panel", run: () => st().set({ notificationsOpen: !st().notificationsOpen }) },
    { id: "notifications.jumpUnread", title: "Jump to Latest Unread", run: () => api.jumpLatestUnread() },
    { id: "notifications.markAllRead", title: "Mark All Notifications Read", run: () => api.markAllRead() },
    { id: "notifications.clear", title: "Clear All Notifications", run: () => api.clearNotifications() },
    { id: "sidebar.toggle", title: "Toggle Sidebar", run: () => st().set({ sidebarOpen: !st().sidebarOpen }) },
    { id: "find.open", title: "Find in Terminal", run: () => st().set({ overlay: "find" }) },
    { id: "findFiles.open", title: "Find in Files", run: () => st().set({ overlay: "findFiles" }) },
    { id: "settings.open", title: "Settings", run: () => st().set({ overlay: "settings" }) },
    { id: "settings.openFile", title: "Open Settings File (hivetty.json)", run: () => api.configOpen() },
    { id: "session.save", title: "Save Session Now", run: () => api.sessionSave() },
    {
      id: "agent.resume",
      title: "Resume Agent Session in This Pane",
      run: () => {
        const p = focusedPane(st().model);
        if (p?.agent) return api.resumeAgent(p.id);
      },
    },
    { id: "terminal.clear", title: "Clear Scrollback", run: () => { const p = focusedPane(st().model); if (p) clearScrollback(p.id); } },
    { id: "font.increase", title: "Increase Font Size", run: () => font(1) },
    { id: "font.decrease", title: "Decrease Font Size", run: () => font(-1) },
    { id: "font.reset", title: "Reset Font Size", run: () => font(null) },
  ];
  for (let i = 1; i <= 8; i++) {
    list.push({ id: `workspace.goto${i}`, title: `Go to Workspace ${i}`, run: () => api.activateWorkspaceIndex(i - 1) });
    list.push({ id: `surface.goto${i}`, title: `Go to Tab ${i}`, run: () => gotoSurface(i - 1) });
  }
  for (const c of st().commands) {
    list.push({
      id: commandActionId(c),
      title: `Run: ${c.name}`,
      detail: `${c.command} · ${targetLabel[c.target] ?? c.target}`,
      run: () => runCustomCommand(c),
    });
  }
  for (const p of st().info?.profiles ?? []) {
    list.push({ id: `profile.workspace.${p.id}`, title: `New Workspace: ${p.name}`, run: () => api.newWorkspace({ profile: p.id }) });
    list.push({ id: `profile.tab.${p.id}`, title: `New Tab: ${p.name}`, run: () => api.newSurface({ profile: p.id }) });
    list.push({ id: `profile.split.${p.id}`, title: `Split Right: ${p.name}`, run: () => api.split("right", { profile: p.id }) });
  }
  return list;
}

function gotoSurface(index: number) {
  const w = activeWorkspace(st().model);
  if (!w || w.surfaces.length === 0) return;
  const s = index < 0 ? w.surfaces[w.surfaces.length - 1] : w.surfaces[Math.min(index, w.surfaces.length - 1)];
  return api.activateSurface(w.id, s.id);
}

export function runAction(id: string) {
  const a = buildActions().find((x) => x.id === id);
  if (a) void a.run();
}

/** Simple subsequence fuzzy score; higher is better, -1 = no match. */
export function fuzzyScore(query: string, text: string): number {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  if (!q) return 0;
  let ti = 0;
  let score = 0;
  let streak = 0;
  for (const ch of q) {
    const idx = t.indexOf(ch, ti);
    if (idx < 0) return -1;
    streak = idx === ti ? streak + 1 : 0;
    score += 1 + streak * 2 + (idx === 0 || t[idx - 1] === " " ? 3 : 0);
    ti = idx + 1;
  }
  return score - t.length * 0.01;
}
