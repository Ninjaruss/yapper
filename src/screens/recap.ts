import { convertFileSrc } from "@tauri-apps/api/core";
import { ipc, type Session, type OutlineRow, type YapperEvent, type TranscriptSegment } from "../ipc";
import { fmtDate, fmtDuration } from "../format";
import { coverageNote, fillersPerMinute, longPauseCount, usualFillersPerMinute, usualWordsPerMinute, wordsPerMinute } from "../stats";
import { sinkGhosts } from "../outline";
import { createDisclosure } from "../disclosure";
import { currentSegmentIndex } from "../transcript";

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
      <p id="recapFocus" class="focus-line" style="display:none;"></p>
    </div>
    <div class="paper-panel" id="recapListenPanel" style="margin-top:16px; display:none;">
      <div class="label">Listen back</div>
      <div class="audio-bar">
        <button id="recapPlay" class="quiet audio-play" aria-label="play">▶</button>
        <div class="audio-track" id="recapTrack" role="slider" tabindex="0" aria-label="seek" aria-valuemin="0" aria-valuenow="0"><div class="audio-fill" id="recapFill"></div></div>
        <span class="label audio-time" id="recapTime">0:00 / 0:00</span>
      </div>
      <audio id="recapAudio" preload="metadata" style="display:none;"></audio>
    </div>
    <div class="paper-panel" id="recapTranscriptPanel" style="margin-top:16px; display:none;">
      <div class="label">Transcript</div>
      <p class="label" id="recapTranscriptHint" style="text-transform:none; letter-spacing:0; font-style:italic;">every line, timestamped</p>
      <div id="recapTranscript" style="max-height:40vh; overflow-y:auto;"></div>
    </div>
    <div class="paper-panel" style="margin-top:16px;">
      <div class="label">The shape of it</div>
      <div id="recapOutline"></div>
    </div>
    <div class="paper-panel" id="recapRetroPanel" style="margin-top:16px; display:none;">
      <div class="label">Looking back</div>
      <div id="recapRetro"></div>
    </div>
    <p id="recapStats" class="label on-desk" style="margin-top:10px;"></p>
    <div class="paper-panel" id="recapMomentsMount" style="margin-top:12px; padding:2px 22px;"></div>
    <p id="recapError" class="paused-note" role="alert"></p>
    <div style="display:flex; gap:10px; margin-top:12px;">
      <button id="recapExport" class="quiet" style="display:none;">Export transcript</button>
      <button id="recapReveal" class="quiet" style="display:none;">Show file</button>
      <button id="recapBack">Back to the desk</button>
    </div>
  `;

  // Moments fold into a disclosure so the recap opens as a clean summary; the
  // events loader below fills #recapSignals inside its body and sets the count.
  const momentsDisc = createDisclosure({ label: "Moments", count: "" });
  const momentsSignals = document.createElement("div");
  momentsSignals.id = "recapSignals";
  momentsDisc.body.appendChild(momentsSignals);
  root.querySelector<HTMLElement>("#recapMomentsMount")!.appendChild(momentsDisc.el);
  const momentsCountEl = momentsDisc.el.querySelector<HTMLElement>(".disc-count")!;

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
  if (session.focus) {
    const focusEl = root.querySelector<HTMLElement>("#recapFocus")!;
    focusEl.textContent = `this take's experiment — ${session.focus}`;
    focusEl.style.display = "";
  }

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

  // Parchment player chrome over the hidden <audio> — the native control is
  // a white system pill that breaks the study. Transcript click-to-seek
  // drives the same element, so the fill/time stay in sync either way.
  const playBtn = root.querySelector<HTMLButtonElement>("#recapPlay")!;
  const trackEl = root.querySelector<HTMLElement>("#recapTrack")!;
  const fillEl = root.querySelector<HTMLElement>("#recapFill")!;
  const timeEl = root.querySelector<HTMLElement>("#recapTime")!;
  playBtn.onclick = () => {
    if (audioEl.paused) {
      void audioEl.play().catch(() => {
        /* onerror path below handles unplayable files */
      });
    } else {
      audioEl.pause();
    }
  };
  audioEl.onplay = () => {
    playBtn.textContent = "❚❚";
    playBtn.setAttribute("aria-label", "pause");
  };
  audioEl.onpause = () => {
    playBtn.textContent = "▶";
    playBtn.setAttribute("aria-label", "play");
  };
  // Follow-playback highlight: as the audio plays, the current transcript line
  // gets a gold `.now` spine and scrolls into view. Populated when segments
  // load below; a no-op until then (and when there's no audio).
  let scrollSegments: TranscriptSegment[] = [];
  const scrollLineEls: HTMLElement[] = [];
  let nowIdx = -1;
  let userScrolledAt = 0;
  const highlightNowPlaying = () => {
    if (scrollLineEls.length === 0) return;
    const idx = currentSegmentIndex(scrollSegments, audioEl.currentTime * 1000);
    if (idx === nowIdx) return;
    if (nowIdx >= 0) {
      scrollLineEls[nowIdx]?.classList.remove("now");
      scrollLineEls[nowIdx]?.removeAttribute("aria-current");
    }
    nowIdx = idx;
    if (idx < 0) return;
    const line = scrollLineEls[idx]!;
    line.classList.add("now");
    line.setAttribute("aria-current", "true");
    // Yield to a user who just scrolled away to read elsewhere.
    if (Date.now() - userScrolledAt > 1800) line.scrollIntoView({ block: "nearest" });
  };

  const refreshClock = () => {
    const dur = Number.isFinite(audioEl.duration) ? audioEl.duration : 0;
    fillEl.style.width = dur > 0 ? `${(audioEl.currentTime / dur) * 100}%` : "0%";
    timeEl.textContent = `${fmtDuration(audioEl.currentTime * 1000)} / ${fmtDuration(dur * 1000)}`;
    // Keep the slider's assistive-tech state in step with the visual fill.
    trackEl.setAttribute("aria-valuemax", String(Math.round(dur)));
    trackEl.setAttribute("aria-valuenow", String(Math.round(audioEl.currentTime)));
    trackEl.setAttribute(
      "aria-valuetext",
      `${fmtDuration(audioEl.currentTime * 1000)} of ${fmtDuration(dur * 1000)}`,
    );
    highlightNowPlaying();
  };
  audioEl.ontimeupdate = refreshClock;
  audioEl.ondurationchange = refreshClock;
  trackEl.onclick = (e) => {
    const dur = audioEl.duration;
    if (!Number.isFinite(dur) || dur <= 0) return;
    const rect = trackEl.getBoundingClientRect();
    audioEl.currentTime = ((e.clientX - rect.left) / rect.width) * dur;
    refreshClock();
  };
  // Keyboard seeking — the track is a focusable slider, so it must answer
  // arrow keys like every other action in the app (transcript lines below
  // are click- and Enter-seekable; the scrubber shouldn't be mouse-only).
  const SEEK_STEP_S = 5;
  trackEl.onkeydown = (e) => {
    const dur = audioEl.duration;
    if (!Number.isFinite(dur) || dur <= 0) return;
    let next: number | null = null;
    switch (e.key) {
      case "ArrowLeft":
      case "ArrowDown":
        next = audioEl.currentTime - SEEK_STEP_S;
        break;
      case "ArrowRight":
      case "ArrowUp":
        next = audioEl.currentTime + SEEK_STEP_S;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = dur;
        break;
      default:
        return;
    }
    e.preventDefault();
    audioEl.currentTime = Math.max(0, Math.min(dur, next));
    refreshClock();
  };
  // WKWebView streams media with byte-range requests, and range responses
  // through the custom asset protocol arrive subtly wrong (valid files fail
  // with MEDIA_ERR_DECODE). Fetching the whole file once and playing from a
  // Blob keeps decoding entirely in-memory and range-free.
  // WebKit judges blob playability by the blob's MIME type; the asset
  // protocol's sniffed types (audio/x-flac, octet-stream) get rejected with
  // MEDIA_ERR_SRC_NOT_SUPPORTED — so stamp the canonical type explicitly.
  const mimeForPath = (path: string): string => {
    const ext = path.toLowerCase().split(".").pop();
    if (ext === "flac") return "audio/flac";
    if (ext === "wav") return "audio/wav";
    return "";
  };
  const loadAudio = (path: string) =>
    fetch(convertFileSrc(path))
      .then((r) => {
        if (!r.ok) throw new Error(`asset fetch ${r.status}`);
        return r.arrayBuffer();
      })
      .then((buf) => {
        const mime = mimeForPath(path);
        const blob = mime ? new Blob([buf], { type: mime }) : new Blob([buf]);
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

  // Looking back: the story-shape retrospective — three quiet observations
  // (stakes / opening / landing, per the Moth rubric) + one experiment for
  // the next take. Generated once per session by the local LLM; cached
  // rows return instantly, otherwise the panel shows a reading state while
  // the model runs. Any failure (model missing, engine trouble) simply
  // leaves the panel hidden — the recap never depends on it.
  const retroPanel = root.querySelector<HTMLElement>("#recapRetroPanel")!;
  const retroEl = root.querySelector<HTMLElement>("#recapRetro")!;
  function renderRetro(retro: {
    stakes: string | null;
    opening: string | null;
    landing: string | null;
    try_next: string;
  }): void {
    retroEl.innerHTML = "";
    const rows: [string, string | null][] = [
      ["stakes", retro.stakes],
      ["the opening", retro.opening],
      ["the landing", retro.landing],
    ];
    for (const [label, text] of rows) {
      if (!text) continue;
      const p = document.createElement("p");
      p.className = "retro-line";
      const tag = document.createElement("span");
      tag.className = "label retro-tag";
      tag.textContent = label;
      const body = document.createElement("span");
      body.textContent = text;
      p.append(tag, body);
      retroEl.appendChild(p);
    }
    const next = document.createElement("p");
    next.className = "retro-next";
    const tag = document.createElement("span");
    tag.className = "label retro-tag";
    tag.textContent = "next take, maybe";
    const body = document.createElement("span");
    body.textContent = retro.try_next;
    next.append(tag, body);
    retroEl.appendChild(next);
  }
  if (session.segment_count > 0 && session.duration_ms != null) {
    ipc
      .getRetro(session.id)
      .then((cached) => {
        if (cached) {
          retroPanel.style.display = "";
          renderRetro(cached);
          return;
        }
        retroPanel.style.display = "";
        retroEl.innerHTML = "";
        const waiting = document.createElement("p");
        waiting.className = "outline-note";
        waiting.textContent = "reading it back…";
        retroEl.appendChild(waiting);
        return ipc.generateRetro(session.id).then((retro) => renderRetro(retro));
      })
      .catch(() => {
        retroPanel.style.display = "none";
      });
  }

  // Stats line: three equal, neutrally-ordered metrics (pace · pauses ·
  // fillers), each vs the speaker's own median where history exists.
  // Pieces arrive from different fetches; renderStats() composes whatever
  // is known so far.
  const statsState: {
    signals: number | null;
    pauses: number | null;
    usualFpm: number | null;
    usualWpm: number | null;
  } = { signals: null, pauses: null, usualFpm: null, usualWpm: null };
  function renderStats(): void {
    const bits: string[] = [];
    if (session.word_count != null) bits.push(`${session.word_count.toLocaleString()} words`);
    const wpm = wordsPerMinute(session);
    if (wpm != null) {
      const usual = statsState.usualWpm;
      bits.push(usual != null ? `${Math.round(wpm)} wpm (usual ~${Math.round(usual)})` : `${Math.round(wpm)} wpm`);
    }
    if (statsState.pauses != null) {
      bits.push(`${statsState.pauses} long pause${statsState.pauses === 1 ? "" : "s"}`);
    }
    const fpm = fillersPerMinute(session);
    if (session.filler_count != null) {
      const rate = fpm != null ? ` (${fpm.toFixed(1)}/min${statsState.usualFpm != null ? ` — your usual ~${statsState.usualFpm.toFixed(1)}` : ""})` : "";
      bits.push(`${session.filler_count} fillers${rate}`);
    }
    if (statsState.signals != null) bits.push(`${statsState.signals} signals`);
    statsEl.textContent = bits.join(" · ");
  }
  renderStats();

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
        statsState.pauses = longPauseCount(segments);
        renderStats();
        transcriptPanel.style.display = "";
        if (session.audio_exists) {
          root.querySelector<HTMLElement>("#recapTranscriptHint")!.textContent =
            "▶ following playback · click a line to jump";
          // Manual scroll (wheel / drag / arrows) pauses the auto-follow so it
          // doesn't yank the view back while you're reading elsewhere.
          for (const ev of ["wheel", "pointerdown", "keydown"]) {
            transcriptEl.addEventListener(ev, () => {
              userScrolledAt = Date.now();
            });
          }
        }
        for (const seg of segments) {
          const p = document.createElement("p");
          p.className = "transcript-line";
          const stamp = document.createElement("span");
          stamp.className = "transcript-stamp";
          stamp.textContent = fmtDuration(seg.start_ms);
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
          scrollLineEls.push(p);
        }
        // Hand the loaded segments to the follow-playback highlighter.
        scrollSegments = segments;
        highlightNowPlaying();
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
        p.className = "outline-note";
        p.textContent = "no outline — the thinking model was off";
        outlineEl.appendChild(p);
        return;
      }
      for (const row of sinkGhosts(rows)) {
        const p = document.createElement("p");
        if (row.status === "covered") {
          p.className = "outline-covered";
          p.textContent = row.label;
        } else if (row.status === "current") {
          p.className = "outline-current";
          p.textContent = row.label;
        } else {
          p.className = "outline-intent";
          p.textContent = `${row.label} (never got there)`;
        }
        outlineEl.appendChild(p);
      }
      const note = coverageNote(rows.map((r) => r.status));
      if (note) {
        const p = document.createElement("p");
        p.className = "label";
        p.style.marginTop = "8px";
        p.style.marginBottom = "0";
        p.textContent = note;
        // Sibling of the outline container, not a child — the
        // #recapOutline p sizing rules would out-specify .label.
        outlineEl.insertAdjacentElement("afterend", p);
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
        p.className = "outline-note";
        p.textContent = "a quiet one — no moments flagged";
        signalsEl.appendChild(p);
      } else {
        for (const ev of events) {
          signalsEl.appendChild(buildSignalRow(ev, errorEl));
        }
      }

      statsState.signals = events.length;
      momentsCountEl.textContent = String(events.length);
      renderStats();
      // "your usual" anchors arrive from history (no-shame: the speaker's
      // own baseline, never a universal rule).
      ipc
        .listSessions()
        .then((all) => {
          statsState.usualFpm = usualFillersPerMinute(all, session.id);
          statsState.usualWpm = usualWordsPerMinute(all, session.id);
          renderStats();
        })
        .catch(() => {
          /* the plain stats line already stands */
        });
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
    ? `[${fmtDuration(ev.at_ms)}] ${label} · ${ev.note}`
    : `[${fmtDuration(ev.at_ms)}] ${label}`;
  row.appendChild(text);

  if (FEEDBACK_KINDS.has(ev.kind)) {
    if (alreadyFlagged) {
      row.appendChild(notedSpan());
    } else {
      const btn = document.createElement("button");
      btn.className = "quiet";
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
