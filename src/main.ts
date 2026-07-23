import "./styles.css";
import "./wisp.css";
import { renderSetup } from "./screens/setup";
import { renderLive } from "./screens/live";
import { ipc } from "./ipc";
import { fmtDuration } from "./format";

const root = document.getElementById("app")!;

function showSetup() {
  renderSetup(root, showLive);
}

async function showLive() {
  renderLive(root, async () => {
    // Minimal end-of-take acknowledgment; real recap arrives in Plan 4.
    const sessions = await ipc.listSessions();
    const last = sessions[0];
    const dur = last?.duration_ms != null ? fmtDuration(last.duration_ms) : "?";
    root.innerHTML = `
      <div class="paper-panel">
        <div class="label">Talk saved</div>
        <p>${dur} · <code>${last?.audio_path ?? "?"}</code></p>
        <div style="display:flex; gap:10px;">
          ${last?.audio_path ? `<button id="reveal" class="quiet" style="color:var(--ink); border-color:var(--ink-soft);">Show file</button>` : ""}
          <button id="again">Back to the desk</button>
        </div>
      </div>`;
    if (last?.audio_path) {
      root.querySelector<HTMLButtonElement>("#reveal")!.onclick = () => {
        ipc.revealSession(last.id).catch(() => {});
      };
    }
    root.querySelector<HTMLButtonElement>("#again")!.onclick = showSetup;
  });
}

showSetup();
