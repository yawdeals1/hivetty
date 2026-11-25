// Interface themes: the colors of everything around the terminal (sidebar, tabs, panels,
// dialogs). Each theme names a handful of base colors; styles.css derives hover, border and
// dim-text shades from them with color-mix, so a theme stays coherent with very few values.
import type { Theme } from "../types";

export interface UiTheme {
  name: string;
  /** short description shown under the name in Settings */
  note: string;
  scheme: "dark" | "light";
  /** window background (behind the terminal) */
  bg: string;
  /** sidebar, tab bar, panels, dialogs */
  surface: string;
  text: string;
  /** focus, active tab, running indicator */
  accent: string;
  /** unread notifications and "needs you" */
  attention: string;
  danger: string;
  ok: string;
}

export const MATCH_TERMINAL = "match";

export const UI_THEMES: UiTheme[] = [
  { name: "Honeycomb", note: "Graphite with a honey accent", scheme: "dark", bg: "#111317", surface: "#181b21", text: "#d8dee9", accent: "#f5b53d", attention: "#4aa8ff", danger: "#ef6b73", ok: "#8fd18b" },
  { name: "Propolis", note: "Warm resin brown", scheme: "dark", bg: "#16100d", surface: "#1f1713", text: "#ecdccd", accent: "#ea9f3a", attention: "#6ccbc3", danger: "#f27a6b", ok: "#a9d27c" },
  { name: "Night Shift", note: "Deep ink blue", scheme: "dark", bg: "#0c1220", surface: "#121a2b", text: "#d3ddf0", accent: "#82a8ff", attention: "#ffb454", danger: "#ff6b81", ok: "#7fd99a" },
  { name: "Lichen", note: "Grey-green with a lime accent", scheme: "dark", bg: "#111613", surface: "#18201b", text: "#d4ded6", accent: "#b9d66b", attention: "#6fc3df", danger: "#e8796f", ok: "#8fd18b" },
  { name: "Aubergine", note: "Dark plum with gold", scheme: "dark", bg: "#150f1a", surface: "#1d1524", text: "#e5d9ee", accent: "#f0b860", attention: "#9db8ff", danger: "#ff7a90", ok: "#93d6a0" },
  { name: "Contrast", note: "Black and white, maximum legibility", scheme: "dark", bg: "#000000", surface: "#0b0b0b", text: "#ffffff", accent: "#ffd400", attention: "#38b6ff", danger: "#ff5c5c", ok: "#4dff88" },
  { name: "Beeswax", note: "Light wax yellow", scheme: "light", bg: "#fffbea", surface: "#f6edca", text: "#2c2416", accent: "#a96800", attention: "#1d6fd0", danger: "#c4323c", ok: "#2e7d36" },
  { name: "Porcelain", note: "Cool light grey", scheme: "light", bg: "#f8f9fb", surface: "#eceff4", text: "#1c222b", accent: "#3757d4", attention: "#c75000", danger: "#c42b3a", ok: "#2b7a3d" },
];

function rgb(hex: string): [number, number, number] | null {
  const m = /^#?([0-9a-f]{6})/i.exec(hex.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function luminance(hex: string): number {
  const c = rgb(hex);
  if (!c) return 0;
  const [r, g, b] = c.map((v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Chroma (0..1): how far a color is from grey. */
function chroma(hex: string): number {
  const c = rgb(hex);
  if (!c) return 0;
  return (Math.max(...c) - Math.min(...c)) / 255;
}

function mix(a: string, b: string, t: number): string {
  const x = rgb(a);
  const y = rgb(b);
  if (!x || !y) return a;
  const v = x.map((c, i) => Math.round(c + (y[i] - c) * t));
  return `#${v.map((c) => c.toString(16).padStart(2, "0")).join("")}`;
}

/** An interface theme built from a terminal color scheme. */
export function fromTerminal(t: Theme): UiTheme {
  const light = luminance(t.background) > 0.4;
  const cursor = t.cursor ?? "";
  const accent = chroma(cursor) > 0.35 ? cursor : light ? t.yellow : t.bright_yellow ?? t.yellow;
  return {
    name: MATCH_TERMINAL,
    note: `Follows ${t.name}`,
    scheme: light ? "light" : "dark",
    bg: t.background,
    surface: mix(t.background, t.foreground, light ? 0.06 : 0.04),
    text: t.foreground,
    accent,
    attention: light ? t.blue : t.bright_blue ?? t.blue,
    danger: light ? t.red : t.bright_red ?? t.red,
    ok: light ? t.green : t.bright_green ?? t.green,
  };
}

export function pickUiTheme(name: string | undefined, terminal: Theme | undefined): UiTheme {
  const named = UI_THEMES.find((t) => t.name === name);
  if (named) return named;
  return terminal ? fromTerminal(terminal) : UI_THEMES[0];
}

/** Text that stays readable on top of a solid `color` (badges, primary buttons). */
export function onColor(color: string): string {
  return luminance(color) > 0.35 ? "#0b0d10" : "#ffffff";
}

export function uiThemeVars(t: UiTheme): Record<string, string> {
  return {
    "--bg": t.bg,
    "--surface": t.surface,
    "--text": t.text,
    "--accent": t.accent,
    "--attention": t.attention,
    "--danger": t.danger,
    "--ok": t.ok,
    "--on-accent": onColor(t.accent),
    "--on-attention": onColor(t.attention),
    "--shadow": t.scheme === "light" ? "rgb(40 36 20 / 0.18)" : "rgb(0 0 0 / 0.55)",
    "--scrim": t.scheme === "light" ? "rgb(30 28 20 / 0.22)" : "rgb(0 0 0 / 0.45)",
  };
}

export function applyUiTheme(t: UiTheme) {
  const root = document.documentElement;
  for (const [k, v] of Object.entries(uiThemeVars(t))) root.style.setProperty(k, v);
  root.dataset.scheme = t.scheme;
  root.style.colorScheme = t.scheme;
}
