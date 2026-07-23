import { ipc } from "../ipc";
import { escapeHtml } from "../escape";

const MAX_TRANSCRIPT_LINES = 40;

export function renderLive(root: HTMLElement, onEnded: () => void): void {
  root.innerHTML = `
    <div class="paper-panel" style="display:flex; align-items:center; gap:24px;">
      <div class="elapsed" id="elapsed">0:00</div>
      <div class="level-meter" style="flex:1"><div id="meter"></div></div>
      <button id="pause" class="quiet">Pause listening</button>
      <button id="end">End the talk</button>
    </div>
    <p id="state" class="paused-note" role="status"></p>
    <div class="paper-panel" style="margin-top:16px;">
      <div class="label">So far</div>
      <div id="transcript" style="max-height:50vh; overflow-y:auto; font-style:normal;"></div>
      <p id="sttState" class="paused-note" style="margin-bottom:0;"></p>
    </div>
  `;

  const elapsedEl = root.querySelector<HTMLElement>("#elapsed")!;
  const meterEl = root.querySelector<HTMLElement>("#meter")!;
  const stateEl = root.querySelector<HTMLElement>("#state")!;
  const pauseBtn = root.querySelector<HTMLButtonElement>("#pause")!;
  const transcriptEl = root.querySelector<HTMLElement>("#transcript")!;
  const sttStateEl = root.querySelector<HTMLElement>("#sttState")!;

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

  let segmentUnlisten: (() => void) | null = null;
  ipc.onSegment((s) => {
    transcriptEl.insertAdjacentHTML("beforeend", `<p style="margin:6px 0;">${escapeHtml(s.text)}</p>`);
    while (transcriptEl.children.length > MAX_TRANSCRIPT_LINES) {
      transcriptEl.removeChild(transcriptEl.firstElementChild!);
    }
    transcriptEl.scrollTop = transcriptEl.scrollHeight;
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      segmentUnlisten = fn;
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
    if (!status.stt_active) {
      sttStateEl.textContent = "transcribing is off (model not ready) — audio still records";
    } else if (status.stt_failed) {
      sttStateEl.textContent = "transcription hit trouble — audio still recording";
    } else {
      sttStateEl.textContent = "";
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
      // Deliberately not cleaning up here: endSession() failed, so the
      // screen stays live and the session is presumably still active —
      // the status-poll timer, level meter, and segment listener must all
      // keep running so the user can retry End and the transcript keeps
      // filling in the meantime.
      return;
    }
    ended = true;
    clearInterval(timer);
    unlisten?.();
    segmentUnlisten?.();
    onEnded();
  };
}
