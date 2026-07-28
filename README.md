# Yapper

A desk for practicing talks out loud. You set an intent, start speaking, and a
quiet companion listens — mirroring the shape of what you're saying, noticing
when you circle back, and holding a timestamped record you can listen back to.

Everything runs **locally**. Speech-to-text and the "thinking" model both live
on your machine; after the one-time model download, Yapper needs no network and
sends nothing anywhere. No accounts, no cloud.

> The framing throughout is no-shame: the companion describes, it never scolds.
> Stats are measured against *your own* past talks, never a universal "correct."

## What it does

**At the desk (setup)**
- Pick a microphone, jot an intent (a title, or paste your whole notes).
- See past talks, export transcripts, and an "over time" sparkline of your
  fillers-per-minute across sessions.
- Configurable hotkeys and a companion "presence" setting (how much it says
  mid-talk: present · quieter · recap-only).

**While you talk (live)**
- A running outline of the shape of your talk, with the current chapter named.
- A transcript that ghosts filler sounds, marks real pauses, and — when you
  trail off — gently anchors the last thing you said ("where was I?").
- The companion (a small animated wisp) reflects rhythm nudges, repeated points
  ("echo noticed"), open questions it's wondering about, and a wrap-up cue.

**Looking back (recap)**
- Listen back to the recording with a keyboard-seekable scrubber; click any
  transcript line to jump the playback there.
- The settled outline (with anything you never got to marked plainly), a
  timeline of "moments" you can flag as wrong, and stats: words, pace, long
  pauses, and fillers — each against your usual.
- A story-shape retrospective (stakes / opening / landing, plus one experiment
  to try next time). That experiment rides forward into your next talk as a
  "focus thread."

## How it works

| Piece | What | Size |
|-------|------|------|
| Speech-to-text | [Moonshine base-en int8](https://github.com/k2-fsa/sherpa-onnx) via sherpa-onnx | ~250 MB |
| Thinking model | [Qwen2.5-3B-Instruct](https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF) (GGUF, q4_k_m) | ~2 GB |

Both download once on first run, one at a time, behind a single progress bar.
Recording and transcription work the moment the STT model lands; insight joins
when the LLM finishes. Nothing blocks you from recording in the meantime.

**Where your data lives** (all on your machine):

| What | Location |
|------|----------|
| Recordings | `~/Music/Yapper/session-<id>.{wav,flac}` (falls back to the app-data dir) |
| Sessions, transcripts, outlines, events | `<app-data-dir>/yapper.db` (SQLite) |
| Downloaded models | `<app-data-dir>/models/` |

Fresh recordings are captured as WAV, then converted to FLAC in the background
to save space (the WAV is deleted once the FLAC verifies).

## Development

Prerequisites: [Rust](https://www.rust-lang.org/tools/install), Node.js, and the
[Tauri v2 system dependencies](https://v2.tauri.app/start/prerequisites/) for
your OS.

```bash
npm install
npm run tauri dev      # run the app (Vite + Tauri, hot-reloading frontend)
```

Frontend-only commands:

```bash
npm run test           # Vitest — frontend unit tests
npm run build          # typecheck (tsc) + production bundle
```

Build a distributable app:

```bash
npm run tauri build
```

## Project layout

```
src/                   Frontend (vanilla TypeScript, no framework)
  screens/             setup · live · recap — one module each
  wisp.ts / wisp.css   the animated companion
  ipc.ts               typed wrapper over every Tauri command + event
  outline, stats,      small pure modules (unit-tested)
  presence, keys, …
src-tauri/src/         Rust backend
  audio/               capture (cpal), WAV writing, FLAC encode
  stt/                 Moonshine STT worker + VAD + resampling
  insight/             LLM worker, prompt, grounding guard
  analysis/            rhythm nudges, repetition/echo detection
  models/              first-run model downloads
  store/               SQLite persistence
  lib.rs               Tauri command surface + session orchestration
```

Frontend and backend talk over a small typed IPC layer: commands live in
[`src/ipc.ts`](src/ipc.ts) and map one-to-one to the `#[tauri::command]`
handlers in [`src-tauri/src/lib.rs`](src-tauri/src/lib.rs); live updates arrive
as Tauri events (`audio:level`, `transcript:segment`, `insight:outline`, …).

## Stack

Tauri 2 · vanilla TypeScript · Vite · Vitest — on the Rust side, cpal (audio),
sherpa-rs (STT), `llama-cpp-2` (in-process llama.cpp, Metal-accelerated on
macOS), and rusqlite (bundled SQLite).
