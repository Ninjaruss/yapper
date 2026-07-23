import { ipc, type InputDevice, type Session } from "../ipc";
import { escapeHtml } from "../escape";
import { fmtDate, fmtDuration } from "../format";

const REFRESH_MS = 4000;

export function renderSetup(
  root: HTMLElement,
  onStarted: () => void,
): void {
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
    <div id="past" style="margin-top:22px;"></div>
  `;

  const mic = root.querySelector<HTMLSelectElement>("#mic")!;
  const errorEl = root.querySelector<HTMLParagraphElement>("#error")!;
  const pastEl = root.querySelector<HTMLElement>("#past")!;

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
