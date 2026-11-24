import { api } from "./api";
import { flushScrollback } from "./terminals";
import { useStore } from "../state/store";

export async function checkForUpdates() {
  const st = await api.updateCheck();
  useStore.getState().set({ update: st });
  return st;
}

/** Hand over the latest scrollback (so the session restores it), then download and install. */
export async function installUpdate() {
  const { set } = useStore.getState();
  set({ updateProgress: 0 });
  try {
    await flushScrollback(true);
    await api.updateInstall(({ downloaded, total }) => set({ updateProgress: total ? downloaded / total : null }));
  } catch (e) {
    set({ updateProgress: null });
    throw e;
  }
}
