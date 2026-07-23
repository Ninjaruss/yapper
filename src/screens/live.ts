import { ipc } from "../ipc";

export function renderLive(root: HTMLElement, onEnded: () => void): void {
  root.innerHTML = `
    <div class="paper-panel" style="display:flex; align-items:center; gap:24px;">
      <div class="elapsed" id="elapsed">0:00</div>
      <div class="level-meter" style="flex:1"><div id="meter"></div></div>
      <button id="pause" class="quiet">Pause listening</button>
      <button id="end">End the talk</button>
    </div>
    <p id="state" class="paused-note" role="status"></p>
  `;

  const elapsedEl = root.querySelector<HTMLElement>("#elapsed")!;
  const meterEl = root.querySelector<HTMLElement>("#meter")!;
  const stateEl = root.querySelector<HTMLElement>("#state")!;
  const pauseBtn = root.querySelector<HTMLButtonElement>("#pause")!;

  let paused = false;
  let ended = false;
  let unlisten: (() => void) | null = null;
  ipc.onLevel((level) => {
    meterEl.style.width = `${Math.min(100, level * 300)}%`;
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      unlisten = fn;
    }
  });

  const timer = setInterval(async () => {
    const status = await ipc.sessionStatus();
    if (!status) return;
    const total = Math.floor(status.elapsed_ms / 1000);
    elapsedEl.textContent = `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
    if (status.writer_failed) {
      stateEl.textContent = "trouble writing audio to disk — end the talk to keep what's saved";
    }
  }, 500);

  pauseBtn.onclick = async () => {
    pauseBtn.disabled = true;
    try {
      if (paused) {
        await ipc.resumeListening();
        pauseBtn.textContent = "Pause listening";
        stateEl.textContent = "";
        paused = false;
      } else {
        await ipc.pauseListening();
        pauseBtn.textContent = "Resume";
        stateEl.textContent = "asleep — hearing nothing";
        paused = true;
      }
    } catch (e) {
      stateEl.textContent = String(e);
    } finally {
      pauseBtn.disabled = false;
    }
  };

  root.querySelector<HTMLButtonElement>("#end")!.onclick = async () => {
    try {
      await ipc.endSession();
    } catch (e) {
      stateEl.textContent = String(e);
      return; // screen stays live; timer/meter keep running
    }
    ended = true;
    clearInterval(timer);
    unlisten?.();
    onEnded();
  };
}
