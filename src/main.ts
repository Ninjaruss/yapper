import "./styles.css";
import { renderSetup } from "./screens/setup";
import { renderLive } from "./screens/live";
import { ipc } from "./ipc";

const root = document.getElementById("app")!;

function showSetup() {
  renderSetup(root, showLive);
}

async function showLive() {
  renderLive(root, async () => {
    // Minimal end-of-take acknowledgment; real recap arrives in Plan 4.
    const sessions = await ipc.listSessions();
    const last = sessions[0];
    const mins = last?.duration_ms ? Math.round(last.duration_ms / 60000) : 0;
    root.innerHTML = `
      <div class="paper-panel">
        <div class="label">Talk saved</div>
        <p>~${mins} min · audio at <code>${last?.audio_path ?? "?"}</code></p>
        <button id="again">Back to the desk</button>
      </div>`;
    root.querySelector<HTMLButtonElement>("#again")!.onclick = showSetup;
  });
}

showSetup();
