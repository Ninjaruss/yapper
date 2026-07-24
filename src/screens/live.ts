import { ipc, type OutlineEntryUI, type Session } from "../ipc";
import { escapeHtml } from "../escape";
import { createWisp } from "../wisp";

const MAX_TRANSCRIPT_LINES = 40;
const VOICE_LEVEL_THRESHOLD = 0.02;
const FLOWING_WITHIN_MS = 1500;
const THINKING_AFTER_MS = 2500;
const ECHO_GLOW_MS = 4000;
const SHINE_UNDERLINE_MS = 4000;

export function renderLive(root: HTMLElement, onEnded: (session: Session) => void): void {
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
      <div id="outline"></div>
      <div class="label" id="wonderingLabel" style="margin-top:10px; display:none;">Wondering</div>
      <p id="wondering" style="font-style:italic; display:none;"></p>
    </div>
    <div class="paper-panel" style="margin-top:16px;">
      <div class="label">Transcript</div>
      <div id="transcript" style="max-height:50vh; overflow-y:auto; font-style:normal;"></div>
      <p id="sttState" class="paused-note" style="margin-bottom:0;"></p>
    </div>
  `;

  const elapsedEl = root.querySelector<HTMLElement>("#elapsed")!;
  const meterEl = root.querySelector<HTMLElement>("#meter")!;
  const stateEl = root.querySelector<HTMLElement>("#state")!;
  const pauseBtn = root.querySelector<HTMLButtonElement>("#pause")!;
  const outlineEl = root.querySelector<HTMLElement>("#outline")!;
  const wonderingLabelEl = root.querySelector<HTMLElement>("#wonderingLabel")!;
  const wonderingEl = root.querySelector<HTMLElement>("#wondering")!;
  const transcriptEl = root.querySelector<HTMLElement>("#transcript")!;
  const sttStateEl = root.querySelector<HTMLElement>("#sttState")!;

  const wisp = createWisp();
  root.querySelector(".paper-panel")!.appendChild(wisp.el);

  let paused = false;
  let ended = false;
  let lastVoiceAt = Date.now();
  const glowTimers = new Map<number, ReturnType<typeof setTimeout>>();

  let unlisten: (() => void) | null = null;
  ipc.onLevel((level) => {
    meterEl.style.width = `${Math.min(100, level * 300)}%`;
    if (level > VOICE_LEVEL_THRESHOLD) {
      lastVoiceAt = Date.now();
    }
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      unlisten = fn;
    }
  });

  let segmentUnlisten: (() => void) | null = null;
  ipc.onSegment((s) => {
    transcriptEl.insertAdjacentHTML(
      "beforeend",
      `<p data-segment-id="${s.id}" style="margin:6px 0;">${escapeHtml(s.text)}</p>`,
    );
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

  let signalUnlisten: (() => void) | null = null;
  ipc.onSignal((sig) => {
    wisp.marginNote(sig.note);
    if (sig.kind === "repetition") {
      wisp.setState("repeat");
      if (sig.echo_of_segment_id !== null) {
        const line = transcriptEl.querySelector<HTMLElement>(
          `[data-segment-id="${sig.echo_of_segment_id}"]`,
        );
        if (line) {
          line.classList.add("echo-glow");
          const existing = glowTimers.get(sig.echo_of_segment_id);
          if (existing !== undefined) clearTimeout(existing);
          const id = sig.echo_of_segment_id;
          glowTimers.set(
            id,
            setTimeout(() => {
              line.classList.remove("echo-glow");
              glowTimers.delete(id);
            }, ECHO_GLOW_MS),
          );
        }
      }
    } else {
      wisp.setState("hot");
    }
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      signalUnlisten = fn;
    }
  });

  // Outline rendering uses textContent (not innerHTML) throughout: labels
  // are LLM-derived and can echo raw spoken words, so they must never be
  // parsed as markup — same discipline as the transcript's escapeHtml.
  let latestOutline: OutlineEntryUI[] = [];
  let currentOutlineP: HTMLElement | null = null;
  function rebuildOutline(entries: OutlineEntryUI[]) {
    latestOutline = entries;
    outlineEl.innerHTML = "";
    currentOutlineP = null;
    for (const entry of entries) {
      const p = document.createElement("p");
      if (entry.status === "covered") {
        p.className = "outline-covered";
        p.textContent = entry.label;
      } else if (entry.status === "current") {
        p.className = "outline-current";
        p.textContent = `✎ ${entry.label}`;
        currentOutlineP = p;
      } else {
        p.className = "outline-intent";
        p.textContent = `◌ ${entry.label}`;
      }
      outlineEl.appendChild(p);
    }
  }

  let outlineUnlisten: (() => void) | null = null;
  ipc.onOutline((entries) => {
    rebuildOutline(entries);
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      outlineUnlisten = fn;
    }
  });

  let questionUnlisten: (() => void) | null = null;
  ipc.onQuestion((question) => {
    wonderingLabelEl.textContent = "Wondering";
    wonderingLabelEl.style.display = "";
    wonderingEl.textContent = question;
    wonderingEl.style.display = "";
    wisp.setState("wondering");
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      questionUnlisten = fn;
    }
  });

  let shineUnderlineTimer: ReturnType<typeof setTimeout> | undefined;
  let shineUnlisten: (() => void) | null = null;
  ipc.onShine(() => {
    wisp.setState("shine");
    if (currentOutlineP) {
      const el = currentOutlineP;
      el.classList.add("shine-underline");
      if (shineUnderlineTimer !== undefined) clearTimeout(shineUnderlineTimer);
      shineUnderlineTimer = setTimeout(() => {
        shineUnderlineTimer = undefined;
        el.classList.remove("shine-underline");
      }, SHINE_UNDERLINE_MS);
    }
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      shineUnlisten = fn;
    }
  });

  let wrapupUnlisten: (() => void) | null = null;
  ipc.onWrapup(() => {
    wisp.setState("wrapup");
    const worthCallingBack = latestOutline
      .filter((e) => e.status === "covered")
      .slice(0, 2)
      .map((e) => e.label)
      .join(" · ");
    wonderingLabelEl.textContent = "Worth calling back";
    wonderingLabelEl.style.display = "";
    wonderingEl.textContent = worthCallingBack;
    wonderingEl.style.display = "";
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      wrapupUnlisten = fn;
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
      sttStateEl.textContent = "transcription hit trouble earlier this session — audio still recording";
    } else {
      sttStateEl.textContent = "";
    }
    if (status.insight_failed) {
      sttStateEl.textContent = sttStateEl.textContent
        ? `${sttStateEl.textContent} · insight resting`
        : "insight resting";
    }
    if (!status.insight_active && latestOutline.length === 0 && outlineEl.children.length === 0) {
      const p = document.createElement("p");
      p.className = "outline-intent";
      p.textContent = "the thinking model is off — mirror only";
      outlineEl.appendChild(p);
    }

    if (!paused) {
      const idle = Date.now() - lastVoiceAt;
      if (idle < FLOWING_WITHIN_MS) {
        wisp.setState("flowing");
      } else if (idle >= THINKING_AFTER_MS) {
        wisp.setState("thinking");
      }
      // Between the two thresholds: leave the current state as-is.
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
        lastVoiceAt = Date.now();
        wisp.setState("flowing");
      } else {
        await ipc.pauseListening();
        pauseBtn.textContent = "Resume";
        stateEl.textContent = "asleep — hearing nothing";
        paused = true;
        wisp.setState("sleep");
      }
    } catch (e) {
      stateEl.textContent = String(e);
    } finally {
      pauseBtn.disabled = false;
    }
  };

  root.querySelector<HTMLButtonElement>("#end")!.onclick = async () => {
    let endedSession: Session;
    try {
      endedSession = await ipc.endSession();
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
    signalUnlisten?.();
    outlineUnlisten?.();
    questionUnlisten?.();
    shineUnlisten?.();
    wrapupUnlisten?.();
    if (shineUnderlineTimer !== undefined) clearTimeout(shineUnderlineTimer);
    for (const t of glowTimers.values()) clearTimeout(t);
    glowTimers.clear();
    wisp.destroy();
    onEnded(endedSession);
  };
}
