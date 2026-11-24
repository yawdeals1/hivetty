import { useRef, useState } from "react";
import type { Id, LayoutNode, Surface } from "../types";
import { api } from "../lib/api";
import { TerminalPane } from "./TerminalPane";
import { BrowserPane } from "./BrowserPane";
import { useStore } from "../state/store";

export function SurfaceView({ surface }: { surface: Surface }) {
  if (surface.zoomed) {
    return (
      <div className="pane-tree">
        <Leaf pane={surface.zoomed} surface={surface} />
      </div>
    );
  }
  return (
    <div className="pane-tree">
      <NodeView node={surface.root} surface={surface} path={[]} />
    </div>
  );
}

function NodeView({ node, surface, path }: { node: LayoutNode; surface: Surface; path: number[] }) {
  if (node.type === "leaf") return <Leaf pane={node.pane} surface={surface} />;
  return <SplitView node={node} surface={surface} path={path} />;
}

function SplitView({ node, surface, path }: { node: Extract<LayoutNode, { type: "split" }>; surface: Surface; path: number[] }) {
  const ref = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<number | null>(null);
  const ratio = drag ?? node.ratio;
  const row = node.axis === "row";

  const onDown = (ev: React.PointerEvent) => {
    ev.preventDefault();
    const el = ref.current;
    if (!el) return;
    (ev.target as HTMLElement).setPointerCapture(ev.pointerId);
    const rect = el.getBoundingClientRect();
    let last = node.ratio;
    const move = (e: PointerEvent) => {
      const r = row ? (e.clientX - rect.left) / rect.width : (e.clientY - rect.top) / rect.height;
      last = Math.min(0.95, Math.max(0.05, r));
      setDrag(last);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      api.setRatio(surface.id, path, last).finally(() => setDrag(null));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div ref={ref} className={`split ${row ? "row" : "column"}`}>
      <div className="split-child" style={{ flexBasis: `${ratio * 100}%` }}>
        <NodeView node={node.a} surface={surface} path={[...path, 0]} />
      </div>
      <div className={`split-handle ${drag !== null ? "dragging" : ""}`} onPointerDown={onDown} onDoubleClick={() => api.setRatio(surface.id, path, 0.5)} />
      <div className="split-child" style={{ flexBasis: `${(1 - ratio) * 100}%` }}>
        <NodeView node={node.b} surface={surface} path={[...path, 1]} />
      </div>
    </div>
  );
}

function Leaf({ pane, surface }: { pane: Id; surface: Surface }) {
  const p = useStore((s) => s.model.panes[pane]);
  if (!p) return null;
  if (p.kind.type === "terminal") return <TerminalPane pane={p} focused={surface.focused === pane} multi={surface.root.type === "split"} />;
  return <BrowserPane pane={p} focused={surface.focused === pane} multi={surface.root.type === "split"} />;
}
