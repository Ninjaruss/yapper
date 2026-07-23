import { ipc, type InputDevice } from "../ipc";
import { escapeHtml } from "../escape";

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
  `;

  const mic = root.querySelector<HTMLSelectElement>("#mic")!;
  const errorEl = root.querySelector<HTMLParagraphElement>("#error")!;

  ipc.listInputDevices().then((devices: InputDevice[]) => {
    mic.innerHTML = devices
      .map((d) => `<option value="${escapeHtml(d.name)}" ${d.is_default ? "selected" : ""}>${escapeHtml(d.name)}</option>`)
      .join("");
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
