import { ipc, type InputDevice, type Session } from "../ipc";
import { escapeHtml } from "../escape";
import { fmtDate, fmtDuration } from "../format";

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

  ipc.listInputDevices().then((devices: InputDevice[]) => {
    mic.innerHTML = devices
      .map((d) => `<option value="${escapeHtml(d.name)}" ${d.is_default ? "selected" : ""}>${escapeHtml(d.name)}</option>`)
      .join("");
  }).catch((e) => { errorEl.textContent = String(e); });

  const pastEl = root.querySelector<HTMLElement>("#past")!;
  ipc.listSessions().then((sessions: Session[]) => {
    if (sessions.length === 0) return;
    pastEl.innerHTML = `
      <div class="label" style="margin-bottom:8px;">Past talks</div>
      ${sessions
        .map((s) => {
          const dur = s.duration_ms != null ? fmtDuration(s.duration_ms) : "interrupted";
          const intent = s.intent.trim().split("\n")[0].slice(0, 60);
          return `
            <div class="paper-panel" style="display:flex; align-items:center; gap:14px; padding:10px 16px; margin-bottom:8px;">
              <span style="font-family:var(--mono); color:var(--ink-soft); min-width:110px;">${fmtDate(s.started_at_ms)}</span>
              <span style="font-family:var(--mono); min-width:56px;">${dur}</span>
              <span style="flex:1; font-style:italic; color:var(--ink-soft); overflow:hidden; text-overflow:ellipsis; white-space:nowrap;">${escapeHtml(intent)}</span>
              ${s.audio_path ? `<button class="quiet reveal" data-id="${s.id}" style="color:var(--ink); border-color:var(--ink-soft); padding:6px 12px; font-size:0.85rem;">Show file</button>` : ""}
            </div>`;
        })
        .join("")}
    `;
    pastEl.querySelectorAll<HTMLButtonElement>("button.reveal").forEach((btn) => {
      btn.onclick = () =>
        ipc.revealSession(Number(btn.dataset.id)).catch((e) => {
          errorEl.textContent = String(e);
        });
    });
  }).catch((e) => { errorEl.textContent = String(e); });

  root.querySelector<HTMLButtonElement>("#start")!.onclick = async () => {
    const intent = root.querySelector<HTMLTextAreaElement>("#intent")!.value;
    try {
      await ipc.startSession(intent, mic.value || undefined);
      onStarted();
    } catch (e) {
      errorEl.textContent = String(e);
    }
  };
}
