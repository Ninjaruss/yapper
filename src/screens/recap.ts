import { convertFileSrc } from "@tauri-apps/api/core";
import { ipc, type Session, type OutlineRow, type YapperEvent, type TranscriptSegment } from "../ipc";
import { fmtDate, fmtDuration } from "../format";

// Human labels for event kinds shown in the "Moments" timeline. Unknown
// kinds fall back to the raw kind string rather than disappearing.
const KIND_LABELS: Record<string, string> = {
  rhythm_filler: "rhythm nudge",
  rhythm_pace: "pace nudge",
  repetition: "echo",
  question: "wondered",
  wrapup: "wrap-up cue",
  shine: "shine",
};

// Only rhythm/repetition signals are the model's *judgment calls* — question,
// wrapup and shine are moments, not accusations, so they never get a
// feedback button (spec: "recap lists fired signals with feedback").
const FEEDBACK_KINDS = new Set(["rhythm_filler", "rhythm_pace", "repetition"]);

function fmtMmSs(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

export function renderRecap(
  root: HTMLElement,
  session: Session,
  onBack: () => void,
): void {
  const intentFirstLine = session.intent.trim().split("\n")[0] || "(no intent set)";

  root.innerHTML = `
    <div class="paper-panel">
      <div class="label">Recap</div>
      <p style="font-family:var(--mono); color:var(--ink-soft); margin-bottom:4px;">
        <span id="recapDate"></span> · <span id="recapDuration"></span>
      </p>
      <p id="recapIntent" style="font-style:italic; margin:0;"></p>
    </div>
    <div class="paper-panel" id="recapListenPanel" style="margin-top:16px; display:none;">
      <div class="label">Listen back</div>
      <audio id="recapAudio" controls preload="metadata" style="width:100%; margin-top:6px;"></audio>
    </div>
    <div class="paper-panel" style="margin-top:16px;">
      <div class="label">The shape of it</div>
      <div id="recapOutline"></div>
    </div>
    <div class="paper-panel" id="recapTranscriptPanel" style="margin-top:16px; display:none;">
      <div class="label">Transcript</div>
      <p class="label" id="recapTranscriptHint" style="text-transform:none; letter-spacing:0; font-style:italic;">every line, timestamped</p>
      <div id="recapTranscript" style="max-height:40vh; overflow-y:auto;"></div>
    </div>
    <div class="paper-panel" style="margin-top:16px;">
      <div class="label">Moments</div>
      <div id="recapSignals"></div>
    </div>
    <p id="recapStats" class="label" style="margin-top:10px;"></p>
    <p id="recapError" class="paused-note" role="alert"></p>
    <div style="display:flex; gap:10px; margin-top:8px;">
      <button id="recapExport" class="quiet" style="color:var(--ink); border-color:var(--ink-soft); display:none;">Export transcript</button>
      <button id="recapReveal" class="quiet" style="color:var(--ink); border-color:var(--ink-soft); display:none;">Show file</button>
      <button id="recapBack">Back to the desk</button>
    </div>
  `;

  const errorEl = root.querySelector<HTMLElement>("#recapError")!;
  const outlineEl = root.querySelector<HTMLElement>("#recapOutline")!;
  const signalsEl = root.querySelector<HTMLElement>("#recapSignals")!;
  const statsEl = root.querySelector<HTMLElement>("#recapStats")!;

  // All dynamic text goes through textContent (never innerHTML) — intent,
  // outline labels and event notes can all echo raw user speech / LLM
  // output and must never be parsed as markup.
  root.querySelector<HTMLElement>("#recapDate")!.textContent = fmtDate(session.started_at_ms);
  root.querySelector<HTMLElement>("#recapDuration")!.textContent = fmtDuration(
    session.duration_ms ?? 0,
  );
  root.querySelector<HTMLElement>("#recapIntent")!.textContent = intentFirstLine;

  if (session.segment_count > 0) {
    const exportBtn = root.querySelector<HTMLButtonElement>("#recapExport")!;
    exportBtn.style.display = "";
    exportBtn.onclick = () => {
      ipc.exportTranscript(session.id).catch((e) => {
        errorEl.textContent = String(e);
      });
    };
  }
  if (session.audio_exists) {
    const revealBtn = root.querySelector<HTMLButtonElement>("#recapReveal")!;
    revealBtn.style.display = "";
    revealBtn.onclick = () => {
      ipc.revealSession(session.id).catch((e) => {
        errorEl.textContent = String(e);
      });
    };
  }
  root.querySelector<HTMLButtonElement>("#recapBack")!.onclick = () => {
    if (objectUrl) URL.revokeObjectURL(objectUrl);
    onBack();
  };

  // Listen-back player: served through Tauri's asset protocol (scoped to the
  // recordings dirs in tauri.conf.json). WKWebView plays both WAV and FLAC.
  const audioEl = root.querySelector<HTMLAudioElement>("#recapAudio")!;
  let objectUrl: string | null = null;
  // WKWebView streams media with byte-range requests, and range responses
  // through the custom asset protocol arrive subtly wrong (valid files fail
  // with MEDIA_ERR_DECODE). Fetching the whole file once and playing from a
  // Blob keeps decoding entirely in-memory and range-free.
  const loadAudio = (path: string) =>
    fetch(convertFileSrc(path))
      .then((r) => {
        if (!r.ok) throw new Error(`asset fetch ${r.status}`);
        return r.blob();
      })
      .then((blob) => {
        if (objectUrl) URL.revokeObjectURL(objectUrl);
        objectUrl = URL.createObjectURL(blob);
        audioEl.src = objectUrl;
      });
  if (session.audio_exists && session.audio_path) {
    root.querySelector<HTMLElement>("#recapListenPanel")!.style.display = "";
    loadAudio(session.audio_path).catch(() => retryFreshPath());
    // A just-ended session's WAV converts to FLAC in the background and the
    // WAV is then deleted; if playback breaks, refetch the fresh path once.
    // Any other failure (file deleted in Finder, retry also failing) hides
    // the player and says so plainly instead of leaving a dead control.
    let retried = false;
    const playbackGone = () => {
      root.querySelector<HTMLElement>("#recapListenPanel")!.style.display = "none";
      const code = audioEl.error?.code;
      errorEl.textContent = code
        ? `playback failed (media error ${code}) — transcript still available`
        : "the recording file went missing — transcript still available";
    };
    // A just-ended session's WAV converts to FLAC in the background and the
    // WAV is then deleted; on any load failure, refetch the fresh path once.
    const retryFreshPath = () => {
      if (retried) {
        playbackGone();
        return;
      }
      retried = true;
      ipc
        .listSessions()
        .then((sessions) => {
          const fresh = sessions.find((s) => s.id === session.id);
          if (fresh?.audio_path) {
            loadAudio(fresh.audio_path).catch(playbackGone);
          } else {
            playbackGone();
          }
        })
        .catch(playbackGone);
    };
    audioEl.onerror = retryFreshPath;
  }

  // Transcript panel: every stored segment, timestamped; clicking a line
  // seeks the player (segment timestamps and the recording share the same
  // speech clock — paused time exists in neither).
  const transcriptPanel = root.querySelector<HTMLElement>("#recapTranscriptPanel")!;
  const transcriptEl = root.querySelector<HTMLElement>("#recapTranscript")!;
  if (session.segment_count > 0) {
    ipc
      .listSegments(session.id)
      .then((segments: TranscriptSegment[]) => {
        if (segments.length === 0) return;
        transcriptPanel.style.display = "";
        if (session.audio_exists) {
          root.querySelector<HTMLElement>("#recapTranscriptHint")!.textContent =
            "click a line to jump the playback there";
        }
        for (const seg of segments) {
          const p = document.createElement("p");
          p.className = "transcript-line";
          const stamp = document.createElement("span");
          stamp.className = "transcript-stamp";
          stamp.textContent = fmtMmSs(seg.start_ms);
          const text = document.createElement("span");
          text.textContent = seg.text;
          p.append(stamp, text);
          if (session.audio_exists) {
            // Keyboard-reachable like every other action in the app.
            p.classList.add("seekable");
            p.setAttribute("role", "button");
            p.tabIndex = 0;
            const seek = () => {
              audioEl.currentTime = seg.start_ms / 1000;
              void audioEl.play().catch(() => {});
            };
            p.onclick = seek;
            p.onkeydown = (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                seek();
              }
            };
          }
          transcriptEl.appendChild(p);
        }
      })
      .catch((e) => {
        errorEl.textContent = String(e);
      });
  }

  // Outline panel — same status styling as the live mirror; at recap time
  // every entry has already settled, but status still renders faithfully
  // (an intent line the talk never reached stays ghosted, now with a quiet
  // "(never got there)" suffix instead of implying it'll still happen).
  ipc
    .listOutline(session.id)
    .then((rows: OutlineRow[]) => {
      outlineEl.innerHTML = "";
      if (rows.length === 0) {
        const p = document.createElement("p");
        p.className = "outline-intent";
        p.textContent = "no outline — the thinking model was off";
        outlineEl.appendChild(p);
        return;
      }
      for (const row of rows) {
        const p = document.createElement("p");
        if (row.status === "covered") {
          p.className = "outline-covered";
          p.textContent = row.label;
        } else if (row.status === "current") {
          p.className = "outline-current";
          p.textContent = `✎ ${row.label}`;
        } else {
          p.className = "outline-intent";
          p.textContent = `◌ ${row.label} (never got there)`;
        }
        outlineEl.appendChild(p);
      }
    })
    .catch((e) => {
      outlineEl.innerHTML = "";
      const p = document.createElement("p");
      p.className = "paused-note";
      p.textContent = String(e);
      outlineEl.appendChild(p);
    });

  // Signals timeline + stats line — both depend on the same listEvents()
  // call, so they're built from one fetch.
  ipc
    .listEvents(session.id)
    .then((events: YapperEvent[]) => {
      signalsEl.innerHTML = "";
      if (events.length === 0) {
        const p = document.createElement("p");
        p.className = "outline-intent";
        p.textContent = "a quiet one — no moments flagged";
        signalsEl.appendChild(p);
      } else {
        for (const ev of events) {
          signalsEl.appendChild(buildSignalRow(ev, errorEl));
        }
      }

      if (session.word_count != null && session.filler_count != null) {
        statsEl.textContent =
          `${session.word_count} words · ${session.filler_count} fillers · ` +
          `${events.length} signals`;
      }
    })
    .catch((e) => {
      signalsEl.innerHTML = "";
      const p = document.createElement("p");
      p.className = "paused-note";
      p.textContent = String(e);
      signalsEl.appendChild(p);
    });
}

function buildSignalRow(ev: YapperEvent, errorEl: HTMLElement): HTMLElement {
  const row = document.createElement("div");
  row.style.display = "flex";
  row.style.alignItems = "center";
  row.style.gap = "10px";
  row.style.margin = "6px 0";

  const alreadyFlagged = ev.user_feedback != null;
  if (alreadyFlagged) {
    row.style.opacity = "0.55";
  }

  const text = document.createElement("span");
  text.style.flex = "1";
  const label = KIND_LABELS[ev.kind] ?? ev.kind;
  text.textContent = ev.note
    ? `[${fmtMmSs(ev.at_ms)}] ${label} · ${ev.note}`
    : `[${fmtMmSs(ev.at_ms)}] ${label}`;
  row.appendChild(text);

  if (FEEDBACK_KINDS.has(ev.kind)) {
    if (alreadyFlagged) {
      row.appendChild(notedSpan());
    } else {
      const btn = document.createElement("button");
      btn.className = "quiet";
      btn.style.color = "var(--ink)";
      btn.style.borderColor = "var(--ink-soft)";
      btn.style.padding = "4px 10px";
      btn.style.fontSize = "0.8rem";
      btn.textContent = "that was wrong";
      btn.onclick = () => {
        btn.disabled = true;
        ipc
          .setEventFeedback(ev.id, "wrong")
          .then(() => {
            row.style.opacity = "0.55";
            btn.replaceWith(notedSpan());
          })
          .catch((e) => {
            btn.disabled = false;
            errorEl.textContent = String(e);
          });
      };
      row.appendChild(btn);
    }
  }

  return row;
}

function notedSpan(): HTMLElement {
  const span = document.createElement("span");
  span.className = "label";
  span.textContent = "noted";
  return span;
}
