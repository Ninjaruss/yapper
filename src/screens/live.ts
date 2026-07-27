import { ipc, type OutlineEntryUI, type Session } from "../ipc";
import { createWisp } from "../wisp";
import { sinkGhosts, updateOutline } from "../outline";
import { eventMatchesCombo, isTypingTarget, loadKeybinds } from "../keys";
import { makePauseMark, needsPauseMark, renderSegmentLine } from "../transcript";

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
      <span id="chapter" class="chapter-title"></span>
      <div class="level-meter" style="flex:1"><div id="meter"></div></div>
      <button id="pause" class="quiet">Pause listening</button>
      <button id="end">End the talk</button>
    </div>
    <p id="state" class="paused-note" role="status"></p>
    <div class="live-grid">
      <div class="live-main">
        <div class="paper-panel">
          <div class="label">So far</div>
          <div id="outline"></div>
          <div class="label" id="wonderingLabel" style="margin-top:12px; display:none;">Wondering</div>
          <p id="wondering" class="wondering-chip" style="display:none;"></p>
        </div>
        <div class="paper-panel transcript-panel" style="margin-top:16px;">
          <div class="label">Transcript</div>
          <div id="transcript" style="max-height:44vh; overflow-y:auto; font-style:normal;"></div>
          <p id="sttState" class="paused-note" style="margin-bottom:0;"></p>
        </div>
      </div>
      <aside class="wisp-rail" id="wispRail" aria-label="companion"></aside>
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
  root.querySelector<HTMLElement>("#wispRail")!.appendChild(wisp.el);

  // Hotkeys (user-configurable on the setup screen): pause/resume and end
  // the talk without hunting for a button mid-recording. Buttons' disabled
  // state guards double-fires; typing targets never trigger binds.
  const keybinds = loadKeybinds();
  const endBtn = root.querySelector<HTMLButtonElement>("#end")!;
  const onHotkey = (e: KeyboardEvent) => {
    if (e.repeat || isTypingTarget(e.target)) return;
    if (eventMatchesCombo(e, keybinds.pause)) {
      e.preventDefault();
      pauseBtn.click();
    } else if (eventMatchesCombo(e, keybinds.startEnd)) {
      e.preventDefault();
      endBtn.click();
    }
  };
  window.addEventListener("keydown", onHotkey);

  let paused = false;
  let ended = false;
  let lastVoiceAt = Date.now();
  let anchoredLine: HTMLElement | null = null;
  const glowTimers = new Map<number, ReturnType<typeof setTimeout>>();
  let nullStatusCount = 0;

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

  // Transcript lines render through transcript.ts: filler sounds as thinner
  // ink, real silences as quiet `· · ·` dividers (speech-clock gap between
  // consecutive segments).
  let prevSegmentEndMs: number | null = null;
  let segmentUnlisten: (() => void) | null = null;
  ipc.onSegment((s) => {
    if (needsPauseMark(prevSegmentEndMs, s.start_ms)) {
      transcriptEl.appendChild(makePauseMark());
    }
    prevSegmentEndMs = s.end_ms;
    transcriptEl.appendChild(renderSegmentLine(s));
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

  // Outline rendering lives in outline.ts (incremental, keyed by label;
  // textContent only — labels are LLM-derived and never parsed as markup).
  let latestOutline: OutlineEntryUI[] = [];
  let currentOutlineP: HTMLElement | null = null;

  // The header names the current chapter (spec: "chapter title auto-derived
  // from current topic") — a glance says where you are without reading the
  // outline. Empty until the insight engine finds a current topic.
  const chapterEl = root.querySelector<HTMLElement>("#chapter")!;

  // Time-in-topic: a quiet "N min" beside the current line once you've
  // been on one topic a while — pacing awareness, not a timer. Tracked on
  // the speech clock (status.elapsed_ms), reset when the topic changes.
  const TOPIC_TIME_AFTER_MS = 180_000;
  let latestElapsedMs = 0;
  let currentTopicLabel: string | null = null;
  let currentTopicSinceMs = 0;
  let topicTimeSpan: HTMLElement | null = null;

  let outlineUnlisten: (() => void) | null = null;
  ipc.onOutline((entries) => {
    latestOutline = entries;
    currentOutlineP = updateOutline(outlineEl, sinkGhosts(entries));
    const current = entries.find((e) => e.status === "current")?.label ?? "";
    chapterEl.textContent = current;
    if (current !== currentTopicLabel) {
      currentTopicLabel = current || null;
      currentTopicSinceMs = latestElapsedMs;
      topicTimeSpan?.remove();
      topicTimeSpan = null;
    }
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
    wonderingEl.classList.remove("chip-callback", "chip-arriving");
    // Force a reflow so re-adding the class restarts the CSS animation
    // even when two questions arrive back to back.
    void wonderingEl.offsetWidth;
    wonderingEl.textContent = question;
    wonderingEl.style.display = "";
    wonderingEl.classList.add("chip-arriving");
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
    wonderingEl.classList.remove("chip-arriving");
    wonderingEl.classList.add("chip-callback");
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      wrapupUnlisten = fn;
    }
  });

  const timer = setInterval(async () => {
    const status = await ipc.sessionStatus();
    if (!status) {
      // Track consecutive null statuses: if the backend session is gone
      // (end_session took it then failed later), every poll returns null
      // and the user is stuck. After 4 consecutive nulls, show recovery UI.
      // Note: during normal session, status is never null (session exists);
      // only accumulates when no session exists. Pre-Begin states never
      // mount live.ts, so 4-null recovery cannot misfire mid-take.
      nullStatusCount++;
      if (nullStatusCount === 4) {
        // Stop polling and show recovery UI
        clearInterval(timer);
        stateEl.textContent = "this take already ended — ";
        const recoverBtn = document.createElement("button");
        recoverBtn.textContent = "Back to the desk";
        recoverBtn.style.marginLeft = "8px";
        recoverBtn.onclick = async () => {
          if (ended) return;
          recoverBtn.disabled = true;
          try {
            const sessions = await ipc.listSessions();
            const last = sessions[0];
            if (!last) {
              stateEl.textContent = "no sessions found";
              recoverBtn.disabled = false;
              return;
            }
            // Run cleanup exactly once before calling onEnded
            ended = true;
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
            window.removeEventListener("keydown", onHotkey);
            // Now run the recap flow with the recovered session
            onEnded(last);
          } catch (e) {
            stateEl.textContent = `recovery failed: ${String(e)}`;
            recoverBtn.disabled = false;
          }
        };
        stateEl.appendChild(recoverBtn);
      }
      return;
    }
    // Reset counter on any non-null status
    nullStatusCount = 0;
    latestElapsedMs = status.elapsed_ms;
    const total = Math.floor(status.elapsed_ms / 1000);
    elapsedEl.textContent = `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;

    // Quiet time-in-topic marker on the current outline line.
    if (
      currentTopicLabel &&
      currentOutlineP &&
      latestElapsedMs - currentTopicSinceMs >= TOPIC_TIME_AFTER_MS
    ) {
      if (topicTimeSpan === null || topicTimeSpan.parentElement !== currentOutlineP) {
        topicTimeSpan?.remove();
        topicTimeSpan = document.createElement("span");
        topicTimeSpan.className = "topic-time";
        currentOutlineP.appendChild(topicTimeSpan);
      }
      const mins = Math.floor((latestElapsedMs - currentTopicSinceMs) / 60_000);
      topicTimeSpan.textContent = `${mins} min`;
    }
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
    if (status.insight_active && latestOutline.length === 0 && outlineEl.children.length === 0) {
      const p = document.createElement("p");
      p.className = "outline-note";
      p.textContent = "listening for the shape of it…";
      outlineEl.appendChild(p);
    } else if (!status.insight_active && latestOutline.length === 0 && outlineEl.children.length === 0) {
      const p = document.createElement("p");
      p.className = "outline-note";
      p.textContent = "the thinking model is off — mirror only";
      outlineEl.appendChild(p);
    }

    if (!paused) {
      const idle = Date.now() - lastVoiceAt;
      if (idle < FLOWING_WITHIN_MS) {
        wisp.setState("flowing");
        // Speaking again: the lost-thread anchor lets go.
        anchoredLine?.classList.remove("thread-anchor");
        anchoredLine = null;
      } else if (idle >= THINKING_AFTER_MS) {
        wisp.setState("thinking");
        // Lost-thread anchor: while you're thinking, the last thing you
        // said gently emphasizes — the mirror answers "where was I?"
        // exactly when you're asking it.
        const lines = transcriptEl.querySelectorAll<HTMLElement>("p[data-segment-id]");
        const last = lines[lines.length - 1];
        if (last && last !== anchoredLine) {
          anchoredLine?.classList.remove("thread-anchor");
          anchoredLine = last;
          last.classList.add("thread-anchor");
        }
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

  endBtn.onclick = async () => {
    if (ended) return;
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
    window.removeEventListener("keydown", onHotkey);
    onEnded(endedSession);
  };
}
