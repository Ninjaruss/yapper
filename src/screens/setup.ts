import { ipc, type InputDevice, type Session } from "../ipc";
import { escapeHtml } from "../escape";
import { fmtDate, fmtDuration } from "../format";

const REFRESH_MS = 4000;

// Guards against re-kicking the download every time the setup screen
// re-renders (e.g. returning here after ending a talk) while a previous
// ensureModels() call from earlier in this app session is still in flight.
// Reset to false on a failed download so a later visit can retry.
let modelDownloadStarted = false;

// Module-scope so a re-render of this screen can unsubscribe whatever
// listener a previous renderSetup() call registered, even though that call's
// own local variable is no longer reachable.
let modelProgressUnlisten: (() => void) | null = null;

export function renderSetup(
  root: HTMLElement,
  onStarted: () => void,
): void {
  // Defensive: drop any listener left over from a previous renderSetup()
  // call before registering a new one below, so listeners never accumulate
  // across re-renders.
  modelProgressUnlisten?.();
  modelProgressUnlisten = null;

  root.innerHTML = `
    <h1>Yapper</h1>
    <div class="paper-panel">
      <div class="label">Microphone</div>
      <select id="mic"></select>
      <div class="level-meter" style="margin-top:10px"><div id="meter"></div></div>
      <div class="label" style="margin-top:18px">Intent — a title, or paste your whole notes</div>
      <textarea id="intent" rows="4" placeholder="what do you want to talk about?"></textarea>
      <div style="margin-top:16px; display:flex; gap:10px;">
        <button id="start">Begin the talk</button>
      </div>
      <p id="error" class="paused-note" role="alert"></p>
    </div>
    <div id="modelBanner"></div>
    <div id="past" style="margin-top:22px;"></div>
  `;

  const mic = root.querySelector<HTMLSelectElement>("#mic")!;
  const errorEl = root.querySelector<HTMLParagraphElement>("#error")!;
  const pastEl = root.querySelector<HTMLElement>("#past")!;
  const bannerEl = root.querySelector<HTMLElement>("#modelBanner")!;

  // Same ended-flag pattern as live.ts's onLevel: if cleanup() runs before
  // the listen() promise resolves, unlisten immediately on arrival instead
  // of leaking the subscription. (modelProgressUnlisten itself is the
  // module-scope variable above.)
  let modelBannerDone = false;
  const stopModelProgressListener = () => {
    modelBannerDone = true;
    modelProgressUnlisten?.();
    modelProgressUnlisten = null;
  };

  async function initModelBanner(): Promise<void> {
    let ready: boolean;
    try {
      ready = await ipc.modelsReady();
    } catch {
      ready = false; // fail-safe: don't block the desk on a status-check error
    }
    if (ready) return;

    bannerEl.innerHTML = `
      <style>
        @keyframes yapper-model-pulse { 0%, 100% { opacity: 0.35; } 50% { opacity: 0.85; } }
      </style>
      <div class="paper-panel" style="margin-top:22px;">
        <div class="label">First run</div>
        <p id="modelText" class="paused-note" style="margin-bottom:8px;">downloading the listening model (~250 MB, one time) — you can record meanwhile; transcription joins when it's ready</p>
        <div class="level-meter"><div id="modelBar"></div></div>
      </div>
    `;
    const textEl = bannerEl.querySelector<HTMLElement>("#modelText")!;
    const barEl = bannerEl.querySelector<HTMLElement>("#modelBar")!;

    ipc.onModelProgress((p) => {
      if (p.total === 0) {
        // No Content-Length from the server: indeterminate progress —
        // full-width bar, pulsing at reduced opacity.
        barEl.style.width = "100%";
        barEl.style.opacity = "0.5";
        barEl.style.animation = "yapper-model-pulse 1.2s ease-in-out infinite";
      } else {
        barEl.style.animation = "none";
        barEl.style.opacity = "1";
        barEl.style.width = `${Math.min(100, (p.downloaded / p.total) * 100)}%`;
      }
    }).then((fn) => {
      if (modelBannerDone) {
        fn();
      } else {
        modelProgressUnlisten = fn;
      }
    });

    if (!modelDownloadStarted) {
      modelDownloadStarted = true;
      ipc.ensureModels()
        .then(() => {
          stopModelProgressListener();
          textEl.textContent = "listening model ready";
          barEl.style.animation = "none";
          barEl.style.opacity = "1";
          barEl.style.width = "100%";
          setTimeout(() => {
            bannerEl.innerHTML = "";
          }, 3000);
        })
        .catch((e) => {
          stopModelProgressListener();
          // Allow a later visit to this screen to retry the download —
          // without this, a failed download could never be retried short
          // of an app restart.
          modelDownloadStarted = false;
          textEl.textContent = `couldn't download the listening model — ${String(e)} — recording still works, transcription won't join until it's downloaded`;
        });
    }
  }
  initModelBanner();

  // Both lists refresh while this screen is visible: devices come and go
  // (bluetooth mics), recordings get deleted in Finder. Selection and
  // scroll are preserved; DOM is only touched when content actually changed.
  let deviceKey = "";
  async function refreshDevices(): Promise<void> {
    const devices: InputDevice[] = await ipc.listInputDevices();
    const key = devices.map((d) => `${d.name}${d.is_default ? "*" : ""}`).join("|");
    if (key === deviceKey) return;
    deviceKey = key;
    const previous = mic.value;
    mic.innerHTML = devices
      .map((d) => `<option value="${escapeHtml(d.name)}" ${d.is_default ? "selected" : ""}>${escapeHtml(d.name)}</option>`)
      .join("");
    if (previous && devices.some((d) => d.name === previous)) {
      mic.value = previous;
    }
  }

  let pastKey = "";
  async function refreshPast(): Promise<void> {
    const sessions: Session[] = await ipc.listSessions();
    const key = sessions
      .map((s) => `${s.id}:${s.duration_ms}:${s.audio_exists}`)
      .join("|");
    if (key === pastKey) return;
    pastKey = key;
    if (sessions.length === 0) {
      pastEl.innerHTML = "";
      return;
    }
    pastEl.innerHTML = `
      <div class="label" style="margin-bottom:8px;">Past talks</div>
      ${sessions
        .map((s) => {
          const dur = s.duration_ms != null ? fmtDuration(s.duration_ms) : "interrupted";
          const intent = s.intent.trim().split("\n")[0].slice(0, 60);
          const fileUi = s.audio_exists
            ? `<button class="quiet reveal" data-id="${s.id}" style="color:var(--ink); border-color:var(--ink-soft); padding:6px 12px; font-size:0.85rem;">Show file</button>`
            : `<span style="font-style:italic; color:var(--ember); font-size:0.85rem;">file missing</span>
               <button class="quiet forget" data-id="${s.id}" style="color:var(--ink); border-color:var(--ink-soft); padding:6px 12px; font-size:0.85rem;">Forget</button>`;
          return `
            <div class="paper-panel" style="display:flex; align-items:center; gap:14px; padding:10px 16px; margin-bottom:8px;">
              <span style="font-family:var(--mono); color:var(--ink-soft); min-width:110px;">${fmtDate(s.started_at_ms)}</span>
              <span style="font-family:var(--mono); min-width:56px;">${dur}</span>
              <span style="flex:1; font-style:italic; color:var(--ink-soft); overflow:hidden; text-overflow:ellipsis; white-space:nowrap;">${escapeHtml(intent)}</span>
              ${fileUi}
            </div>`;
        })
        .join("")}
    `;
    pastEl.querySelectorAll<HTMLButtonElement>("button.reveal").forEach((btn) => {
      btn.onclick = () =>
        ipc.revealSession(Number(btn.dataset.id)).catch((e) => {
          errorEl.textContent = String(e);
          void refreshPast(); // file may have vanished since last poll
        });
    });
    pastEl.querySelectorAll<HTMLButtonElement>("button.forget").forEach((btn) => {
      btn.onclick = () =>
        ipc.forgetSession(Number(btn.dataset.id))
          .then(() => refreshPast())
          .catch((e) => { errorEl.textContent = String(e); });
    });
  }

  const refreshAll = () => {
    refreshDevices().catch((e) => { errorEl.textContent = String(e); });
    refreshPast().catch((e) => { errorEl.textContent = String(e); });
  };
  refreshAll();
  const timer = setInterval(refreshAll, REFRESH_MS);
  window.addEventListener("focus", refreshAll);
  const cleanup = () => {
    clearInterval(timer);
    window.removeEventListener("focus", refreshAll);
    stopModelProgressListener();
  };

  root.querySelector<HTMLButtonElement>("#start")!.onclick = async () => {
    const intent = root.querySelector<HTMLTextAreaElement>("#intent")!.value;
    try {
      await ipc.startSession(intent, mic.value || undefined);
      cleanup();
      onStarted();
    } catch (e) {
      errorEl.textContent = String(e);
    }
  };
}
