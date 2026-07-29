// Self-update wrapper over Tauri's updater + process plugins. Everything is
// best-effort: a check that fails (offline, non-Tauri dev/harness, no manifest
// yet) resolves to "no update" rather than throwing, so the desk never breaks
// over an update check.

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";

const AUTOCHECK_KEY = "yapper.autoUpdate";

/** Auto-check on launch — on by default; the user can turn it off in Settings. */
export function autoUpdateEnabled(): boolean {
  return localStorage.getItem(AUTOCHECK_KEY) !== "off";
}
export function setAutoUpdate(on: boolean): void {
  localStorage.setItem(AUTOCHECK_KEY, on ? "on" : "off");
}

/** The running app's version (e.g. "0.1.0"), or "" if unavailable. */
export async function currentVersion(): Promise<string> {
  try {
    return await getVersion();
  } catch {
    return "";
  }
}

/** Resolves to the pending Update if a newer signed release exists, else null.
 * Never throws — any failure (offline, no manifest, not running under Tauri)
 * is treated as "nothing to update to". */
export async function checkForUpdate(): Promise<Update | null> {
  try {
    return await check();
  } catch {
    return null;
  }
}

/** Download + install the update, then relaunch into the new version. Progress
 * callback receives downloaded/total bytes as they arrive. */
export async function installAndRelaunch(
  update: Update,
  onProgress?: (downloaded: number, total: number | null) => void,
): Promise<void> {
  let downloaded = 0;
  let total: number | null = null;
  await update.downloadAndInstall((event) => {
    if (event.event === "Started") {
      total = event.data.contentLength ?? null;
    } else if (event.event === "Progress") {
      downloaded += event.data.chunkLength;
    }
    onProgress?.(downloaded, total);
  });
  await relaunch();
}
