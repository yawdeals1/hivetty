// Replays a saved session into a headless xterm the same way the app does, followed by the
// bytes ConPTY really sends when a PowerShell pane starts, and checks nothing is overwritten.
import { describe, expect, it } from "vitest";
import { Terminal } from "@xterm/headless";
import { restoreTrailer, sanitizeScrollback } from "./termutil";

const write = (t: Terminal, s: string) => new Promise<void>((r) => t.write(s, r));

function lines(t: Terminal): string[] {
  const b = t.buffer.active;
  const out: string[] = [];
  for (let i = 0; i < b.length; i++) {
    const l = b.getLine(i);
    if (!l) continue;
    if (l.isWrapped && out.length) out[out.length - 1] += l.translateToString(true);
    else out.push(l.translateToString(true));
  }
  return out.map((l) => l.trimEnd());
}

// Captured from a real pane (HIVETTY_DEBUG_RAW): ConPTY's first output on a 141x20 PowerShell.
const conptyStart =
  "\x1b[?9001h\x1b[?1004h\x1b[m\x1b]0;powershell.exe\x07\x1b[?25h\x1b[?25l\x1b[8;20;141t" +
  "\x1b[K\r\n".repeat(19) +
  "\x1b[K\x1b[H\x1b[?25h\x1b]133;A\x1b\\PS C:\\proj> \x1b]133;B\x1b\\";

describe("session restore replay", () => {
  it("keeps every restored line above the divider and the new prompt below it", async () => {
    const t = new Terminal({ cols: 141, rows: 20, allowProposedApi: true });
    const saved = "PS C:\\> cd proj\r\n\x1b[93mecho\x1b[37m one\r\n\x1b[0mone\r\nPS C:\\proj> \r\n\r\n\r\n\x1b[19A\x1b[115C";
    await write(t, sanitizeScrollback(saved));
    await write(t, restoreTrailer(t.rows, "now"));
    await write(t, conptyStart);
    const all = lines(t).filter((l) => l.trim());
    expect(all).toEqual(["PS C:\\> cd proj", "echo one", "one", "PS C:\\proj>", "── restored session · now ──", "PS C:\\proj>"]);
    // the live prompt is on the first viewport row, history is in scrollback
    expect(t.buffer.active.baseY).toBeGreaterThan(0);
    expect(t.buffer.active.getLine(t.buffer.active.baseY)?.translateToString(true).trimEnd()).toBe("PS C:\\proj>");
  });

  it("survives restoring twice", async () => {
    const t = new Terminal({ cols: 141, rows: 20, allowProposedApi: true });
    await write(t, sanitizeScrollback("first\r\nPS C:\\> "));
    await write(t, restoreTrailer(t.rows, "one"));
    await write(t, conptyStart);
    // what the app would save now, then restore into a fresh terminal
    const { SerializeAddon } = await import("@xterm/addon-serialize");
    const ser = new SerializeAddon();
    t.loadAddon(ser as never);
    const saved = ser.serialize({ scrollback: 2000, excludeAltBuffer: true, excludeModes: true });
    const t2 = new Terminal({ cols: 141, rows: 20, allowProposedApi: true });
    await write(t2, sanitizeScrollback(saved));
    await write(t2, restoreTrailer(t2.rows, "two"));
    await write(t2, conptyStart);
    expect(lines(t2).filter((l) => l.trim())).toEqual(["first", "PS C:\\>", "── restored session · one ──", "PS C:\\proj>", "── restored session · two ──", "PS C:\\proj>"]);
  });
});
