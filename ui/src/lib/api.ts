import { Channel, invoke } from "@tauri-apps/api/core";
import type { BrowserProfile, ConfigState, CustomCommand, Dir, FindMessage, FindOpts, HostStatus, Id, ImportReport, Info, Model, Theme, UpdateStatus } from "../types";

export const api = {
  getState: () => invoke<Model>("get_state"),
  getInfo: () => invoke<Info>("get_info"),
  newWorkspace: (opts: { name?: string; profile?: string; cwd?: string } = {}) => invoke<[Id, Id]>("new_workspace", opts),
  newSurface: (opts: { workspace?: Id; profile?: string } = {}) => invoke<Id>("new_surface", opts),
  split: (dir: Dir, opts: { pane?: Id; profile?: string } = {}) => invoke<Id>("split", { dir, ...opts }),
  closePane: (pane: Id) => invoke("close_pane", { pane }),
  closeSurface: (workspace: Id, surface: Id) => invoke("close_surface", { workspace, surface }),
  closeWorkspace: (workspace: Id) => invoke("close_workspace", { workspace }),
  focusPane: (pane: Id) => invoke("focus_pane", { pane }),
  focusDir: (dir: Dir) => invoke("focus_dir", { dir }),
  resizePane: (dir: Dir, delta = 0.05) => invoke("resize_pane", { dir, delta }),
  setRatio: (surface: Id, path: number[], ratio: number) => invoke("set_ratio", { surface, path, ratio }),
  equalize: () => invoke("equalize"),
  toggleZoom: () => invoke("toggle_zoom"),
  activateWorkspace: (workspace: Id) => invoke("activate_workspace", { workspace }),
  activateWorkspaceIndex: (index: number) => invoke("activate_workspace_index", { index }),
  activateSurface: (workspace: Id, surface: Id) => invoke("activate_surface", { workspace, surface }),
  cycleWorkspace: (step: number) => invoke("cycle_workspace", { step }),
  cycleSurface: (step: number) => invoke("cycle_surface", { step }),
  moveWorkspace: (workspace: Id, to: number) => invoke("move_workspace", { workspace, to }),
  updateWorkspace: (workspace: Id, patch: { name?: string; description?: string; color?: string }) =>
    invoke("update_workspace", { workspace, ...patch }),
  renameSurface: (surface: Id, title: string) => invoke("rename_surface", { surface, title }),
  markAllRead: () => invoke("mark_all_read"),
  clearNotifications: () => invoke("clear_notifications"),
  jumpLatestUnread: () => invoke<Id | null>("jump_latest_unread"),
  setAppFocused: (focused: boolean) => invoke("set_app_focused", { focused }),
  ptyAttach: (pane: Id, onData: (d: Uint8Array) => void) => {
    const ch = new Channel<ArrayBuffer>();
    ch.onmessage = (buf) => onData(new Uint8Array(buf));
    return invoke<boolean>("pty_attach", { pane, channel: ch });
  },
  ptyWrite: (pane: Id, data: string) => invoke("pty_write", { pane, data }),
  ptyResize: (pane: Id, cols: number, rows: number) => invoke("pty_resize", { pane, cols, rows }),
  restartPane: (pane: Id) => invoke("restart_pane", { pane }),
  browserSetBounds: (pane: Id, x: number, y: number, w: number, h: number, visible: boolean) =>
    invoke("browser_set_bounds", { pane, x, y, w, h, visible }),
  browserNavigate: (pane: Id, url: string) => invoke<string>("browser_navigate", { pane, url }),
  browserHistory: (pane: Id, action: "back" | "forward" | "reload") => invoke("browser_history", { pane, action }),
  browserDevtools: (pane: Id) => invoke("browser_devtools", { pane }),
  browserOpen: (url: string, near?: Id) => invoke<Id>("browser_open", { url, near }),
  browserFocus: (pane: Id) => invoke("browser_focus", { pane }),
  getConfig: () => invoke<ConfigState>("get_config"),
  /** Set a setting by key path; `undefined` removes it so the default applies. */
  configSet: (path: string[], value: unknown) =>
    invoke<ConfigState>("config_set", value === undefined ? { path, remove: true } : { path, value }),
  configOpen: () => invoke("config_open"),
  configImportWt: (opts: { path?: string; text?: string } = {}) => invoke<Theme[]>("config_import_wt", opts),
  sessionPutScrollback: (pane: Id, data: string) => invoke("session_put_scrollback", { pane, data }),
  sessionTakeRestored: (pane: Id) => invoke<string | null>("session_take_restored", { pane }),
  sessionSave: () => invoke<string>("session_save"),
  sessionFlushed: () => invoke("session_flushed"),
  resumeAgent: (pane: Id) => invoke("pane_resume_agent", { pane }),
  dismissResume: (pane: Id) => invoke("pane_dismiss_resume", { pane }),
  findStart: (opts: FindOpts, onMessage: (m: FindMessage) => void) => {
    const ch = new Channel<FindMessage>();
    ch.onmessage = onMessage;
    return invoke<number>("find_start", { opts, channel: ch });
  },
  findCancel: () => invoke("find_cancel"),
  openFileAt: (path: string, line?: number, col?: number, near?: Id) => invoke("open_file_at", { path, line, col, near }),
  projectCommands: (pane?: Id) => invoke<{ commands: CustomCommand[]; warnings: string[] }>("project_commands", { pane }),
  runCommand: (command: CustomCommand, pane?: Id) => invoke<Id>("run_command", { command, pane }),
  browserProfiles: () => invoke<BrowserProfile[]>("browser_profiles"),
  browserImport: (pane: Id, profile: string, opts: { cookies?: boolean; history?: boolean; domains?: string[] } = {}) =>
    invoke<ImportReport>("browser_import", { pane, profile, cookies: opts.cookies ?? true, history: opts.history ?? true, domains: opts.domains ?? [] }),
  newSshWorkspace: (host: string, command?: string) => invoke<[Id, Id]>("new_ssh_workspace", { host, command }),
  paneDropFiles: (pane: Id, paths: string[]) => invoke("pane_drop_files", { pane, paths }),
  hostStatus: () => invoke<HostStatus>("host_status"),
  hostStop: () => invoke("host_stop"),
  uiReady: (scriptMs: number, frameMs: number, bundleMs: number) => invoke("ui_ready", { scriptMs, frameMs, bundleMs }),
  debugLog: (msg: string) => invoke("debug_log", { msg }),
  updateStatus: () => invoke<UpdateStatus>("update_status"),
  updateCheck: () => invoke<UpdateStatus>("update_check"),
  updateInstall: (onProgress: (p: { downloaded: number; total: number | null }) => void) => {
    const ch = new Channel<{ downloaded: number; total: number | null }>();
    ch.onmessage = onProgress;
    return invoke("update_install", { progress: ch });
  },
  browserSuggest: (query: string) => invoke<{ url: string; title: string }[]>("browser_suggest", { query }),
};
