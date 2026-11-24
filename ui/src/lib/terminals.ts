// Persistent xterm.js instances keyed by pane id. Views borrow the DOM element; switching
// workspaces/tabs never disposes a terminal, so scrollback and render state survive.
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SerializeAddon } from "@xterm/addon-serialize";
import { openUrl } from "./open";
import { pickTheme, restoreTrailer, sanitizeScrollback, themeToXterm } from "./termutil";
export { pickTheme, restoreTrailer, themeToXterm };
import { api } from "./api";
import { isAppShortcut } from "./keymap";
import { useStore } from "../state/store";
import type { Config, Id, Theme } from "../types";

export interface TermSettings {
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  cursorStyle: "bar" | "block" | "underline";
  cursorBlink: boolean;
  scrollback: number;
  theme: ITheme;
  copyOnSelect: boolean;
  confirmPaste: boolean;
  bell: "attention" | "sound" | "none";
  /** lines of scrollback saved with the session (0 = don't save) */
  sessionLines: number;
  /** expose output to screen readers (xterm's accessibility tree) */
  screenReader: boolean;
}

const settings: TermSettings = {
  fontFamily: '"Cascadia Code", "Cascadia Mono", Consolas, "Courier New", monospace',
  fontSize: 14,
  lineHeight: 1.1,
  cursorStyle: "bar",
  cursorBlink: true,
  scrollback: 20000,
  theme: {
    background: "#111317",
    foreground: "#d8dee9",
    cursor: "#f5b53d",
    cursorAccent: "#111317",
    selectionBackground: "#3b4252aa",
  },
  copyOnSelect: false,
  confirmPaste: true,
  bell: "attention",
  sessionLines: 2000,
  screenReader: false,
};

export function termSettings(): Readonly<TermSettings> {
  return settings;
}

export interface TermEntry {
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  serialize: SerializeAddon;
  el: HTMLDivElement;
  opened: boolean;
  replaying?: boolean;
  lastSize: string;
  /** output arrived since the last scrollback save */
  dirty: boolean;
  webgl?: WebglAddon;
}

/**
 * Browsers allow only ~16 live WebGL contexts per page; past that the oldest are lost. Keep the
 * GPU renderer on the most recently shown terminals and let the rest (hidden ones) drop to the
 * DOM renderer, re-enabling it when they're shown again.
 */
const MAX_WEBGL = 12;
const webglOrder: Id[] = [];

function enableWebgl(pane: Id, e: TermEntry) {
  const i = webglOrder.indexOf(pane);
  if (i >= 0) webglOrder.splice(i, 1);
  webglOrder.push(pane);
  if (!e.webgl) {
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        if (useStore.getState().info?.debug_perf) void api.debugLog(`webgl context lost for ${pane.slice(0, 8)}`);
        webgl.dispose();
        if (e.webgl === webgl) e.webgl = undefined;
      });
      e.term.loadAddon(webgl);
      e.webgl = webgl;
      if (useStore.getState().info?.debug_perf) void api.debugLog(`webgl on for ${pane.slice(0, 8)}`);
    } catch (err) {
      /* no WebGL: DOM renderer */
      if (useStore.getState().info?.debug_perf) void api.debugLog(`webgl failed for ${pane.slice(0, 8)}: ${err}`);
    }
  }
  // Evict least recently shown terminals that aren't on screen.
  for (let k = 0; webglOrder.length > MAX_WEBGL && k < webglOrder.length; ) {
    const id = webglOrder[k];
    const other = entries.get(id);
    if (other && other.el.isConnected) {
      k++;
      continue;
    }
    webglOrder.splice(k, 1);
    other?.webgl?.dispose();
    if (other) other.webgl = undefined;
  }
}

/** Terminal-generated replies to queries: cursor position (CSI r;c R), device attributes/status. */
const CPR_REPLY = /^(\x1b\[\d+;\d+R)+$/;
const QUERY_REPLY = /^(\x1b\[[?>]?[\d;]*[Rcn]|\x1b\[\?[\d;]*\$y|\x1b\][\d]+;[^\x07\x1b]*(\x07|\x1b\\))+$/;

const entries = new Map<Id, TermEntry>();

export function getTerminal(pane: Id): TermEntry {
  let e = entries.get(pane);
  if (e) return e;
  const term = new Terminal({
    fontFamily: settings.fontFamily,
    fontSize: settings.fontSize,
    lineHeight: settings.lineHeight,
    cursorBlink: settings.cursorBlink,
    cursorStyle: settings.cursorStyle,
    allowProposedApi: true,
    scrollback: settings.scrollback,
    theme: settings.theme,
    windowsPty: { backend: "conpty", buildNumber: 22621 },
    macOptionIsMeta: false,
    rightClickSelectsWord: false,
    screenReaderMode: settings.screenReader,
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  const serialize = new SerializeAddon();
  const unicode = new Unicode11Addon();
  term.loadAddon(fit);
  term.loadAddon(search);
  term.loadAddon(serialize);
  term.loadAddon(unicode);
  term.unicode.activeVersion = "11";
  term.loadAddon(new WebLinksAddon((ev, uri) => ev.ctrlKey && openUrl(uri)));

  const el = document.createElement("div");
  el.className = "xterm-host";

  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== "keydown") return true;
    if (isAppShortcut(ev)) return false;
    // Ctrl+C copies when there is a selection; otherwise it's SIGINT
    if (ev.ctrlKey && !ev.shiftKey && !ev.altKey && ev.key.toLowerCase() === "c" && term.hasSelection()) {
      navigator.clipboard.writeText(term.getSelection());
      term.clearSelection();
      return false;
    }
    if (ev.ctrlKey && ev.key.toLowerCase() === "v" && !ev.altKey) {
      ev.preventDefault();
      pasteInto(pane);
      return false;
    }
    if (ev.ctrlKey && ev.shiftKey && ev.key.toLowerCase() === "c") {
      if (term.hasSelection()) navigator.clipboard.writeText(term.getSelection());
      return false;
    }
    return true;
  });

  term.onData((d) => {
    // The backend answers cursor-position queries; never forward xterm's duplicate.
    if (CPR_REPLY.test(d)) return;
    if (entries.get(pane)?.replaying && QUERY_REPLY.test(d)) return;
    api.ptyWrite(pane, d);
  });
  term.onBinary((d) => api.ptyWrite(pane, d));
  term.onWriteParsed(() => {
    const en = entries.get(pane);
    if (en) en.dirty = true;
  });
  term.onSelectionChange(() => {
    if (settings.copyOnSelect && term.hasSelection()) navigator.clipboard.writeText(term.getSelection()).catch(() => {});
  });
  term.onBell(() => {
    if (settings.bell === "sound") beep();
  });
  term.onResize(({ cols, rows }) => api.ptyResize(pane, cols, rows));

  el.addEventListener("contextmenu", (ev) => {
    ev.preventDefault();
    if (term.hasSelection()) {
      navigator.clipboard.writeText(term.getSelection());
      term.clearSelection();
    } else {
      pasteInto(pane);
    }
  });

  e = { term, fit, search, serialize, el, opened: false, lastSize: "", dirty: false };
  entries.set(pane, e);
  return e;
}

async function pasteInto(pane: Id) {
  const e = entries.get(pane);
  if (!e) return;
  let text = await navigator.clipboard.readText().catch(() => "");
  if (!text) return;
  text = text.replace(/\r?\n/g, "\r");
  if (settings.confirmPaste && text.includes("\r") && !e.term.modes.bracketedPasteMode) {
    const lines = text.split("\r").filter(Boolean).length;
    if (lines > 1 && !window.confirm(`Paste ${lines} lines into the terminal?`)) return;
  }
  e.term.paste(text.replace(/\r/g, "\n"));
}

/** Mount an entry into a container; first mount opens xterm and attaches the PTY stream. */
export function mountTerminal(pane: Id, container: HTMLElement): TermEntry {
  const e = getTerminal(pane);
  if (e.el.parentElement !== container) container.appendChild(e.el);
  if (!e.opened) {
    e.term.open(e.el);
    e.opened = true;
    void attach(pane, e);
  }
  enableWebgl(pane, e);
  fitTerminal(pane);
  return e;
}

export function fitTerminal(pane: Id) {
  const e = entries.get(pane);
  // Skip while hidden/collapsing so the PTY never gets resized to a sliver (which garbles output).
  if (!e || !e.opened || !e.el.isConnected || e.el.clientWidth < 40 || e.el.clientHeight < 30) return;
  try {
    e.fit.fit();
  } catch {
    /* not visible */
  }
  const size = `${e.term.cols}x${e.term.rows}`;
  if (size !== e.lastSize) {
    e.lastSize = size;
    api.ptyResize(pane, e.term.cols, e.term.rows);
  }
}

/** Re-fit every visible terminal (e.g. after moving to a monitor with a different scale). */
export function refitAll() {
  for (const id of entries.keys()) fitTerminal(id);
}

export function focusTerminal(pane: Id) {
  entries.get(pane)?.term.focus();
}

export function disposeMissing(live: Set<Id>) {
  for (const [id, e] of entries) {
    if (!live.has(id)) {
      const i = webglOrder.indexOf(id);
      if (i >= 0) webglOrder.splice(i, 1);
      e.term.dispose();
      e.el.remove();
      entries.delete(id);
    }
  }
}

export function setFontSize(size: number) {
  applySettings({ fontSize: size });
}

/** Update settings for new terminals and push them to every existing one. */
export function applySettings(p: Partial<TermSettings>) {
  Object.assign(settings, p);
  for (const [id, e] of entries) {
    const o = e.term.options;
    if (o.fontFamily !== settings.fontFamily) o.fontFamily = settings.fontFamily;
    if (o.fontSize !== settings.fontSize) o.fontSize = settings.fontSize;
    if (o.lineHeight !== settings.lineHeight) o.lineHeight = settings.lineHeight;
    if (o.cursorStyle !== settings.cursorStyle) o.cursorStyle = settings.cursorStyle;
    if (o.cursorBlink !== settings.cursorBlink) o.cursorBlink = settings.cursorBlink;
    if (o.scrollback !== settings.scrollback) o.scrollback = settings.scrollback;
    if (o.theme !== settings.theme) o.theme = settings.theme;
    if (o.screenReaderMode !== settings.screenReader) o.screenReaderMode = settings.screenReader;
    fitTerminal(id);
  }
}

/** Settings derived from the user config. */
export function settingsFromConfig(c: Config, builtin: Theme[]): Partial<TermSettings> {
  const t = pickTheme(c, builtin);
  return {
    fontFamily: c.font.family,
    fontSize: c.font.size,
    lineHeight: c.font.line_height,
    cursorStyle: c.cursor.style,
    cursorBlink: c.cursor.blink,
    scrollback: c.terminal.scrollback,
    copyOnSelect: c.terminal.copy_on_select,
    confirmPaste: c.terminal.confirm_multiline_paste,
    bell: c.terminal.bell,
    sessionLines: c.session.scrollback ? c.session.scrollback_lines : 0,
    screenReader: c.terminal.screen_reader,
    ...(t ? { theme: themeToXterm(t) } : {}),
  };
}

let audio: AudioContext | undefined;
let lastBeep = 0;
function beep() {
  const now = Date.now();
  if (now - lastBeep < 250) return;
  lastBeep = now;
  try {
    audio ??= new AudioContext();
    const osc = audio.createOscillator();
    const gain = audio.createGain();
    osc.frequency.value = 880;
    gain.gain.setValueAtTime(0.08, audio.currentTime);
    gain.gain.exponentialRampToValueAtTime(0.0001, audio.currentTime + 0.12);
    osc.connect(gain).connect(audio.destination);
    osc.start();
    osc.stop(audio.currentTime + 0.13);
  } catch {
    /* no audio */
  }
}

/** First mount: show restored scrollback (if any), then stream the live PTY. */
async function attach(pane: Id, e: TermEntry) {
  const saved = settings.sessionLines > 0 ? await api.sessionTakeRestored(pane).catch(() => null) : null;
  const restored = saved ? sanitizeScrollback(saved) : "";
  if (restored) {
    e.replaying = true;
    e.term.write(restored);
    e.term.write(restoreTrailer(e.term.rows, new Date().toLocaleString([], { dateStyle: "medium", timeStyle: "short" })));
  }
  let first = true;
  const perf = perfProbe(pane);
  api.ptyAttach(pane, (d) => {
    // (info may arrive after the first terminal attaches, so check per chunk)
    if (useStore.getState().info?.debug_perf) perf(d.length, e.term);
    if (first) {
      // The replay contains terminal queries (e.g. ConPTY's cursor-position request) that the
      // shell already got answered; suppress xterm's duplicate replies while replaying.
      first = false;
      e.replaying = true;
      e.term.write(d, () => (e.replaying = false));
    } else {
      e.term.write(d);
    }
  });
  if (restored) e.term.write("", () => first && (e.replaying = false));
}

/**
 * HIVETTY_DEBUG_PERF: after each burst of output, report how long xterm took to parse it, from
 * the first chunk's arrival to the last chunk being parsed.
 */
function perfProbe(pane: Id) {
  let bytes = 0;
  let start = 0;
  let chunks = 0;
  let timer = 0;
  return (n: number, term: Terminal) => {
    if (!bytes) start = performance.now();
    bytes += n;
    chunks++;
    term.write("", () => {
      const end = performance.now();
      clearTimeout(timer);
      timer = window.setTimeout(() => {
        const ms = end - start;
        if (bytes > 256 * 1024) {
          void api.debugLog(`perf ${pane.slice(0, 8)}: ${(bytes / 1048576).toFixed(1)} MB in ${chunks} msgs, ${ms.toFixed(0)} ms, ${(bytes / 1048576 / (ms / 1000)).toFixed(1)} MB/s`);
        }
        bytes = 0;
        chunks = 0;
      }, 500);
    });
  };
}

/** Serialize terminals with new output and hand them to the backend for the session file. */
export async function flushScrollback(force = false) {
  const lines = settings.sessionLines;
  if (lines <= 0) return;
  const jobs: Promise<unknown>[] = [];
  for (const [id, e] of entries) {
    if (!e.opened || (!e.dirty && !force)) continue;
    e.dirty = false;
    let data: string;
    try {
      data = sanitizeScrollback(e.serialize.serialize({ scrollback: lines, excludeAltBuffer: true, excludeModes: true }));
    } catch {
      continue;
    }
    jobs.push(api.sessionPutScrollback(id, data).catch(() => {}));
  }
  await Promise.all(jobs);
}

export function clearScrollback(pane: Id) {
  entries.get(pane)?.term.clear();
}

export function terminalEntry(pane: Id) {
  return entries.get(pane);
}
