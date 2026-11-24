// Mirrors hivetty-core model types (serde JSON).
export type Id = string;
export type Axis = "row" | "column";
export type Dir = "left" | "right" | "up" | "down";

export type LayoutNode =
  | { type: "leaf"; pane: Id }
  | { type: "split"; axis: Axis; ratio: number; a: LayoutNode; b: LayoutNode };

export type PaneKind =
  | { type: "terminal"; profile: string; cwd?: string; command?: string }
  | { type: "browser"; url: string };

export interface Pane {
  id: Id;
  kind: PaneKind;
  title: string | null;
  cwd: string | null;
  /** cwd as a Windows path (WSL translated; absent for ssh panes) */
  local_cwd?: string;
  unread: boolean;
  running: boolean;
  last_exit: number | null;
  exited: boolean;
  agent?: { agent: string; session_id: string };
  git?: GitInfo;
  ports?: number[];
  /** Restored with a recorded agent session that can be resumed. */
  resumable?: boolean;
}

export interface PrInfo {
  number: number;
  title: string;
  url: string;
  state: "OPEN" | "CLOSED" | "MERGED" | string;
  draft: boolean;
  checks: "passing" | "failing" | "pending" | "none" | string;
}

export interface GitInfo {
  root: string;
  branch: string;
  detached: boolean;
  dirty: boolean;
  pr?: PrInfo;
}

export interface Surface {
  id: Id;
  title: string | null;
  root: LayoutNode;
  focused: Id;
  zoomed: Id | null;
}

export interface Workspace {
  id: Id;
  name: string;
  description: string | null;
  color: string | null;
  surfaces: Surface[];
  active_surface: Id;
  /** set for workspaces opened with `hivetty ssh` */
  remote?: Remote;
}

export interface Remote {
  host: string;
  port?: number;
  identity?: string;
  options?: string[];
}

export interface HostStatus {
  running: boolean;
  sessions: number;
  live: number;
  exe: string | null;
}

export interface PaneStatus {
  text: string;
  kind: "busy" | "done" | "error";
}

export interface Notification {
  id: Id;
  pane: Id;
  workspace: Id;
  title: string | null;
  body: string;
  ts: number;
  read: boolean;
}

export interface Model {
  workspaces: Workspace[];
  active_workspace: Id | null;
  panes: Record<Id, Pane>;
  notifications: Notification[];
}

export interface Profile {
  id: string;
  name: string;
  command: string;
  args: string[];
  shell: string;
}

export interface Info {
  profiles: Profile[];
  default_profile: string;
  pipe: string;
  version: string;
  debug_perf: boolean;
}

export interface Theme {
  name: string;
  background: string;
  foreground: string;
  cursor?: string;
  selection_background?: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  bright_black: string;
  bright_red: string;
  bright_green: string;
  bright_yellow: string;
  bright_blue: string;
  bright_magenta: string;
  bright_cyan: string;
  bright_white: string;
}

export type CommandTarget = "current" | "split-right" | "split-down" | "tab" | "workspace";

export interface CustomCommand {
  name: string;
  command: string;
  target: CommandTarget;
  cwd?: string | null;
  profile?: string | null;
  description?: string | null;
  keybinding?: string | null;
  /** project file it came from; null = user settings */
  source?: string | null;
}

export interface Config {
  font: { family: string; size: number; line_height: number };
  cursor: { style: "bar" | "block" | "underline"; blink: boolean };
  theme: string;
  themes: Theme[];
  terminal: { scrollback: number; copy_on_select: boolean; confirm_multiline_paste: boolean; bell: "attention" | "sound" | "none"; tmux_shim: boolean; agent_hooks: boolean; wsl_shell_integration: boolean; screen_reader: boolean };
  default_profile?: string;
  profiles: { id: string; name: string; command: string; args: string[]; shell?: string; cwd?: string; env?: Record<string, string> }[];
  hidden_profiles: string[];
  keybindings: Record<string, string | null>;
  session: { restore: boolean; scrollback: boolean; scrollback_lines: number; resume_agents: boolean; persistent: boolean };
  editor?: { command: string; terminal: boolean };
  commands: CustomCommand[];
  updates: { channel: "stable" | "nightly"; check: boolean; endpoint?: string };
}

export interface UpdateStatus {
  state: "unconfigured" | "idle" | "checking" | "none" | "available" | "downloading" | "installing" | "error";
  channel: string;
  current: string;
  version: string | null;
  notes: string | null;
  date: string | null;
  error: string | null;
}

export interface ConfigState {
  config: Config;
  warnings: string[];
  error: string | null;
  path: string;
  builtin_themes: Theme[];
}

export interface BrowserProfile {
  id: string;
  browser: string;
  name: string;
}

export interface ImportReport {
  profile: string;
  browser: string;
  history?: { imported?: number; error?: string };
  cookies?: { imported?: number; app_bound?: number; failed?: number; applied?: boolean; error?: string };
}

export interface FindOpts {
  root: string;
  query: string;
  regex: boolean;
  case_sensitive: boolean;
  whole_word: boolean;
  include: string;
  all_files: boolean;
}

export interface FindHit {
  line: number;
  col: number;
  len: number;
  text: string;
}

export interface FileHits {
  path: string;
  rel: string;
  hits: FindHit[];
}

export type FindMessage =
  | { id: number; files: FileHits[]; done?: undefined }
  | { id: number; done: true; files_searched?: number; matches?: number; truncated?: boolean; error?: string };
