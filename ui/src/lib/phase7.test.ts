import { describe, expect, it } from "vitest";
import { computeBindings, defaultBindings, normalizeChord, prettyChord } from "./keymap";
import { pickTheme, restoreTrailer, sanitizeScrollback, themeToXterm } from "./termutil";
import { fuzzyScore } from "./actions";
import type { Theme } from "../types";

const theme = (name: string, bg = "#000000"): Theme => ({
  name,
  background: bg,
  foreground: "#ffffff",
  black: "#000001",
  red: "#ff0000",
  green: "#00ff00",
  yellow: "#ffff00",
  blue: "#0000ff",
  magenta: "#ff00ff",
  cyan: "#00ffff",
  white: "#eeeeee",
  bright_black: "#555555",
  bright_red: "#ff5555",
  bright_green: "#55ff55",
  bright_yellow: "#ffff55",
  bright_blue: "#5555ff",
  bright_magenta: "#ff55ff",
  bright_cyan: "#55ffff",
  bright_white: "#fffffe",
});

describe("keymap", () => {
  it("normalizes user-written chords", () => {
    expect(normalizeChord("Shift+Ctrl+D")).toBe("ctrl+shift+d");
    expect(normalizeChord("control + alt + ,")).toBe("ctrl+alt+,");
    expect(prettyChord("ctrl+alt+shift+f")).toBe("Ctrl+Alt+Shift+F");
  });

  it("applies overrides, unbinds with null, and adds extras without beating user bindings", () => {
    const b = computeBindings({ "Ctrl+Shift+D": null, "ctrl+alt+x": "pane.zoom", "ctrl+alt+y": "pane.flash" }, { "ctrl+alt+y": "command.user.Dev", "ctrl+alt+d": "command.user.Dev" });
    expect(b["ctrl+shift+d"]).toBeUndefined();
    expect(b["ctrl+alt+x"]).toBe("pane.zoom");
    expect(b["ctrl+alt+y"]).toBe("pane.flash");
    expect(b["ctrl+alt+d"]).toBe("command.user.Dev");
    expect(b["ctrl+shift+n"]).toBe(defaultBindings["ctrl+shift+n"]);
  });

  it("has the new phase-7 defaults", () => {
    expect(defaultBindings["ctrl+,"]).toBe("settings.open");
    expect(defaultBindings["ctrl+alt+shift+f"]).toBe("findFiles.open");
  });
});

describe("themes", () => {
  it("maps snake_case config themes to xterm", () => {
    const x = themeToXterm(theme("A"));
    expect(x.brightMagenta).toBe("#ff55ff");
    expect(x.cursor).toBe("#ffffff");
    expect(x.cursorAccent).toBe("#000000");
  });

  it("prefers the last user theme with the name, then built-ins, then the first built-in", () => {
    const builtin = [theme("Hivetty Dark", "#111111"), theme("Campbell", "#0c0c0c")];
    expect(pickTheme({ theme: "Campbell", themes: [] }, builtin)?.background).toBe("#0c0c0c");
    expect(pickTheme({ theme: "Campbell", themes: [theme("Campbell", "#222222"), theme("Campbell", "#333333")] }, builtin)?.background).toBe("#333333");
    expect(pickTheme({ theme: "Nope", themes: [] }, builtin)?.name).toBe("Hivetty Dark");
  });
});

describe("session restore", () => {
  it("resets modes and pushes restored text into scrollback", () => {
    const t = restoreTrailer(30, "today");
    expect(t.startsWith("\x1b[0m")).toBe(true);
    expect(t).not.toContain("?1049l");
    expect(t).toContain("restored session · today");
    expect(t.match(/\r\n/g)?.length).toBe(31);
    expect(t.endsWith("\x1b[H")).toBe(true);
  });
});

describe("scrollback sanitizing", () => {
  it("drops trailing cursor moves and blank rows so new output can't overwrite history", () => {
    const s = "PS C:\\> echo hi\r\nhi\r\nPS C:\\> \r\n\r\n\r\n\x1b[19A\x1b[115C";
    expect(sanitizeScrollback(s)).toBe("PS C:\\> echo hi\r\nhi\r\nPS C:\\>\x1b[0m");
  });
  it("collapses long runs of empty lines but keeps short ones", () => {
    expect(sanitizeScrollback("a\r\n\r\n\r\n\r\n\r\n\r\nb")).toBe("a\r\n\r\nb");
    expect(sanitizeScrollback("a\r\n\r\nb")).toBe("a\r\n\r\nb");
    expect(sanitizeScrollback("a\r\n\x1b[0m\r\n\r\n\r\n\r\nb")).toBe("a\r\n\r\nb");
  });
  it("leaves clean text alone", () => {
    expect(sanitizeScrollback("\x1b[31mred\x1b[0m done")).toBe("\x1b[31mred\x1b[0m done");
  });
});

describe("palette", () => {
  it("fuzzy matches custom command titles", () => {
    expect(fuzzyScore("rdev", "Run: Dev server")).toBeGreaterThan(0);
    expect(fuzzyScore("xyz", "Run: Dev server")).toBe(-1);
  });
});
