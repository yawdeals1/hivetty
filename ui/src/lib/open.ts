import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { api } from "./api";

/** http(s) links open in a browser pane beside the current one; everything else goes to the OS. */
export function openUrl(uri: string, opts: { external?: boolean } = {}) {
  if (!opts.external && /^https?:\/\//i.test(uri)) {
    api.browserOpen(uri).catch(() => openExternal(uri));
    return;
  }
  openExternal(uri).catch((e) => console.error("open failed", uri, e));
}

export function openPort(port: number) {
  openUrl(`http://localhost:${port}`);
}
