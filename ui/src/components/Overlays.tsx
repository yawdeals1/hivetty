import { useEffect, useMemo, useRef, useState } from "react";
import { buildActions, fuzzyScore, refreshCommands } from "../lib/actions";
import { chordsFor, prettyChord } from "../lib/keymap";
import { api } from "../lib/api";
import { activeWorkspace, focusedPane, useStore } from "../state/store";
import { focusTerminal, terminalEntry } from "../lib/terminals";

export const closeOverlay = () => {
  const s = useStore.getState();
  s.set({ overlay: null });
  const p = focusedPane(s.model);
  if (p) setTimeout(() => focusTerminal(p.id), 0);
};

const close = closeOverlay;

export function CommandPalette() {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const commands = useStore((s) => s.commands);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const actions = useMemo(() => buildActions(), [commands]);
  useEffect(() => {
    void refreshCommands();
  }, []);
  const results = useMemo(() => {
    return actions
      .map((a) => ({ a, s: Math.max(fuzzyScore(q, a.title), a.detail ? fuzzyScore(q, a.detail) - 5 : -1) }))
      .filter((r) => r.s >= 0)
      .sort((x, y) => y.s - x.s)
      .slice(0, 50)
      .map((r) => r.a);
  }, [q, actions]);

  const run = (i: number) => {
    const a = results[i];
    close();
    if (a) setTimeout(() => a.run(), 0);
  };

  return (
    <div className="overlay-backdrop" onMouseDown={close}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <input
          autoFocus
          placeholder="Type a command…"
          value={q}
          onChange={(e) => {
            setQ(e.target.value);
            setSel(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") close();
            else if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(results.length - 1, s + 1)); }
            else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(0, s - 1)); }
            else if (e.key === "Enter") run(sel);
          }}
        />
        <div className="palette-list">
          {results.map((a, i) => (
            <div key={a.id} className={`palette-item ${i === sel ? "sel" : ""}`} onMouseEnter={() => setSel(i)} onClick={() => run(i)}>
              <span className="palette-title">
                {a.title}
                {a.detail && <span className="palette-detail">{a.detail}</span>}
              </span>
              <span className="chords">{chordsFor(a.id).slice(0, 1).map(prettyChord).join(" ")}</span>
            </div>
          ))}
          {results.length === 0 && <div className="palette-empty">No matching commands</div>}
        </div>
      </div>
    </div>
  );
}

export function RenameDialog() {
  const ws = useStore((s) => activeWorkspace(s.model));
  if (!ws) return null;
  return (
    <div className="overlay-backdrop" onMouseDown={close}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <input
          autoFocus
          defaultValue={ws.name}
          onFocus={(e) => e.currentTarget.select()}
          onKeyDown={(e) => {
            if (e.key === "Escape") close();
            if (e.key === "Enter") {
              api.updateWorkspace(ws.id, { name: e.currentTarget.value });
              close();
            }
          }}
        />
        <div className="palette-hint">Rename workspace — Enter to save, Esc to cancel</div>
      </div>
    </div>
  );
}

export function FindBar() {
  const pane = useStore((s) => focusedPane(s.model));
  const [q, setQ] = useState("");
  const [opts, setOpts] = useState({ caseSensitive: false, regex: false, wholeWord: false });
  const [count, setCount] = useState<{ index: number; total: number } | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const entry = pane ? terminalEntry(pane.id) : undefined;
  const deco = {
    matchBackground: "#f5b53d55",
    activeMatchBackground: "#f5b53d",
    matchOverviewRuler: "#f5b53d",
    activeMatchColorOverviewRuler: "#ffffff",
  };
  useEffect(() => {
    input.current?.focus();
    const sub = entry?.search.onDidChangeResults((r) => setCount(r.resultCount > 0 ? { index: r.resultIndex, total: r.resultCount } : { index: -1, total: 0 }));
    return () => {
      sub?.dispose();
      entry?.search.clearDecorations();
    };
  }, [entry]);
  // re-run when an option changes
  useEffect(() => {
    if (q) find(1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts]);
  const find = (dir: 1 | -1, query = q) => {
    if (!entry) return;
    if (!query) {
      entry.search.clearDecorations();
      setCount(null);
      return;
    }
    const o = { ...opts, decorations: deco, incremental: dir === 1 };
    if (dir === 1) entry.search.findNext(query, o);
    else entry.search.findPrevious(query, o);
  };
  return (
    <div className="findbar">
      <input
        ref={input}
        placeholder="Find"
        value={q}
        onChange={(e) => {
          setQ(e.target.value);
          find(1, e.target.value);
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") close();
          if (e.key === "Enter") find(e.shiftKey ? -1 : 1);
        }}
      />
      <button className={opts.caseSensitive ? "on" : ""} title="Match case" onClick={() => setOpts((o) => ({ ...o, caseSensitive: !o.caseSensitive }))}>Aa</button>
      <button className={opts.wholeWord ? "on" : ""} title="Whole word" onClick={() => setOpts((o) => ({ ...o, wholeWord: !o.wholeWord }))}>ab</button>
      <button className={opts.regex ? "on" : ""} title="Regex" onClick={() => setOpts((o) => ({ ...o, regex: !o.regex }))}>.*</button>
      <span className="find-count">{count === null ? "" : count.total === 0 ? "No results" : `${count.index + 1} of ${count.total}`}</span>
      <button title="Previous (Shift+Enter)" onClick={() => find(-1)}>↑</button>
      <button title="Next (Enter)" onClick={() => find(1)}>↓</button>
      <button title="Close (Esc)" onClick={close}>×</button>
    </div>
  );
}
