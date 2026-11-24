import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { installUpdate } from "../lib/updates";
import { useStore } from "../state/store";

/** "Update available" strip above the tabs. Dismissing hides it until the next version. */
export function UpdateBanner() {
  const st = useStore((s) => s.update);
  const progress = useStore((s) => s.updateProgress);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    api.updateStatus().then((s) => useStore.getState().set({ update: s }));
  }, []);

  const working = progress !== null || st?.state === "downloading" || st?.state === "installing";
  if (!st || (st.state !== "available" && !working) || (!working && dismissed === st.version)) return null;

  return (
    <div className="config-banner update-banner" role="status" aria-live="polite">
      <span>
        {working ? (
          <>
            <b>Updating to Hivetty {st.version}</b>
            {progress !== null && progress > 0 ? ` — ${Math.round(progress * 100)}%` : "…"}
          </>
        ) : (
          <>
            <b>Hivetty {st.version} is available</b> ({st.channel}; you have {st.current}).
          </>
        )}
        {err && <span className="err"> {err}</span>}
      </span>
      {!working && (
        <>
          <button className="primary" onClick={() => installUpdate().catch((e) => setErr(String(e)))}>
            Install and restart
          </button>
          <button onClick={() => useStore.getState().set({ overlay: "settings" })}>Details</button>
          <button onClick={() => setDismissed(st.version)}>Later</button>
        </>
      )}
    </div>
  );
}
