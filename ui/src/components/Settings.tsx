import { useEffect, useMemo, useState } from "react";
import { api } from "../lib/api";
import { buildActions } from "../lib/actions";
import { chordOf, currentBindings, defaultBindings, normalizeChord, prettyChord, setSuspended } from "../lib/keymap";
import { useStore } from "../state/store";
import { closeOverlay } from "./Overlays";
import { checkForUpdates, installUpdate } from "../lib/updates";
import type { CommandTarget, Config, CustomCommand, HostStatus, Theme } from "../types";

type Section = "appearance" | "terminal" | "shells" | "session" | "keyboard" | "commands" | "updates";

const SECTIONS: [Section, string][] = [
  ["appearance", "Appearance"],
  ["terminal", "Terminal"],
  ["shells", "Shells"],
  ["session", "Session"],
  ["keyboard", "Keyboard"],
  ["commands", "Commands"],
  ["updates", "Updates"],
];

/** Text/number input that commits on blur or Enter (so typing doesn't rewrite the file per key). */
function Field({ value, onCommit, type = "text", placeholder, min, max, step, wide }: {
  value: string | number;
  onCommit: (v: string) => void;
  type?: "text" | "number";
  placeholder?: string;
  min?: number;
  max?: number;
  step?: number;
  wide?: boolean;
}) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const commit = () => {
    if (draft !== String(value)) onCommit(draft);
  };
  return (
    <input
      className={wide ? "wide" : ""}
      type={type}
      value={draft}
      min={min}
      max={max}
      step={step}
      placeholder={placeholder}
      spellCheck={false}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") {
          setDraft(String(value));
          e.stopPropagation();
        }
      }}
    />
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="set-row">
      <div className="set-label">
        <div>{label}</div>
        {hint && <div className="set-hint">{hint}</div>}
      </div>
      <div className="set-control">{children}</div>
    </div>
  );
}

function Toggle({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <button role="switch" aria-checked={checked} className={`switch ${checked ? "on" : ""}`} onClick={() => onChange(!checked)}>
      <span />
    </button>
  );
}

const num = (s: string) => {
  const n = Number(s);
  return Number.isFinite(n) ? n : undefined;
};

export function Settings() {
  const cs = useStore((s) => s.config);
  const [section, setSection] = useState<Section>("appearance");
  const [error, setError] = useState<string | null>(null);
  if (!cs) return null;
  const c = cs.config;
  const save = (path: string[], value: unknown) =>
    api
      .configSet(path, value)
      .then(() => setError(null))
      .catch((e) => setError(String(e)));

  return (
    <div className="overlay-backdrop" onMouseDown={closeOverlay}>
      <div
        className="settings"
        role="dialog"
        aria-label="Settings"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") closeOverlay();
        }}
      >
        <nav className="set-nav">
          <div className="set-title">Settings</div>
          {SECTIONS.map(([id, name]) => (
            <button key={id} className={section === id ? "on" : ""} onClick={() => setSection(id)} autoFocus={id === section}>
              {name}
            </button>
          ))}
          <div className="set-file">
            <button onClick={() => api.configOpen()} title={cs.path}>
              Edit hivetty.json
            </button>
          </div>
        </nav>
        <div className="set-body">
          {(cs.error || error || cs.warnings.length > 0) && (
            <div className="set-problems">
              {cs.error && <div className="err">Settings file error (previous settings kept): {cs.error}</div>}
              {error && <div className="err">{error}</div>}
              {cs.warnings.map((w) => (
                <div key={w}>⚠ {w}</div>
              ))}
            </div>
          )}
          {section === "appearance" && <Appearance c={c} builtin={cs.builtin_themes} save={save} />}
          {section === "terminal" && <TerminalSection c={c} save={save} />}
          {section === "shells" && <Shells c={c} save={save} />}
          {section === "session" && <SessionSection c={c} save={save} />}
          {section === "keyboard" && <Keyboard c={c} save={save} />}
          {section === "commands" && <Commands c={c} save={save} />}
          {section === "updates" && <UpdatesSection c={c} save={save} />}
        </div>
        <button className="set-close" title="Close (Esc)" onClick={closeOverlay}>
          ×
        </button>
      </div>
    </div>
  );
}

type Save = (path: string[], value: unknown) => Promise<unknown>;

function Swatch({ t, active, onPick, onDelete }: { t: Theme; active: boolean; onPick: () => void; onDelete?: () => void }) {
  const colors = [t.red, t.green, t.yellow, t.blue, t.magenta, t.cyan];
  return (
    <div className={`theme-card ${active ? "on" : ""}`} onClick={onPick} role="radio" aria-checked={active} tabIndex={0} onKeyDown={(e) => e.key === "Enter" && onPick()}>
      <div className="theme-preview" style={{ background: t.background, color: t.foreground }}>
        <span>~/dev $ ls</span>
        <span className="theme-dots">
          {colors.map((col, i) => (
            <i key={i} style={{ background: col }} />
          ))}
        </span>
      </div>
      <div className="theme-name">
        {t.name}
        {onDelete && (
          <span
            className="theme-del"
            title="Remove theme"
            onClick={(e) => {
              e.stopPropagation();
              onDelete();
            }}
          >
            ×
          </span>
        )}
      </div>
    </div>
  );
}

function Appearance({ c, builtin, save }: { c: Config; builtin: Theme[]; save: Save }) {
  const [importing, setImporting] = useState<Theme[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [importErr, setImportErr] = useState<string | null>(null);
  const [paste, setPaste] = useState<string | null>(null);
  const custom = c.themes;
  const startImport = (text?: string) => {
    setImportErr(null);
    api
      .configImportWt(text ? { text } : {})
      .then((list) => {
        setImporting(list);
        setPicked(new Set(list.filter((t) => !builtin.some((b) => b.name === t.name)).map((t) => t.name)));
        setPaste(null);
      })
      .catch((e) => {
        setImportErr(String(e));
        if (!text) setPaste((p) => p ?? "");
      });
  };
  const doImport = () => {
    const add = (importing ?? []).filter((t) => picked.has(t.name));
    const next = [...custom.filter((t) => !add.some((a) => a.name === t.name)), ...add];
    void save(["themes"], next);
    setImporting(null);
  };
  return (
    <section>
      <h2>Theme</h2>
      <div className="theme-grid" role="radiogroup" aria-label="Theme">
        {builtin.map((t) => (
          <Swatch key={t.name} t={t} active={c.theme === t.name} onPick={() => save(["theme"], t.name)} />
        ))}
        {custom.map((t, i) => (
          <Swatch
            key={`c-${t.name}-${i}`}
            t={t}
            active={c.theme === t.name}
            onPick={() => save(["theme"], t.name)}
            onDelete={() => save(["themes"], custom.filter((_, j) => j !== i))}
          />
        ))}
      </div>
      <div className="set-actions">
        <button onClick={() => startImport()}>Import from Windows Terminal…</button>
        {paste === null && <button onClick={() => setPaste("")}>Paste a scheme…</button>}
      </div>
      {importErr && <div className="set-hint err">{importErr}</div>}
      {paste !== null && (
        <div className="import-box">
          <div className="set-hint">Paste a Windows Terminal color scheme (one object, a list, or a whole settings.json).</div>
          <textarea className="set-paste" value={paste} spellCheck={false} onChange={(e) => setPaste(e.target.value)} placeholder={'{ "name": "My Theme", "background": "#1e1e2e", "foreground": "#cdd6f4", "black": "#45475a", ... }'} />
          <div className="set-actions">
            <button onClick={() => startImport(paste)} disabled={!paste.trim()}>
              Read scheme
            </button>
            <button onClick={() => setPaste(null)}>Cancel</button>
          </div>
        </div>
      )}
      {importing && (
        <div className="import-box">
          <div className="set-hint">Pick the color schemes to add.</div>
          <div className="import-list">
            {importing.map((t) => (
              <label key={t.name} className="fif-check">
                <input
                  type="checkbox"
                  checked={picked.has(t.name)}
                  onChange={() =>
                    setPicked((p) => {
                      const n = new Set(p);
                      if (n.has(t.name)) n.delete(t.name);
                      else n.add(t.name);
                      return n;
                    })
                  }
                />
                <span className="mini-swatch" style={{ background: t.background, borderColor: t.foreground }} />
                {t.name}
              </label>
            ))}
          </div>
          <div className="set-actions">
            <button onClick={doImport} disabled={picked.size === 0}>
              Add {picked.size} {picked.size === 1 ? "theme" : "themes"}
            </button>
            <button onClick={() => setImporting(null)}>Cancel</button>
          </div>
        </div>
      )}
      <h2>Font</h2>
      <Row label="Font family" hint="CSS font list; the first installed font wins.">
        <Field wide value={c.font.family} onCommit={(v) => save(["font", "family"], v || undefined)} />
      </Row>
      <Row label="Size">
        <Field type="number" min={6} max={48} step={1} value={c.font.size} onCommit={(v) => save(["font", "size"], num(v))} />
      </Row>
      <Row label="Line height">
        <Field type="number" min={0.8} max={3} step={0.05} value={c.font.line_height} onCommit={(v) => save(["font", "line_height"], num(v))} />
      </Row>
      <h2>Cursor</h2>
      <Row label="Style">
        <select value={c.cursor.style} onChange={(e) => save(["cursor", "style"], e.target.value)}>
          <option value="bar">Bar</option>
          <option value="block">Block</option>
          <option value="underline">Underline</option>
        </select>
      </Row>
      <Row label="Blink">
        <Toggle checked={c.cursor.blink} onChange={(v) => save(["cursor", "blink"], v)} />
      </Row>
    </section>
  );
}

function TerminalSection({ c, save }: { c: Config; save: Save }) {
  const t = c.terminal;
  return (
    <section>
      <h2>Terminal</h2>
      <Row label="Scrollback lines" hint="Per terminal. Larger values use more memory.">
        <Field type="number" min={0} max={200000} step={1000} value={t.scrollback} onCommit={(v) => save(["terminal", "scrollback"], num(v))} />
      </Row>
      <Row label="Copy on select" hint="Copy text to the clipboard as soon as you select it.">
        <Toggle checked={t.copy_on_select} onChange={(v) => save(["terminal", "copy_on_select"], v)} />
      </Row>
      <Row label="Confirm multi-line paste" hint="Ask before pasting several lines into a shell without bracketed paste.">
        <Toggle checked={t.confirm_multiline_paste} onChange={(v) => save(["terminal", "confirm_multiline_paste"], v)} />
      </Row>
      <Row label="tmux compatibility" hint="Put a tmux-compatible command on PATH in local terminals so tools that drive tmux (e.g. Claude Code agent teams) open Hivetty splits. Applies to new terminals.">
        <Toggle checked={t.tmux_shim} onChange={(v) => save(["terminal", "tmux_shim"], v)} />
      </Row>
      <Row label="WSL shell integration" hint="In WSL terminals whose login shell is bash, report the folder and prompt marks so splits, git info and Find in Files follow you.">
        <Toggle checked={t.wsl_shell_integration} onChange={(v) => save(["terminal", "wsl_shell_integration"], v)} />
      </Row>
      <Row label="Screen reader support" hint="Expose terminal output to Narrator, NVDA or JAWS. Makes heavy output a little slower.">
        <Toggle checked={t.screen_reader} onChange={(v) => save(["terminal", "screen_reader"], v)} />
      </Row>
      <Row label="Bell" hint="What a bell (BEL) does in a pane you're not looking at.">
        <select value={t.bell} onChange={(e) => save(["terminal", "bell"], e.target.value)}>
          <option value="attention">Mark the pane</option>
          <option value="sound">Mark the pane and beep</option>
          <option value="none">Ignore</option>
        </select>
      </Row>
      <h2>Opening files</h2>
      <Row label="Editor command" hint="Used by Find in Files. {file}, {line} and {col} are filled in. Leave empty for VS Code (if installed) or the default app.">
        <Field wide placeholder="nvim +{line} {file}" value={c.editor?.command ?? ""} onCommit={(v) => save(["editor"], v.trim() ? { command: v.trim(), terminal: c.editor?.terminal ?? true } : undefined)} />
      </Row>
      {c.editor && (
        <Row label="Run editor in a split" hint="On for terminal editors (vim, helix, micro); off for GUI editors.">
          <Toggle checked={c.editor.terminal} onChange={(v) => save(["editor", "terminal"], v)} />
        </Row>
      )}
    </section>
  );
}

function Shells({ c, save }: { c: Config; save: Save }) {
  const info = useStore((s) => s.info);
  const profiles = info?.profiles ?? [];
  const [draft, setDraft] = useState({ name: "", command: "", args: "" });
  const add = () => {
    if (!draft.name.trim() || !draft.command.trim()) return;
    let id = draft.name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "custom";
    while (profiles.some((p) => p.id === id) || c.profiles.some((p) => p.id === id)) id += "-2";
    const args = draft.args.match(/"[^"]*"|\S+/g)?.map((a) => a.replace(/^"|"$/g, "")) ?? [];
    void save(["profiles"], [...c.profiles, { id, name: draft.name.trim(), command: draft.command.trim(), args }]);
    setDraft({ name: "", command: "", args: "" });
  };
  return (
    <section>
      <h2>Shells</h2>
      <Row label="Default shell" hint="Used for new workspaces, tabs and splits.">
        <select value={info?.default_profile ?? ""} onChange={(e) => save(["default_profile"], e.target.value)}>
          {profiles.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
      </Row>
      <div className="set-list">
        {profiles.map((p) => {
          const isCustom = c.profiles.some((cp) => cp.id === p.id);
          return (
            <div key={p.id} className="set-item">
              <div>
                <div>
                  {p.name} <code className="dim">{p.id}</code>
                </div>
                <div className="set-hint mono">
                  {p.command} {p.args.join(" ")}
                </div>
              </div>
              {isCustom ? (
                <button onClick={() => save(["profiles"], c.profiles.filter((cp) => cp.id !== p.id))}>Remove</button>
              ) : (
                <button onClick={() => save(["hidden_profiles"], [...c.hidden_profiles, p.id])} disabled={profiles.length <= 1}>
                  Hide
                </button>
              )}
            </div>
          );
        })}
        {c.hidden_profiles.map((id) => (
          <div key={id} className="set-item hidden">
            <div>
              <code>{id}</code> <span className="set-hint">hidden</span>
            </div>
            <button onClick={() => save(["hidden_profiles"], c.hidden_profiles.filter((h) => h !== id))}>Show</button>
          </div>
        ))}
      </div>
      <h2>Add a shell</h2>
      <div className="set-form">
        <input placeholder="Name (e.g. Nushell)" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
        <input placeholder="Program (e.g. nu.exe)" value={draft.command} onChange={(e) => setDraft({ ...draft, command: e.target.value })} spellCheck={false} />
        <input placeholder="Arguments" value={draft.args} onChange={(e) => setDraft({ ...draft, args: e.target.value })} spellCheck={false} onKeyDown={(e) => e.key === "Enter" && add()} />
        <button onClick={add} disabled={!draft.name.trim() || !draft.command.trim()}>
          Add
        </button>
      </div>
      <div className="set-hint">For a starting folder or environment variables, edit the profile in hivetty.json.</div>
    </section>
  );
}

function HostRow() {
  const [h, setH] = useState<HostStatus | null>(null);
  const refresh = () => void api.hostStatus().then(setH).catch(() => setH(null));
  useEffect(refresh, []);
  if (!h) return null;
  return (
    <Row label="Session host" hint={h.exe ? undefined : "hivetty-host.exe wasn't found next to Hivetty, so shells run inside the app."}>
      <span className="set-hint">{h.running ? `Running · ${h.live} of ${h.sessions} shells alive` : "Not running"}</span>
      {h.running && (
        <button
          onClick={() => {
            if (window.confirm("Stop the session host? Every shell it holds ends.")) void api.hostStop().then(refresh);
          }}
        >
          Stop
        </button>
      )}
    </Row>
  );
}

function SessionSection({ c, save }: { c: Config; save: Save }) {
  const s = c.session;
  const [saved, setSaved] = useState<string | null>(null);
  return (
    <section>
      <h2>Session</h2>
      <Row label="Restore on launch" hint="Reopen workspaces, tabs, splits, folders and browser pages.">
        <Toggle checked={s.restore} onChange={(v) => save(["session", "restore"], v)} />
      </Row>
      <Row label="Restore scrollback" hint="Save terminal output with the session. Secret-looking values (tokens, passwords, keys) are redacted before saving.">
        <Toggle checked={s.scrollback} onChange={(v) => save(["session", "scrollback"], v)} />
      </Row>
      {s.scrollback && (
        <Row label="Lines saved per terminal">
          <Field type="number" min={0} max={100000} step={500} value={s.scrollback_lines} onCommit={(v) => save(["session", "scrollback_lines"], num(v))} />
        </Row>
      )}
      <Row label="Resume agent sessions automatically" hint="Run e.g. claude --resume <id> in panes that had an agent session. Off: restored panes show a Resume button.">
        <Toggle checked={s.resume_agents} onChange={(v) => save(["session", "resume_agents"], v)} />
      </Row>
      <Row label="Keep shells running after closing" hint="New terminals run in a background session host (hivetty-host.exe), so shells and agents keep working when Hivetty closes and are reattached on the next launch.">
        <Toggle checked={s.persistent} onChange={(v) => save(["session", "persistent"], v)} />
      </Row>
      <HostRow />
      <div className="set-actions">
        <button onClick={() => api.sessionSave().then((p) => setSaved(p)).catch((e) => setSaved(String(e)))}>Save session now</button>
        {saved && <span className="set-hint mono">{saved}</span>}
      </div>
      <div className="set-hint">Commands that started a pane are not saved or re-run.</div>
    </section>
  );
}

function UpdatesSection({ c, save }: { c: Config; save: Save }) {
  const u = c.updates;
  const st = useStore((s) => s.update);
  const progress = useStore((s) => s.updateProgress);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    api.updateStatus().then((s) => useStore.getState().set({ update: s }));
  }, [u.channel, u.endpoint]);
  const busy = st?.state === "checking" || st?.state === "downloading" || st?.state === "installing" || progress !== null;
  let line = "";
  switch (st?.state) {
    case "unconfigured":
      line = "This build has no update server. Set an update URL below to enable updates.";
      break;
    case "checking":
      line = "Checking…";
      break;
    case "none":
      line = `You're up to date (${st.channel}).`;
      break;
    case "available":
      line = `Hivetty ${st.version} is available.`;
      break;
    case "downloading":
      line = progress !== null ? `Downloading… ${Math.round(progress * 100)}%` : "Downloading…";
      break;
    case "installing":
      line = "Installing; Hivetty will restart.";
      break;
    case "error":
      line = `Couldn't check for updates: ${st.error}`;
      break;
  }
  const persistent = c.session.persistent;
  return (
    <section>
      <h2>Updates</h2>
      <Row label="Version">
        <span className="mono">{st?.current ?? useStore.getState().info?.version ?? ""}</span>
      </Row>
      <Row label="Channel" hint="Stable gets tested releases. Nightly gets a build of the latest code every day.">
        <select value={u.channel} onChange={(e) => save(["updates", "channel"], e.target.value)}>
          <option value="stable">Stable</option>
          <option value="nightly">Nightly</option>
        </select>
      </Row>
      <Row label="Check automatically" hint="Look for updates at startup and every few hours. Nothing installs without asking.">
        <Toggle checked={u.check} onChange={(v) => save(["updates", "check"], v)} />
      </Row>
      <Row label="Update URL" hint="Advanced: an update manifest URL to use instead of the built-in one. {{channel}} becomes stable or nightly.">
        <Field wide placeholder="built-in" value={u.endpoint ?? ""} onCommit={(v) => save(["updates", "endpoint"], v.trim() || undefined)} />
      </Row>
      <div className="set-actions">
        <button
          disabled={busy || st?.state === "unconfigured"}
          onClick={() => {
            setErr(null);
            checkForUpdates().catch((e) => setErr(String(e)));
          }}
        >
          Check now
        </button>
        {st?.state === "available" && (
          <button className="primary" disabled={busy} onClick={() => installUpdate().catch((e) => setErr(String(e)))}>
            Install {st.version} and restart
          </button>
        )}
        <span className="set-hint" role="status" aria-live="polite">
          {line}
        </span>
      </div>
      {st?.state === "available" && st.notes && <pre className="update-notes">{st.notes}</pre>}
      {st?.state === "available" && !persistent && (
        <div className="set-hint">Installing closes Hivetty; your layout is restored, but running shells end. Turn on "Keep shells running after closing" (Session) to keep them.</div>
      )}
      {err && <div className="set-problems"><div className="err">{err}</div></div>}
    </section>
  );
}

function Keyboard({ c, save }: { c: Config; save: Save }) {
  const [q, setQ] = useState("");
  const [recording, setRecording] = useState<string | null>(null);
  const commands = useStore((s) => s.commands);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const actions = useMemo(() => buildActions().filter((a) => !a.id.startsWith("profile.")), [commands]);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const bindings = useMemo(() => currentBindings(), [c.keybindings, c.commands]);
  const byAction = useMemo(() => {
    const m = new Map<string, string[]>();
    for (const [chord, a] of Object.entries(bindings)) m.set(a, [...(m.get(a) ?? []), chord]);
    return m;
  }, [bindings]);

  useEffect(() => {
    if (!recording) return;
    setSuspended(true);
    const onKey = (ev: KeyboardEvent) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (ev.key === "Escape") {
        setRecording(null);
        return;
      }
      const chord = chordOf(ev);
      if (!chord) return; // modifier only
      setRecording(null);
      void save(["keybindings", chord], recording);
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      setSuspended(false);
    };
  }, [recording, save]);

  const unbind = (chord: string) => {
    const user = Object.keys(c.keybindings).find((k) => normalizeChord(k) === chord);
    // a chord only the user added: delete the entry; a default chord: store null to unbind it
    if (user && !(chord in defaultBindings)) void save(["keybindings", user], undefined);
    else void save(["keybindings", user ?? chord], null);
  };

  const shown = actions.filter((a) => !q || a.title.toLowerCase().includes(q.toLowerCase()) || a.id.includes(q.toLowerCase()) || (byAction.get(a.id) ?? []).some((ch) => prettyChord(ch).toLowerCase().includes(q.toLowerCase())));
  return (
    <section>
      <h2>Keyboard shortcuts</h2>
      <div className="set-actions">
        <input className="wide" placeholder="Search actions or keys" value={q} onChange={(e) => setQ(e.target.value)} />
        {Object.keys(c.keybindings).length > 0 && <button onClick={() => save(["keybindings"], undefined)}>Reset all</button>}
      </div>
      <div className="keys">
        {shown.map((a) => (
          <div key={a.id} className="key-row">
            <span className="key-title">{a.title}</span>
            <span className="key-chords">
              {(byAction.get(a.id) ?? []).map((ch) => (
                <span key={ch} className="kbd">
                  {prettyChord(ch)}
                  <span className="kbd-x" title="Remove" onClick={() => unbind(ch)}>
                    ×
                  </span>
                </span>
              ))}
              <button className={`kbd-add ${recording === a.id ? "rec" : ""}`} onClick={() => setRecording(recording === a.id ? null : a.id)}>
                {recording === a.id ? "Press keys… (Esc cancels)" : "+"}
              </button>
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}

/** file:// URL of the project schema next to the user config. */
function schemaUrl(configPath: string) {
  return "file:///" + configPath.replace(/\\/g, "/").replace(/hivetty\.json$/, "hivetty-project.schema.json");
}

const TARGETS: [CommandTarget, string][] = [
  ["split-right", "Split right"],
  ["split-down", "Split down"],
  ["tab", "New tab"],
  ["workspace", "New workspace"],
  ["current", "Focused terminal"],
];

function Commands({ c, save }: { c: Config; save: Save }) {
  const project = useStore((s) => s.commands.filter((x) => x.source));
  const empty = { name: "", command: "", target: "split-right" as CommandTarget, cwd: "", keybinding: "" };
  const [draft, setDraft] = useState(empty);
  const add = () => {
    if (!draft.name.trim() || !draft.command.trim()) return;
    const cmd: CustomCommand = { name: draft.name.trim(), command: draft.command.trim(), target: draft.target };
    if (draft.cwd.trim()) cmd.cwd = draft.cwd.trim();
    if (draft.keybinding.trim()) cmd.keybinding = normalizeChord(draft.keybinding);
    void save(["commands"], [...c.commands, cmd]);
    setDraft(empty);
  };
  const sources = [...new Set(project.map((p) => p.source!))];
  return (
    <section>
      <h2>Your commands</h2>
      <div className="set-hint">Shown in the command palette (Ctrl+Shift+P) everywhere.</div>
      <div className="set-list">
        {c.commands.length === 0 && <div className="set-hint">None yet.</div>}
        {c.commands.map((cmd, i) => (
          <div key={`${cmd.name}-${i}`} className="set-item">
            <div>
              <div>
                {cmd.name} <span className="set-hint">· {TARGETS.find((t) => t[0] === cmd.target)?.[1]}</span>
                {cmd.keybinding && <span className="kbd small">{prettyChord(normalizeChord(cmd.keybinding))}</span>}
              </div>
              <div className="set-hint mono">{cmd.command}</div>
            </div>
            <button onClick={() => save(["commands"], c.commands.filter((_, j) => j !== i))}>Remove</button>
          </div>
        ))}
      </div>
      <div className="set-form">
        <input placeholder="Name" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
        <input className="grow" placeholder="Command (e.g. pnpm dev)" value={draft.command} spellCheck={false} onChange={(e) => setDraft({ ...draft, command: e.target.value })} />
        <select value={draft.target} onChange={(e) => setDraft({ ...draft, target: e.target.value as CommandTarget })}>
          {TARGETS.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </select>
      </div>
      <div className="set-form">
        <input className="grow" placeholder="Folder (optional)" value={draft.cwd} spellCheck={false} onChange={(e) => setDraft({ ...draft, cwd: e.target.value })} />
        <input placeholder="Shortcut, e.g. Ctrl+Alt+D" value={draft.keybinding} onChange={(e) => setDraft({ ...draft, keybinding: e.target.value })} onKeyDown={(e) => e.key === "Enter" && add()} />
        <button onClick={add} disabled={!draft.name.trim() || !draft.command.trim()}>
          Add
        </button>
      </div>
      <h2>Project commands</h2>
      <div className="set-hint">
        Add a <code>hivetty.json</code> (or <code>.hivetty.json</code>) to a repository. Hivetty reads it from the focused pane's folder and every folder above it:
      </div>
      <pre className="set-code">{`{
  "$schema": "${schemaUrl(useStore.getState().config?.path ?? "")}",
  "commands": [
    { "name": "Dev server", "command": "pnpm dev", "target": "split-right" },
    { "name": "Tests", "command": "cargo test", "target": "split-down", "cwd": "crates" }
  ]
}`}</pre>
      {sources.length === 0 ? (
        <div className="set-hint">No project commands for the focused pane.</div>
      ) : (
        sources.map((src) => (
          <div key={src} className="set-list">
            <div className="set-hint mono">{src}</div>
            {project
              .filter((p) => p.source === src)
              .map((p) => (
                <div key={p.name} className="set-item">
                  <div>
                    <div>{p.name}</div>
                    <div className="set-hint mono">{p.command}</div>
                  </div>
                </div>
              ))}
          </div>
        ))
      )}
    </section>
  );
}
