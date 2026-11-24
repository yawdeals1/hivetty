// Pure terminal helpers (no DOM), kept separate so they can be unit tested.
import type { ITheme } from "@xterm/xterm";
import type { Config, Theme } from "../types";

/** Map a config theme (snake_case) to xterm's ITheme. */
export function themeToXterm(t: Theme): ITheme {
  return {
    background: t.background,
    foreground: t.foreground,
    cursor: t.cursor ?? t.foreground,
    cursorAccent: t.background,
    selectionBackground: t.selection_background ?? "#ffffff40",
    black: t.black,
    red: t.red,
    green: t.green,
    yellow: t.yellow,
    blue: t.blue,
    magenta: t.magenta,
    cyan: t.cyan,
    white: t.white,
    brightBlack: t.bright_black,
    brightRed: t.bright_red,
    brightGreen: t.bright_green,
    brightYellow: t.bright_yellow,
    brightBlue: t.bright_blue,
    brightMagenta: t.bright_magenta,
    brightCyan: t.bright_cyan,
    brightWhite: t.bright_white,
  };
}

/** The theme a config selects: user themes first (last definition wins), then built-ins. */
export function pickTheme(c: Pick<Config, "theme" | "themes">, builtin: Theme[]): Theme | undefined {
  return [...c.themes].reverse().find((t) => t.name === c.theme) ?? builtin.find((t) => t.name === c.theme) ?? builtin[0];
}

/** Text written after restored scrollback: reset attributes/modes, a divider, then enough
 *  newlines to push it all into scrollback (the new shell owns the whole viewport). */
export function restoreTrailer(rows: number, stamp: string): string {
  // Reset SGR, show cursor, stop mouse reporting / bracketed paste. (No "leave alternate
  // screen": the text is serialized without it, and ?1049l would also restore the saved
  // cursor — row 1 — making the divider overwrite the restored history.)
  const reset = "\x1b[0m\x1b[?25h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l";
  // Finally home the cursor: ConPTY's first repaint then clears the (blank) viewport in place
  // instead of scrolling more empty lines into history.
  return `${reset}\r\n\x1b[2m── restored session · ${stamp} ──\x1b[0m` + "\r\n".repeat(Math.max(1, rows)) + "\x1b[H";
}


/** Clean serialized terminal text for saving/replaying: drop the trailing cursor moves the
 *  serializer emits (they point into the old viewport and would make new output overwrite
 *  history), drop trailing blank rows, and collapse long runs of empty lines. */
export function sanitizeScrollback(s: string): string {
  const trailing = /(?:\x1b\[[0-9;]*[ABCDEFGHJKdfm]|\r|\n|[ \t])+$/;
  let out = s.replace(trailing, "");
  // keep the SGR reset if the text ended inside a colored run
  if (out.length < s.length) out += "\x1b[0m";
  return out.replace(/(?:\r\n(?:\x1b\[[0-9;]*m)*[ \t]*){4,}/g, "\r\n\r\n");
}
