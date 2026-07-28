import { ipc, type InputDevice, type Session } from "../ipc";
import { escapeHtml } from "../escape";
import { fmtDate, fmtDuration } from "../format";
import { createWisp } from "../wisp";
import { createDisclosure } from "../disclosure";
import { createOverflowMenu } from "../overflow";
import { PRESENCE_HINTS, loadPresence, savePresence, type Presence } from "../presence";
import {
  DEFAULT_KEYBINDS,
  comboFromEvent,
  eventMatchesCombo,
  loadKeybinds,
  prettyCombo,
  saveKeybinds,
  type Keybinds,
} from "../keys";

const REFRESH_MS = 4000;

// Guards against re-kicking the download every time the setup screen
// re-renders (e.g. returning here after ending a talk) while a previous
// ensureModels() call from earlier in this app session is still in flight.
// Reset to false on a failed download so a later visit can retry.
let modelDownloadStarted = false;

// Module-scope so a re-render of this screen can unsubscribe whatever
// listener a previous renderSetup() call registered, even though that call's
// own local variable is no longer reachable.
let modelProgressUnlisten: (() => void) | null = null;
let modelReadyUnlisten: (() => void) | null = null;

export function renderSetup(
  root: HTMLElement,
  onStarted: () => void,
  onRecap: (session: Session) => void,
): void {
  // Defensive: drop any listener left over from a previous renderSetup()
  // call before registering a new one below, so listeners never accumulate
  // across re-renders.
  modelProgressUnlisten?.();
  modelProgressUnlisten = null;
  modelReadyUnlisten?.();
  modelReadyUnlisten = null;

  // Decluttered layout: one hero (mic + intent + Begin) leads; Past talks
  // follow; everything set-once (keys, companion, over-time) folds into a
  // single collapsed Settings disclosure built below.
  root.innerHTML = `
    <h1 class="setup-wordmark">Yapper</h1>
    <div class="setup-top">
      <div class="paper-panel setup-hero">
      <div class="label">Microphone</div>
      <div class="mic-row">
        <select id="mic"></select>
        <div class="level-meter"><div id="meter"></div></div>
      </div>
      <div class="label intent-label">What do you want to talk about?</div>
      <textarea id="intent" rows="4" placeholder="a title, or paste your whole notes…"></textarea>
      <p id="focusLine" class="focus-line" style="display:none;"></p>
      <div class="begin-row">
        <button id="start">Begin the talk</button>
      </div>
      <p id="error" class="paused-note" role="alert"></p>
      </div>
      <aside class="setup-wisp-col"><div id="setupWisp" aria-label="companion"></div></aside>
    </div>
    <div id="modelBanner"></div>
    <div id="past" style="margin-top:22px;"></div>
    <div id="settings" class="paper-panel settings-panel"></div>
  `;

  // ---- Settings disclosure: keys + companion + over-time, folded away ----
  const settings = createDisclosure({ label: "Settings", gear: true });
  settings.body.innerHTML = `
    <div class="settings-group">
      <div class="label">Keys</div>
      <div class="key-row"><span>Begin / end the talk</span><kbd id="kbdStartEnd"></kbd><button class="quiet key-change" data-bind="startEnd">change</button></div>
      <div class="key-row"><span>Pause / resume listening</span><kbd id="kbdPause"></kbd><button class="quiet key-change" data-bind="pause">change</button></div>
      <p class="label key-hint" id="keyHint"></p>
      <button class="quiet key-reset" id="keysReset">reset to defaults</button>
    </div>
    <div class="settings-group">
      <div class="label">Companion</div>
      <div class="presence-row" role="radiogroup" aria-label="Companion presence">
        <button class="quiet presence-opt" data-presence="present" role="radio">present</button>
        <button class="quiet presence-opt" data-presence="quieter" role="radio">quieter</button>
        <button class="quiet presence-opt" data-presence="recap-only" role="radio">recap only</button>
      </div>
      <p class="label key-hint" id="presenceHint"></p>
    </div>
    <div class="settings-group" id="trendGroup" style="display:none;">
      <div class="label">Over time</div>
      <div id="trend"></div>
    </div>
  `;
  root.querySelector<HTMLElement>("#settings")!.appendChild(settings.el);

  // The companion is present at the desk even before a talk — asleep, a
  // pilot light. The live screen's wisp wakes fresh; this one just sleeps.
  const wisp = createWisp();
  wisp.setState("sleep");
  root.querySelector<HTMLElement>("#setupWisp")!.appendChild(wisp.el);

  const mic = root.querySelector<HTMLSelectElement>("#mic")!;
  const errorEl = root.querySelector<HTMLParagraphElement>("#error")!;
  const pastEl = root.querySelector<HTMLElement>("#past")!;
  const trendEl = root.querySelector<HTMLElement>("#trend")!;
  const bannerEl = root.querySelector<HTMLElement>("#modelBanner")!;

  // Same ended-flag pattern as live.ts's onLevel: if cleanup() runs before
  // the listen() promise resolves, unlisten immediately on arrival instead
  // of leaking the subscription. (modelProgressUnlisten/modelReadyUnlisten
  // themselves are the module-scope variables above.)
  let modelBannerDone = false;
  const stopModelListeners = () => {
    modelBannerDone = true;
    modelProgressUnlisten?.();
    modelProgressUnlisten = null;
    modelReadyUnlisten?.();
    modelReadyUnlisten = null;
  };

  // Yapper downloads two models, one at a time (STT first, then the LLM) —
  // see lib.rs's `ensure_models`. Both share this one progress bar; each
  // `model:progress` event's `model` field says which download is currently
  // active, so the banner text is recomputed per-event rather than fixed at
  // banner-open time.
  function textForModel(model: string): string {
    return model === "llm"
      ? "downloading the thinking model (~2 GB, one time) — you can record meanwhile; insight joins when it's ready"
      : "downloading the listening model (~250 MB, one time) — you can record meanwhile; transcription joins when it's ready";
  }

  async function initModelBanner(): Promise<void> {
    let ready: { stt: boolean; llm: boolean };
    try {
      ready = await ipc.modelsReady();
    } catch {
      // fail-safe: don't block the desk on a status-check error
      ready = { stt: false, llm: false };
    }
    if (ready.stt && ready.llm) return;

    bannerEl.innerHTML = `
      <div class="paper-panel" style="margin-top:22px;">
        <div class="label">First run</div>
        <p id="modelText" class="paused-note" style="margin-bottom:8px;">${textForModel(ready.stt ? "llm" : "moonshine-base-en-int8")}</p>
        <div class="level-meter"><div id="modelBar"></div></div>
      </div>
    `;
    const textEl = bannerEl.querySelector<HTMLElement>("#modelText")!;
    const barEl = bannerEl.querySelector<HTMLElement>("#modelBar")!;

    // Guards the "models ready" fade so it only ever runs once, whichever
    // path notices completion first — the model:ready handler below (fires
    // the instant the last needed model finishes) and ensureModels()'s own
    // .then() (fires once both downloads have fully returned) land within
    // moments of each other.
    let bannerFaded = false;
    function showModelsReady(): void {
      if (bannerFaded) return;
      bannerFaded = true;
      stopModelListeners();
      textEl.textContent = "models ready";
      barEl.style.animation = "none";
      barEl.style.opacity = "1";
      barEl.style.width = "100%";
      setTimeout(() => {
        bannerEl.innerHTML = "";
      }, 3000);
    }

    ipc.onModelProgress((p) => {
      textEl.textContent = textForModel(p.model);
      if (p.total === 0) {
        // No Content-Length from the server: indeterminate progress —
        // full-width bar, pulsing at reduced opacity.
        barEl.style.width = "100%";
        barEl.style.opacity = "0.5";
        barEl.style.animation = "yapper-model-pulse 1.2s ease-in-out infinite";
      } else {
        barEl.style.animation = "none";
        barEl.style.opacity = "1";
        barEl.style.width = `${Math.min(100, (p.downloaded / p.total) * 100)}%`;
      }
    }).then((fn) => {
      if (modelBannerDone) {
        fn();
      } else {
        modelProgressUnlisten = fn;
      }
    });

    // model:ready fires per-model, the moment that model's files are
    // verified on disk — well before ensureModels() as a whole resolves, and
    // (for whichever model downloads second) before that model's own first
    // model:progress event, which can lag by up to ~1 MB of download. This
    // flips the banner text immediately on the STT→LLM handoff instead of
    // leaving stale "listening model" text up while the LLM download is
    // already underway, and fades the banner immediately once the last
    // needed model lands rather than waiting on ensureModels() to resolve.
    ipc.onModelReady((model) => {
      if (model === "llm") {
        showModelsReady();
      } else if (!ready.llm) {
        // The model that just finished wasn't the LLM, so per the
        // sequential STT-then-LLM order in ensure_models, the LLM is next —
        // but only if it actually needs downloading (it may have already
        // been present at banner-open time, in which case ensureModels()
        // resolves right behind this event and showModelsReady() below
        // handles it).
        textEl.textContent = textForModel("llm");
      }
    }).then((fn) => {
      if (modelBannerDone) {
        fn();
      } else {
        modelReadyUnlisten = fn;
      }
    });

    if (!modelDownloadStarted) {
      modelDownloadStarted = true;
      ipc.ensureModels()
        .then(() => {
          showModelsReady();
        })
        .catch((e) => {
          stopModelListeners();
          // Allow a later visit to this screen to retry the download —
          // without this, a failed download could never be retried short
          // of an app restart.
          modelDownloadStarted = false;
          textEl.textContent = `couldn't download a model — ${String(e)} — recording still works, transcription/insight won't join until it's downloaded`;
        });
    }
  }
  initModelBanner();

  // Both lists refresh while this screen is visible: devices come and go
  // (bluetooth mics), recordings get deleted in Finder. Selection and
  // scroll are preserved; DOM is only touched when content actually changed.
  // Sentinel (not "") so the very first refresh always renders — an empty
  // device list has key "", which would otherwise match the initial value
  // and skip drawing the placeholder.
  let deviceKey = " ";
  async function refreshDevices(): Promise<void> {
    const devices: InputDevice[] = await ipc.listInputDevices();
    const key = devices.map((d) => `${d.name}${d.is_default ? "*" : ""}`).join("|");
    if (key === deviceKey) return;
    deviceKey = key;
    if (devices.length === 0) {
      // No inputs found — a plain disabled placeholder instead of an empty
      // dropdown. Begin-the-talk still works: start_session falls back to the
      // system default device when none is passed.
      mic.innerHTML = `<option value="" disabled selected>no microphone found</option>`;
      return;
    }
    const previous = mic.value;
    mic.innerHTML = devices
      .map((d) => `<option value="${escapeHtml(d.name)}" ${d.is_default ? "selected" : ""}>${escapeHtml(d.name)}</option>`)
      .join("");
    if (previous && devices.some((d) => d.name === previous)) {
      mic.value = previous;
    }
  }

  let pastKey = "";
  async function refreshPast(): Promise<void> {
    const sessions: Session[] = await ipc.listSessions();

    // The trend panel reuses this same fetch (no extra ipc round-trip) and
    // has its own change-key, since filler_count/word_count land via a
    // separate DB write shortly after duration_ms (see lib.rs's end_session
    // then update_counts) and aren't part of pastKey below.
    refreshTrend(sessions);

    const key = sessions
      .map((s) => `${s.id}:${s.duration_ms}:${s.audio_exists}:${s.segment_count}`)
      .join("|");
    if (key === pastKey) return;
    pastKey = key;
    pastEl.innerHTML = "";
    if (sessions.length === 0) return;

    const header = document.createElement("div");
    header.className = "label on-desk";
    header.style.marginBottom = "8px";
    header.textContent = "Past talks";
    pastEl.appendChild(header);

    // Compact rows: date · duration · intent · a primary Recap chip, with the
    // rarely-used actions (Export, Show file / Forget) folded into an overflow
    // menu so the row stays legible. textContent throughout — intent is raw
    // user speech and must never parse as markup.
    for (const s of sessions) {
      const row = document.createElement("div");
      row.className = "talk-row paper-panel";

      const date = document.createElement("span");
      date.className = "talk-date";
      date.textContent = fmtDate(s.started_at_ms);
      const dur = document.createElement("span");
      dur.className = "talk-dur";
      dur.textContent = s.duration_ms != null ? fmtDuration(s.duration_ms) : "interrupted";
      const intent = document.createElement("span");
      intent.className = "talk-intent";
      intent.textContent = s.intent.trim().split("\n")[0].slice(0, 80);
      row.append(date, dur, intent);

      if (!s.audio_exists) {
        const missing = document.createElement("span");
        missing.className = "talk-missing";
        missing.textContent = "file missing";
        row.appendChild(missing);
      }

      if (s.duration_ms != null) {
        const recap = document.createElement("button");
        recap.className = "talk-recap";
        recap.textContent = "Recap";
        recap.onclick = () => {
          const session = sessions.find((x) => x.id === s.id);
          if (!session) return;
          // Same cleanup onStarted runs — recap fully replaces this screen.
          cleanup();
          onRecap(session);
        };
        row.appendChild(recap);
      }

      const items = [];
      if (s.segment_count > 0) {
        items.push({
          label: "Export transcript",
          onSelect: () =>
            ipc.exportTranscript(s.id).catch((e) => { errorEl.textContent = String(e); }),
        });
      }
      if (s.audio_exists) {
        items.push({
          label: "Show file",
          onSelect: () =>
            ipc.revealSession(s.id).catch((e) => {
              errorEl.textContent = String(e);
              void refreshPast(); // file may have vanished since last poll
            }),
        });
      } else {
        items.push({
          label: "Forget",
          onSelect: () =>
            ipc.forgetSession(s.id).then(() => refreshPast()).catch((e) => {
              errorEl.textContent = String(e);
            }),
        });
      }
      if (items.length) row.appendChild(createOverflowMenu(items));

      pastEl.appendChild(row);
    }
  }

  // "Over time": fillers-per-speaking-minute per completed talk, oldest to
  // newest. Only sessions with real counts and at least two minutes of
  // speaking are informative enough to plot; fewer than three such points
  // and the whole panel stays hidden rather than showing a lonely dot.
  const MIN_TREND_POINTS = 3;
  const MIN_DURATION_FOR_TREND_MS = 120_000;

  function trendSeries(sessions: Session[]): { id: number; fpm: number }[] {
    return sessions
      .filter(
        (s) =>
          s.duration_ms != null &&
          s.duration_ms >= MIN_DURATION_FOR_TREND_MS &&
          s.filler_count != null &&
          s.word_count != null &&
          s.word_count > 0,
      )
      .slice()
      .reverse() // listSessions() is newest-first; the trend reads oldest→newest
      .map((s) => {
        const speakingMs = s.duration_ms! - s.paused_ms;
        const minutes = speakingMs / 60_000;
        const fpm = minutes > 0 ? s.filler_count! / minutes : 0;
        return { id: s.id, fpm };
      })
      .filter((p) => Number.isFinite(p.fpm));
  }

  // Builds the sparkline as an SVG string. Every interpolated value here is
  // a number computed above (ids and fpm ratios) — never session text — so
  // string interpolation is safe without escapeHtml; textContent/escapeHtml
  // discipline still applies everywhere else dynamic strings are involved.
  function renderSparkline(points: { id: number; fpm: number }[]): string {
    const W = 300;
    const H = 48;
    const PAD = 4;
    const values = points.map((p) => p.fpm);
    const min = Math.min(...values);
    const max = Math.max(...values);
    const range = max - min;
    const xStep = points.length > 1 ? (W - 2 * PAD) / (points.length - 1) : 0;
    const coords = points.map((p, i) => {
      const x = PAD + i * xStep;
      const y = range > 0 ? PAD + (1 - (p.fpm - min) / range) * (H - 2 * PAD) : H / 2;
      return { x, y };
    });
    const linePoints = coords.map((c) => `${c.x.toFixed(2)},${c.y.toFixed(2)}`).join(" ");
    const last = coords[coords.length - 1]!;
    return `
      <svg viewBox="0 0 ${W} ${H}" width="100%" height="48" preserveAspectRatio="none" role="img" aria-label="fillers per minute, by talk">
        <polyline points="${linePoints}" fill="none" style="stroke:var(--gold-ink); stroke-width:2.5;" stroke-linecap="round" stroke-linejoin="round" />
        <circle cx="${last.x.toFixed(2)}" cy="${last.y.toFixed(2)}" r="3.5" style="fill:var(--gold-ink);" />
      </svg>
    `;
  }

  let trendKey = "";
  function refreshTrend(sessions: Session[]): void {
    const points = trendSeries(sessions);
    const key = points.map((p) => `${p.id}:${p.fpm}`).join("|");
    if (key === trendKey) return;
    trendKey = key;
    // The sparkline lives inside the Settings disclosure (#trendGroup already
    // carries the "Over time" label). Hide the whole group when there aren't
    // enough points rather than showing a lonely dot.
    const group = root.querySelector<HTMLElement>("#trendGroup");
    if (points.length < MIN_TREND_POINTS) {
      if (group) group.style.display = "none";
      trendEl.innerHTML = "";
      return;
    }
    if (group) group.style.display = "";
    trendEl.innerHTML = `${renderSparkline(points)}<p class="label" style="margin-top:6px; margin-bottom:0;">fillers per minute, by talk</p>`;
  }

  // ---- Keys panel: show current binds, capture replacements, persist ----
  let keybinds: Keybinds = loadKeybinds();
  const kbdEls = {
    pause: root.querySelector<HTMLElement>("#kbdPause")!,
    startEnd: root.querySelector<HTMLElement>("#kbdStartEnd")!,
  };
  const keyHintEl = root.querySelector<HTMLElement>("#keyHint")!;
  const renderKbds = () => {
    kbdEls.pause.textContent = prettyCombo(keybinds.pause);
    kbdEls.startEnd.textContent = prettyCombo(keybinds.startEnd);
  };
  renderKbds();

  // While capturing, the next non-modifier keydown becomes the bind
  // (Escape cancels). Capture-phase listener so the start hotkey below
  // never fires off the very keystroke being recorded.
  let captureCleanup: (() => void) | null = null;
  const stopCapture = () => {
    captureCleanup?.();
    captureCleanup = null;
    keyHintEl.textContent = "";
  };
  root.querySelectorAll<HTMLButtonElement>("button.key-change").forEach((btn) => {
    btn.onclick = () => {
      stopCapture();
      const bind = btn.dataset.bind as keyof Keybinds;
      keyHintEl.textContent = "press a key combination… (Esc cancels)";
      const onCapture = (e: KeyboardEvent) => {
        e.preventDefault();
        e.stopPropagation();
        if (e.code === "Escape") {
          stopCapture();
          return;
        }
        const combo = comboFromEvent(e);
        if (!combo) return; // bare modifier — keep waiting
        keybinds = { ...keybinds, [bind]: combo };
        saveKeybinds(keybinds);
        renderKbds();
        stopCapture();
      };
      window.addEventListener("keydown", onCapture, { capture: true });
      captureCleanup = () =>
        window.removeEventListener("keydown", onCapture, { capture: true });
    };
  });
  root.querySelector<HTMLButtonElement>("#keysReset")!.onclick = () => {
    stopCapture();
    keybinds = { ...DEFAULT_KEYBINDS };
    saveKeybinds(keybinds);
    renderKbds();
  };

  // Focus thread: the last retro's experiment rides into the next take —
  // deliberate practice, one focus at a time. Best-effort; absent quietly.
  const focusLineEl = root.querySelector<HTMLElement>("#focusLine")!;
  ipc
    .latestFocus()
    .then((focus) => {
      if (focus) {
        focusLineEl.textContent = `carrying forward — ${focus}`;
        focusLineEl.style.display = "";
      }
    })
    .catch(() => {
      /* no focus line, nothing lost */
    });

  // ---- Companion presence: how much the companion says during a take ----
  const presenceHintEl = root.querySelector<HTMLElement>("#presenceHint")!;
  const presenceButtons = root.querySelectorAll<HTMLButtonElement>("button.presence-opt");
  const renderPresence = () => {
    const current = loadPresence();
    presenceButtons.forEach((btn) => {
      const active = btn.dataset.presence === current;
      btn.classList.toggle("selected", active);
      // Radiogroup semantics: a radio announces via aria-checked (not
      // aria-pressed), and only the checked option is a tab stop — arrow
      // keys move between the rest (roving tabindex).
      btn.setAttribute("aria-checked", String(active));
      btn.tabIndex = active ? 0 : -1;
    });
    presenceHintEl.textContent = PRESENCE_HINTS[current];
  };
  renderPresence();
  const selectPresence = (btn: HTMLButtonElement) => {
    savePresence(btn.dataset.presence as Presence);
    renderPresence();
  };
  presenceButtons.forEach((btn, i) => {
    btn.onclick = () => selectPresence(btn);
    // Arrow keys walk the group and select as they land — the WAI-ARIA
    // radiogroup pattern. Wraps at both ends.
    btn.onkeydown = (e) => {
      const dir =
        e.key === "ArrowRight" || e.key === "ArrowDown"
          ? 1
          : e.key === "ArrowLeft" || e.key === "ArrowUp"
            ? -1
            : 0;
      if (dir === 0) return;
      e.preventDefault();
      const next = presenceButtons[(i + dir + presenceButtons.length) % presenceButtons.length];
      selectPresence(next);
      next.focus();
    };
  });

  // Start-the-talk hotkey. Deliberately no typing-target guard: pressing
  // ⌘/Ctrl+Enter straight from the intent textarea is the natural flow.
  const onHotkey = (e: KeyboardEvent) => {
    if (!e.repeat && eventMatchesCombo(e, keybinds.startEnd)) {
      e.preventDefault();
      root.querySelector<HTMLButtonElement>("#start")!.click();
    }
  };
  window.addEventListener("keydown", onHotkey);

  const refreshAll = () => {
    refreshDevices().catch((e) => { errorEl.textContent = String(e); });
    refreshPast().catch((e) => { errorEl.textContent = String(e); });
  };
  refreshAll();
  const timer = setInterval(refreshAll, REFRESH_MS);
  window.addEventListener("focus", refreshAll);
  const cleanup = () => {
    clearInterval(timer);
    window.removeEventListener("focus", refreshAll);
    window.removeEventListener("keydown", onHotkey);
    stopCapture();
    stopModelListeners();
    wisp.destroy();
  };

  root.querySelector<HTMLButtonElement>("#start")!.onclick = async () => {
    const intent = root.querySelector<HTMLTextAreaElement>("#intent")!.value;
    try {
      await ipc.startSession(intent, mic.value || undefined);
      cleanup();
      onStarted();
    } catch (e) {
      errorEl.textContent = String(e);
    }
  };
}
