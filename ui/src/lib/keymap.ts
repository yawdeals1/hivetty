// Chord normalization + binding table. Bindings map chord -> action id; actions live in actions.ts.

export function chordOf(ev: KeyboardEvent): string {
  const parts: string[] = [];
  if (ev.ctrlKey) parts.push("ctrl");
  if (ev.altKey) parts.push("alt");
  if (ev.shiftKey) parts.push("shift");
  if (ev.metaKey) parts.push("meta");
  let k = ev.key;
  // Use the physical key for letters/digits so Shift doesn't change the name ("D" vs "d", "!" vs "1").
  if (/^Key[A-Z]$/.test(ev.code)) k = ev.code.slice(3);
  else if (/^Digit[0-9]$/.test(ev.code)) k = ev.code.slice(5);
  else if (ev.code === "Equal") k = "=";
  else if (ev.code === "Minus") k = "-";
  else if (ev.code === "BracketLeft") k = "[";
  else if (ev.code === "BracketRight") k = "]";
  else if (ev.code === "Comma") k = ",";
  else if (ev.code === "Period") k = ".";
  else if (ev.code === "Slash") k = "/";
  else if (ev.code === "Semicolon") k = ";";
  else if (ev.code === "Backquote") k = "`";
  else if (k === " ") k = "space";
  k = k.toLowerCase();
  if (["control", "alt", "shift", "meta"].includes(k)) return "";
  parts.push(k);
  return parts.join("+");
}

/** Canonicalize a user-written chord like "Ctrl+Shift+D" to "ctrl+shift+d". */
export function normalizeChord(s: string): string {
  const order = ["ctrl", "alt", "shift", "meta"];
  const toks = s.toLowerCase().split("+").map((t) => t.trim()).filter(Boolean);
  const mods = order.filter((m) => toks.includes(m) || (m === "ctrl" && toks.includes("control")));
  const key = toks.filter((t) => !order.includes(t) && t !== "control").pop() ?? "";
  return [...mods, key].join("+");
}

export const defaultBindings: Record<string, string> = {
  "ctrl+shift+n": "workspace.new",
  "ctrl+shift+t": "surface.new",
  "ctrl+shift+w": "pane.close",
  "ctrl+shift+d": "pane.splitRight",
  "ctrl+alt+shift+d": "pane.splitDown",
  "alt+shift+=": "pane.splitRight",
  "alt+shift+-": "pane.splitDown",
  "alt+arrowleft": "pane.focusLeft",
  "alt+arrowright": "pane.focusRight",
  "alt+arrowup": "pane.focusUp",
  "alt+arrowdown": "pane.focusDown",
  "alt+shift+arrowleft": "pane.resizeLeft",
  "alt+shift+arrowright": "pane.resizeRight",
  "alt+shift+arrowup": "pane.resizeUp",
  "alt+shift+arrowdown": "pane.resizeDown",
  "ctrl+shift+z": "pane.zoom",
  "ctrl+shift+h": "pane.flash",
  "ctrl+shift+=": "pane.equalize",
  "ctrl+tab": "surface.next",
  "ctrl+shift+tab": "surface.prev",
  "ctrl+pagedown": "workspace.next",
  "ctrl+pageup": "workspace.prev",
  "ctrl+shift+]": "workspace.next",
  "ctrl+shift+[": "workspace.prev",
  "ctrl+shift+p": "palette.open",
  "ctrl+shift+i": "notifications.toggle",
  "ctrl+shift+u": "notifications.jumpUnread",
  "ctrl+shift+b": "sidebar.toggle",
  "ctrl+shift+r": "workspace.rename",
  "ctrl+shift+f": "find.open",
  "ctrl+alt+shift+f": "findFiles.open",
  "ctrl+,": "settings.open",
  "ctrl+shift+k": "terminal.clear",
  "ctrl+=": "font.increase",
  "ctrl+-": "font.decrease",
  "ctrl+0": "font.reset",
  ...Object.fromEntries(Array.from({ length: 8 }, (_, i) => [`ctrl+alt+${i + 1}`, `workspace.goto${i + 1}`])),
  "ctrl+alt+9": "workspace.gotoLast",
  ...Object.fromEntries(Array.from({ length: 8 }, (_, i) => [`alt+${i + 1}`, `surface.goto${i + 1}`])),
  "alt+9": "surface.gotoLast",
};

let bindings: Record<string, string> = { ...defaultBindings };

/** Build the active table: defaults, then user overrides (null unbinds), then extra bindings
 *  such as custom-command chords (which never replace an explicit user binding). */
export function computeBindings(overrides: Record<string, string | null>, extra: Record<string, string> = {}): Record<string, string> {
  const next: Record<string, string> = { ...defaultBindings };
  for (const [chord, action] of Object.entries(overrides)) {
    const c = normalizeChord(chord);
    if (action === null || action === "") delete next[c];
    else next[c] = action;
  }
  for (const [chord, action] of Object.entries(extra)) {
    const c = normalizeChord(chord);
    if (!(c in overrides) && !Object.keys(overrides).some((k) => normalizeChord(k) === c)) next[c] = action;
  }
  return next;
}

export function setBindings(overrides: Record<string, string | null>, extra: Record<string, string> = {}) {
  bindings = computeBindings(overrides, extra);
}

export function currentBindings(): Readonly<Record<string, string>> {
  return bindings;
}

let suspended = false;
/** While recording a new shortcut, app shortcuts must not fire. */
export function setSuspended(v: boolean) {
  suspended = v;
}

export function actionFor(ev: KeyboardEvent): string | undefined {
  if (suspended) return undefined;
  const c = chordOf(ev);
  return c ? bindings[c] : undefined;
}

export function isAppShortcut(ev: KeyboardEvent): boolean {
  return actionFor(ev) !== undefined;
}

export function chordsFor(action: string): string[] {
  return Object.entries(bindings)
    .filter(([, a]) => a === action)
    .map(([c]) => c);
}

export function prettyChord(c: string): string {
  return c
    .split("+")
    .map((t) =>
      ({ ctrl: "Ctrl", alt: "Alt", shift: "Shift", meta: "Win", arrowleft: "←", arrowright: "→", arrowup: "↑", arrowdown: "↓", pagedown: "PgDn", pageup: "PgUp", tab: "Tab", space: "Space", escape: "Esc", enter: "Enter" })[t] ??
      t.toUpperCase(),
    )
    .join("+");
}
