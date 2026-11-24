import { useEffect, useLayoutEffect, useRef } from "react";
import type { Pane } from "../types";
import { api } from "../lib/api";
import { fitTerminal, focusTerminal, mountTerminal } from "../lib/terminals";
import { useStore } from "../state/store";

export function TerminalPane({ pane, focused, multi }: { pane: Pane; focused: boolean; multi: boolean }) {
  const slot = useRef<HTMLDivElement>(null);
  const flash = useStore((s) => s.flashPane === pane.id);
  const overlay = useStore((s) => s.overlay);
  const status = useStore((s) => s.paneStatus[pane.id]);

  useLayoutEffect(() => {
    if (!slot.current) return;
    mountTerminal(pane.id, slot.current);
    const ro = new ResizeObserver(() => fitTerminal(pane.id));
    ro.observe(slot.current);
    return () => ro.disconnect();
  }, [pane.id]);

  useEffect(() => {
    if (focused && !overlay) focusTerminal(pane.id);
  }, [focused, overlay, pane.id]);

  const cls = ["pane", "terminal-pane", focused && multi ? "focused" : "", pane.unread ? "unread" : "", flash ? "flash" : ""].join(" ");

  return (
    <div
      className={cls}
      data-pane-id={pane.id}
      onMouseDown={() => {
        if (!focused) api.focusPane(pane.id);
      }}
    >
      <div ref={slot} className="xterm-slot" />
      {status && (
        <div className={`pane-status ${status.kind}`} role="status">
          {status.kind === "busy" && <span className="spinner" />}
          {status.text}
        </div>
      )}
      {pane.resumable && pane.agent && !pane.exited && (
        <div className="resume-banner" role="status">
          <span>
            Previous {pane.agent.agent === "claude" ? "Claude Code" : pane.agent.agent === "codex" ? "Codex" : pane.agent.agent} session
          </span>
          <button
            onClick={() => {
              api.resumeAgent(pane.id);
              focusTerminal(pane.id);
            }}
          >
            Resume
          </button>
          <button onClick={() => api.dismissResume(pane.id)}>Dismiss</button>
        </div>
      )}
      {pane.exited && (
        <div className="exited-banner">
          <span>
            Process exited{pane.last_exit !== null ? ` with code ${pane.last_exit}` : ""}.
          </span>
          <button onClick={() => api.restartPane(pane.id)}>Restart</button>
          <button onClick={() => api.closePane(pane.id)}>Close</button>
        </div>
      )}
    </div>
  );
}
