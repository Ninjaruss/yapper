# Plan 2 Task 1 — Spike Notes: sherpa-rs + Moonshine

**Status:** proven working against the golden fixture. Treat this file as canon for Tasks 5 and 7 — the plan's guessed API shapes were correct in every detail that matters, with one difference noted below (`transcribe()`'s return type).

## Crate version

```toml
[dependencies]
sherpa-rs = "0.6.8"
```

Pinned in `Cargo.lock` as `sherpa-rs 0.6.8` / `sherpa-rs-sys 0.6.8` (both from crates.io). `cargo add sherpa-rs` picked up default features `download-binaries` + `tts` — no feature flags need to be set explicitly for our use case (Moonshine offline ASR only).

### Build behavior (important gotcha)

With the default `download-binaries` feature, **`sherpa-rs-sys`'s build.rs downloads prebuilt `sherpa-onnx` dylibs instead of compiling from source with cmake.** First `cargo build` took ~47s (not the "several minutes" the task brief warned about) because of this — cmake was never invoked. Confirmed by checking `target/debug/` after build: `libsherpa-onnx-c-api.dylib` and `libsherpa-onnx-cxx-api.dylib` appear there, copied in by build.rs, not built by CMake. If `download-binaries` is ever disabled (e.g. for reasons Task 7+ discovers), expect a real cmake-from-source build and budget several minutes for it. cmake at `/opt/homebrew/bin/cmake` was present but apparently unused in this default-feature build.

No special env vars were needed. `SHERPA_BUILD_DEBUG=1` is available (per build.rs) if the download step ever needs debugging.

## The real API

Source of truth: `crates/sherpa-rs/src/moonshine.rs` in thewh1teagle/sherpa-rs at tag `v0.6.8` (fetched via `gh api repos/thewh1teagle/sherpa-rs/contents/...`), cross-checked against the crate's own `examples/moonshine.rs`.

```rust
use sherpa_rs::moonshine::{MoonshineConfig, MoonshineRecognizer};

let config = MoonshineConfig {
    preprocessor: format!("{model_dir}/preprocess.onnx"),
    encoder: format!("{model_dir}/encode.int8.onnx"),
    uncached_decoder: format!("{model_dir}/uncached_decode.int8.onnx"),
    cached_decoder: format!("{model_dir}/cached_decode.int8.onnx"),
    tokens: format!("{model_dir}/tokens.txt"),
    ..Default::default()   // provider: None (defaults to cpu), num_threads: Some(1), debug: false
};
let mut recognizer = MoonshineRecognizer::new(config).unwrap(); // Result<Self, eyre::Report>

let result = recognizer.transcribe(16_000, &sixteen_k_f32_samples);
// result: OfflineRecognizerResult { lang: String, text: String, timestamps: Vec<f32>, tokens: Vec<String> }
println!("{}", result.text);
```

### Deviation from the plan's guess

The plan's pseudocode assumed `recognizer.transcribe(...)` returns something directly printable as the text (`let text = recognizer.transcribe(...); println!("{text:?}")`). **In reality it returns `OfflineRecognizerResult` (a.k.a. `MoonshineRecognizerResult`, a type alias), not a `String`.** The text is at `result.text`. Task 5's `MoonshineEngine::transcribe` must do:

```rust
impl TranscribeEngine for MoonshineEngine {
    fn transcribe(&mut self, samples_16k: &[f32]) -> Result<String, YapperError> {
        let result = self.recognizer.transcribe(16_000, samples_16k);
        Ok(result.text.trim().to_string())
    }
}
```

Everything else in the plan's Task 5 sketch (`MoonshineConfig` field names, `MODEL_FILES` list, constructor shape, `Path`-based `model_dir.join(f)`) matched the real API exactly — no other changes needed.

`MoonshineRecognizer::new` returns `eyre::Result<Self>` (i.e. `Result<Self, eyre::Report>`), which is `Display`-able, so `.map_err(|e| YapperError::Audio(format!("moonshine init: {e}")))` works as written in the plan.

`MoonshineRecognizer` is `unsafe impl Send + Sync` (confirmed in source) — safe to move into the worker thread per Task 8's design.

## Model files + download URL

Release: `k2-fsa/sherpa-onnx`, tag **`asr-models`**, asset **`sherpa-onnx-moonshine-base-en-int8.tar.bz2`**.

```
https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-base-en-int8.tar.bz2
```

Archive size: 250,807,309 bytes (~239 MiB) — the plan's "~400 MB" UI copy (Task 9) should be corrected to "~240 MB".

Extracted, the archive has **one top-level directory** (`sherpa-onnx-moonshine-base-en-int8/`) containing the files directly — Task 7's unpack logic must strip that one path component. Contents:

| file | size (bytes) |
|---|---|
| `preprocess.onnx` | 14,077,290 |
| `encode.int8.onnx` | 50,311,494 |
| `uncached_decode.int8.onnx` | 122,120,451 |
| `cached_decode.int8.onnx` | 99,983,837 |
| `tokens.txt` | 436,688 |
| `LICENSE` | 1,071 |
| `README.md` | 175 |
| `test_wavs/` (dir, ignorable) | — |

Total unpacked: ~274 MB in `~/Library/Application Support/net.ninjaruss.yapper/models/moonshine-base-en-int8/`. This matches the 5-file `MODEL_FILES` list already sketched in the plan's Task 5 (`preprocess.onnx`, `encode.int8.onnx`, `uncached_decode.int8.onnx`, `cached_decode.int8.onnx`, `tokens.txt`) — `LICENSE`/`README.md`/`test_wavs/` are extra and can be ignored by `models_present()`.

Downloaded and extracted for this spike with a plain `curl -L` + `tar xjf`; extraction placed files with the top-level dir stripped by hand-copying (`cp -a src/. dest/`) — Task 7's `ensure_models` needs equivalent strip-prefix logic when unpacking the tarball with the `tar` crate.

## Transcription speed observed

Fixture: `docs/superpowers/fixtures/first-session.wav` (11-minute real recording, per spec). The spike example caps input to the first 60s of (naively-decimated) 16 kHz audio to keep the run fast.

- 60.0s of 16 kHz audio → transcribed in **9.36s** wall-clock (single `transcribe()` call, `num_threads: Some(1)` default, CPU provider, unoptimized `cargo run` debug build).
- That's roughly **6.4x real-time** even in a debug build — comfortably fast enough for the utterance-sized chunks (a few seconds each) the real VAD-chunked pipeline (Task 4/8) will feed it. A release build should be faster still.
- Output was clean, punctuated, recognizable English matching the actual spoken content of the fixture (rambling narration about the app/UI, colors, testing) — no garbage/hallucination observed.

## Gotchas

- No cmake build was actually exercised (see "Build behavior" above) — don't assume the "several minutes" first-build warning applies unless `download-binaries` is turned off.
- `MoonshineConfig`'s `..Default::default()` sets `provider: None` (resolved internally to `get_default_provider()`, effectively `"cpu"` on this machine) and `num_threads: Some(1)`. Task 5/7 can leave these as-is; no explicit provider/thread tuning was needed to get a fast, correct transcription.
- The naive `step_by` decimation to 16 kHz (spec's WAV is 48 kHz, per Plan 1) was sufficient for Moonshine to produce a clean transcript in this spike — the real rubato resampler (Task 3) should only improve on this, not fix a correctness bug.
- `result.text` includes a leading space in the observed output (`" Okay, so..."`) — worth `.trim()`ing in `MoonshineEngine::transcribe`, which the plan's Task 5 code already does.
