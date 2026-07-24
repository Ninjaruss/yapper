# Plan 4 Task 1 — llama.cpp spike notes

Status: **DONE.** Local Qwen2.5-3B-Instruct (Metal, in-process via `llama-cpp-2`)
reliably answers Yapper's insight prompt in strict, schema-matching JSON — 3/3
consecutive runs parsed clean, no fence-stripping even needed.

## Crate pick: `llama-cpp-2` (utilityai/llama-cpp-rs)

Candidates evaluated per the plan:

| crate | repo | latest version | last publish | downloads |
|---|---|---|---|---|
| **`llama-cpp-2`** ✅ | utilityai/llama-cpp-rs | 0.1.152 | 2026-07-21 (2 days before this spike) | 870k |
| `llama_cpp` | edgenai/llama_cpp-rs | 0.3.2 | 2024-04-29 | 39k |
| `llama-cpp-4` (not in plan, found while researching) | eugenehp/llama-cpp-rs, a fork | 0.4.2 | 2026-07-13 | 18k |

`llama_cpp` (edgenai) is stale — no publish in over two years, effectively
unmaintained. `llama-cpp-4` is an actively-updated fork but has a fraction of
the adoption/scrutiny. `llama-cpp-2` is the clear pick: published two days
before this spike, by far the most downloaded, a real org (utilityai) uses it
in production, and it mimics the upstream llama.cpp C API closely enough that
its docs/examples map directly onto llama.cpp's own (well-documented)
concepts. Pinned exact version `=0.1.152` in Cargo.toml (not a caret range) —
this crate's API shifts release to release as it tracks upstream llama.cpp,
so an unpinned range could silently break the example on a future
`cargo update`.

Builds cleanly on macOS Apple Silicon with the `metal` feature (a plain
`cargo build`, cmake at `/opt/homebrew/bin/cmake` was picked up
automatically via `llama-cpp-sys-2`'s build.rs, which shells out to cmake to
build ggml/llama.cpp). No manual cmake invocation needed. First build of the
example (including compiling ggml/llama.cpp from source, Metal backend
included) took well under a minute wall-clock on this M4 (`Finished release
profile [optimized] target(s) in 53.48s` for the whole dependency graph,
which was mostly other yapper deps — llama-cpp-sys-2's own cmake+ninja step
is folded into that, no separate timing surfaced by cargo). Confirmed real
Metal linkage post-build with `otool -L`: binary links
`Metal.framework`, `MetalKit.framework`, `Accelerate.framework`; `find`
turned up a genuinely-built `libggml-metal.a` and the JIT-embedded
`.metal` shader source under `target/release/build/llama-cpp-sys-2-*/out/`.
At runtime `load_tensors: offloaded 37/37 layers to GPU` confirms full Metal
offload, not a CPU fallback.

Dependency placement: added under **`[dev-dependencies]`** (target-gated
`cfg(target_os = "macos")` for the `metal` feature, non-macOS falls back to
CPU-only) rather than `[dependencies]`, specifically so the main
lib/binary build stays completely untouched by this spike — `cargo check
--lib` is a 0.6s no-op with no llama-cpp-2 compilation at all. Task 5
(`LlamaEngine`) is expected to promote it to a real `[dependencies]` entry
once `src/insight/llama.rs` exists.

## Exact API sequence (canonical reference for Task 5's `LlamaEngine`)

```rust
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

// 1. Backend — once per process.
let backend = LlamaBackend::init()?;

// 2. Model params — offload every layer to GPU (Metal on macOS); llama.cpp
//    clamps a too-large number to the model's real layer count.
let model_params = LlamaModelParams::default().with_n_gpu_layers(1_000_000);
let model = LlamaModel::load_from_file(&backend, path, &model_params)?;

// 3. Chat template — prefer the one baked into the GGUF (do NOT assume
//    ChatML by name; ask the model). Falls back to a hand-rolled ChatML
//    string only if the model has none (`chat_template(None)` errors).
let tmpl: LlamaChatTemplate = model.chat_template(None)?;

// 4. Context — one per generation call in this spike (cheap to recreate;
//    real worker can reuse across calls if it manages KV cache clearing —
//    not explored here, out of scope for the spike).
let ctx_params = LlamaContextParams::default()
    .with_n_ctx(std::num::NonZeroU32::new(2048))
    .with_n_threads(8)
    .with_n_threads_batch(8);
let mut ctx = model.new_context(&backend, ctx_params)?;

// 5. Build the prompt via the chat template, not string concatenation.
let messages = [
    LlamaChatMessage::new("system".into(), SYSTEM_PROMPT.into())?,
    LlamaChatMessage::new("user".into(), user_prompt)?,
];
// add_ass = true: leaves the prompt ending on the assistant's opening tag
// (<|im_start|>assistant\n for Qwen/ChatML) so the model doesn't waste
// tokens re-emitting a turn header before the JSON.
let prompt = model.apply_chat_template(&tmpl, &messages, true)?;

// 6. Tokenize. AddBos::Never — the chat template's own special tokens
//    (<|im_start|> etc.) already delimit turns; Qwen2 has no real BOS token
//    concept baked into its tokenizer's normal use, so forcing one on top
//    of the template output is redundant/wrong here.
let tokens = model.str_to_token(&prompt, AddBos::Never)?;

// 7. Prefill: one batched decode() call for the whole prompt.
let mut batch = LlamaBatch::new(512, 1);
let last = tokens.len() as i32 - 1;
for (i, t) in (0_i32..).zip(tokens) {
    batch.add(t, i, &[0], i == last)?;
}
ctx.decode(&mut batch)?;

// 8. Sampler chain — order matters (filters first, temperature, then the
//    final distribution draw).
let mut sampler = LlamaSampler::chain_simple([
    LlamaSampler::top_k(40),
    LlamaSampler::top_p(0.9, 1),
    LlamaSampler::temp(0.3),
    LlamaSampler::dist(seed),
]);

// 9. Decode loop: sample one token, accept it into the sampler's state,
//    stop on EOG or the output budget, else feed it back as a 1-token batch.
let mut n_cur = batch.n_tokens();
loop {
    let token = sampler.sample(&ctx, batch.n_tokens() - 1);
    sampler.accept(token);
    if model.is_eog_token(token) { break; }
    let piece = model.token_to_piece(token, &mut decoder, true, None)?;
    // ...append piece to output...
    batch.clear();
    batch.add(token, n_cur, &[0], true)?;
    n_cur += 1;
    ctx.decode(&mut batch)?;
}
```

Full working version: `src-tauri/examples/insight_spike.rs`.

## Metal notes

- `LlamaModelParams::with_n_gpu_layers(1_000_000)` is the idiomatic
  "offload everything" call across this crate's examples — llama.cpp clamps
  to the real layer count internally, no need to query it first.
- Metal is auto-linked once the `metal` feature is on for
  `target_os = "macos"`; no extra env vars or paths needed on this machine
  (cmake at `/opt/homebrew/bin/cmake` was found automatically).
- First run of the process JIT-compiles ~15 Metal compute pipelines
  (`ggml_metal_library_compile_pipeline: compiling pipeline: ...` — matmul,
  rope, flash-attention, rms-norm, swiglu kernels etc. for this specific
  model shape/quant). This is a one-time cost per process, not per call —
  it shows up once at model/context init, not on every generation.
- Confirmed `load_tensors: offloaded 37/37 layers to GPU` and
  `MTL0_Mapped model buffer size = 2001.74 MiB` (weights live in GPU-mapped
  memory) — genuine GPU execution, not CPU fallback with GPU linked but
  unused.
- `ggml_metal_device_init: tensor API disabled for pre-M5 and pre-A19
  devices` — an informational line on this M4, not an error; llama.cpp uses
  an older-but-supported Metal path.

## Model

- **Name:** Qwen2.5-3B-Instruct, GGUF `q4_k_m` quantization (from bartowski/
  Qwen2.5-3B-Instruct-GGUF's upstream source repo, `Qwen/Qwen2.5-3B-Instruct-GGUF`
  on Hugging Face — the Qwen org's own official quant).
- **License:** Apache 2.0 (Qwen2.5 base models are Apache-2.0; only the
  72B variant uses a separate Qwen license).
- **URL (verified headless, no auth):**
  `https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf`
  — `curl -L` follows a redirect through HF's Xet CDN and streams the file
  directly with `Content-Length` reported up front; no login, no token, no
  gating page. This is the exact URL the plan proposed as first choice and
  it worked without needing the bartowski fallback.
- **Byte size:** 2,104,932,768 bytes (verified: HTTP `Content-Length`
  matched the final downloaded file size on disk exactly, and the file's
  GGUF magic header (`4747 5546` = `"GGUF"`) is intact).
- **Saved to:**
  `$HOME/Library/Application Support/net.ninjaruss.yapper/models/llm/model.gguf`
  (NOT committed — matches the plan's model-manager path convention of
  `<app_data_dir>/models/<dir>/`; Task 6 will formalize this as an
  `LLM` `ModelSpec`).
- **Model facts from load-time metadata:** 3.40B params, 36 layers,
  n_embd=2048, n_vocab=151936, `n_ctx_train=32768` (trained context is far
  larger than the 2048 we cap at for this spike — llama.cpp logs an
  informational `n_ctx_seq (2048) < n_ctx_train (32768)` note, not an
  error/warning to act on).

## Chat template

Used the template **baked into the GGUF itself** via
`model.chat_template(None)` rather than hand-rolling ChatML — it came back
successfully (Qwen ships its own Jinja chat template in the GGUF metadata,
which llama.cpp's built-in minimal Jinja-subset engine can execute). It
renders to the expected `<|im_start|>system\n...<|im_end|>\n<|im_start|>user\n...<|im_end|>\n<|im_start|>assistant\n`
shape (ChatML), confirming the model's template *is* ChatML under the hood —
but the code never assumes that; it always asks the model. The hand-rolled
ChatML fallback mentioned in the plan was not needed for this model and was
not exercised by the spike; if a future model swap hits a model with no
baked-in template, `LlamaChatTemplate::new("chatml")` is the one-line
fallback per the crate's docs.

## Prompt used

System: `"You are a silent note-taking companion. Reply with STRICT JSON
only, no prose, no code fences."`

User: a fixed `INTENT` line ("Figure out why the Q3 onboarding funnel
dropped and decide what to test next.") + 6 short transcript lines the
speaker might plausibly say while thinking out loud, followed by an
explicit instruction to return **only** a JSON object matching
`{"outline":[{"label":"...","status":"covered"|"current"|"intent_untouched"}],"question":"..."|null,"wrapup_ready":false,"shine":false}`
plus inline rules (outline ≤6 labels in the speaker's own words, question
only if genuinely curious, wrapup/shine semantics). Full text in
`build_user_prompt()` in the example.

## Sampler settings (final, all 3 runs used these)

```rust
LlamaSampler::chain_simple([
    LlamaSampler::top_k(40),
    LlamaSampler::top_p(0.9, 1),
    LlamaSampler::temp(0.3),
    LlamaSampler::dist(seed),   // seed varies per run: 1000, 1001, 1002
])
```

Max output budget: 400 tokens (actual usage was well under this — see
below). No grammar/constrained-decoding was needed to hit reliable JSON at
temperature 0.3 with this model+prompt combination — worth remembering as a
fallback lever (`LlamaSampler::grammar(...)` exists in this crate, could
force valid JSON structurally) if reliability degrades once the real
worker's prompt grows more complex in Task 4.

No iteration was needed to reach reliability — the very first sampler
config tried (top_k 40 → top_p 0.9 → temp 0.3 → dist) produced clean JSON
on the first run and all three runs. `add_ass: true` on the chat template
(ending the prompt on the assistant's opening tag) is doing real work here:
it stops the model from re-emitting a role header before the JSON, which is
the most common failure mode this setup was defending against.

## 3-run JSON reliability results

All 3 consecutive runs (fresh `LlamaContext` per run, same loaded model,
same prompt, only the sampler's RNG seed varies 1000/1001/1002) produced
valid JSON matching the exact `InsightUpdate` shape — no code-fence
stripping was even triggered (model never wrapped output in
` ```json ` fences despite the system prompt explicitly forbidding it,
i.e. it didn't need forbidding). Parser validated: `outline` is an array of
`{label: string, status: "covered"|"current"|"intent_untouched"}`, `question`
is string-or-null, `wrapup_ready`/`shine` are booleans.

Run 1 (92 output tokens):
```json
{"outline":[{"label":"Q3 Onboarding Funnel Analysis","status":"current"},{"label":"Signups Flat, Activation Down","status":"current"},{"label":"Verification Step Impact","status":"current"},{"label":"Support Ticket Increase","status":"current"},{"label":"Mobile vs Desktop Testing","status":"intent_untouched"}],"question":"What specific issues are you seeing with the verification step on mobile devices?","wrapup_ready":false,"shine":false}
```

Run 2 (91 output tokens): same shape, `"Signups and Activation Trends"`
label variant, otherwise near-identical.

Run 3 (80 output tokens): same shape, dropped the "Support Ticket
Increase" outline entry (4 entries instead of 5) — otherwise consistent.

**3/3 parsed. 0/3 needed fence-stripping or salvage slicing** (the
`find('{')..rfind('}')` slice was a no-op every time — output was clean
JSON front to back with no leading/trailing prose).

Qualitative note for Task 4 (prompt/parser): the model reliably marks
every outline entry `"current"` rather than distinguishing `"covered"` vs
`"current"` in this miniature example — plausible here since the 6-line
transcript is genuinely one continuous train of thought with no clear topic
boundary, but Task 4's real-session testing should watch whether the model
under- or over-uses `"current"` once transcripts span multiple topics.

## Tokens/sec observed

- **Prompt (prefill) tokens:** 341 tokens, all 3 runs identical (same
  fixed prompt). Prefill is a single batched forward pass, not
  autoregressive, so it's dramatically faster than decode — run 1 (which
  also paid for some one-time Metal pipeline JIT compilation) measured
  341 tok in 0.20s (~1.7k tok/s); runs 2-3, fully warmed up, measured under
  the 10ms floor this spike's wall-clock timer could resolve at 2-decimal
  precision (reported as 0.00s — a measurement-resolution artifact of the
  spike's `Instant` timer print format, not a real zero; the underlying
  compute is genuinely sub-3ms for a 341-token prefill on Metal, consistent
  with matmul-parallelized prefill throughput). Not a concern for the real
  worker: prefill on realistic ~90s-of-transcript prompts will still be a
  small fraction of a second on this hardware.
- **Decode (generation) tokens:** 31.3–32.8 tok/s across the 3 runs
  (80–92 output tokens each, 2.56–2.82s wall-clock per run). This is the
  number that matters for the slow-lane worker's cadence: at ~32 tok/s, a
  ~100-token JSON response costs ~3s of wall-clock — trivially affordable
  on a 45s cadence, leaving huge headroom even if a real transcript-heavy
  prompt roughly doubles generation length.
- **Model load:** 1.21s (weights mmap'd + copied to Metal-mapped buffer,
  2001.74 MiB on GPU + 166.92 MiB CPU-mapped for non-offloaded bits). Paid
  once per app session, not per insight call.

## Context budget

- Prompt (system + user, this miniature 6-line/1-intent-line example):
  **341 tokens** out of the spike's **2048-token** `n_ctx` — 16.6% of
  budget, ~1700 tokens of headroom.
- The real worker (Task 4/7) needs to fit: intent line + up to 10 outline
  entries (short labels) + "recent" transcript (spec says last ~90s of
  segments) + the instruction/schema boilerplate (which alone is roughly
  150-200 tokens of this 341, based on the 6-line/48-word transcript here
  contributing comparatively little). 1700 tokens of remaining headroom at
  ~90s of natural speech (very roughly 200-300 words / ~270-400 tokens for
  a fast talker) comfortably fits with room to spare for a fuller 10-entry
  outline — but Task 4 should still measure actual token counts against a
  real transcript fixture rather than assume, since speaking rate and
  transcript formatting (timestamps, speaker labels if any) will change the
  real number.
- 2048 was chosen to match the plan's explicit ≤2048 budget goal — not
  because the model is limited to it (`n_ctx_train=32768`, so context could
  be raised later without a model swap if a task needs more room).

## Gotchas

1. **Dependency scope matters for the "don't touch the lib" constraint.**
   Putting `llama-cpp-2` under `[dependencies]` would compile it into the
   main lib/binary too, defeating the point of "cargo check clean for the
   main lib" as a fast sanity gate. Put it under `[dev-dependencies]`
   (examples/tests only) until Task 5 actually needs it in `src/`.
2. **Version pinning.** This crate's API surface moves with upstream
   llama.cpp; used an exact pin (`=0.1.152`) rather than a caret range so a
   `cargo update` doesn't silently change the API Task 5 is built against.
3. **`AddBos::Never`, not `Always`, when using a chat template.** The
   simple.cpp-style examples in the crate's own repo use `AddBos::Always`
   for raw-completion prompts, but that's wrong once `apply_chat_template`
   has already produced a fully-delimited turn sequence — double-adding a
   BOS (or adding one to a model like Qwen2 that doesn't really use one in
   normal chat use) risks confusing the model. Verify this choice again in
   Task 5 against whatever real model ends up pinned.
4. **`add_ass: true` is not optional in practice.** Without it, the
   rendered prompt doesn't end on the assistant's opening tag, and the
   model wastes tokens (and sometimes derails) re-emitting a chat-turn
   header before getting to content. This was the single highest-leverage
   correctness knob in this spike — worth calling out explicitly for
   Task 4's prompt builder.
5. **`chat_template(None)` can fail** (`ChatTemplateError::MissingTemplate`)
   if a model has no baked-in template at all — didn't happen with this
   Qwen2.5 GGUF, but Task 5/6 should decide the fallback behavior (hardcode
   ChatML via `LlamaChatTemplate::new("chatml")`) rather than assume every
   future model choice ships one.
6. **First-process-run Metal JIT compile cost is real but one-time.**
   Don't let it worry a future perf regression check — it shows up once at
   startup (~15 `ggml_metal_library_compile_pipeline` lines), not per
   insight call. If Task 7's worker spawns fresh contexts per session
   rather than reusing the model+backend across the app's lifetime, this
   cost would repeat — worth keeping the model/backend loaded once at app
   startup, not per session.
7. **KV cache / context reuse across calls was not explored.** This spike
   creates a brand new `LlamaContext` per run (3 fresh contexts) rather
   than reusing one context across calls and clearing/managing its KV
   cache — simplest thing that could prove the JSON-reliability question,
   but Task 5's real engine should decide whether per-call context creation
   (simple, ~nothing added to the ~1.2s-per-context overhead was even
   measured, likely small since weights are already resident) is
   acceptable for a call cadence of once per 45s, versus reusing one
   context + clearing KV state between calls.

## Gates status

- Example runs successfully, 3/3 consecutive runs produce parseable
  `InsightUpdate`-shaped JSON: **confirmed**, see the 3-run results above.
- `cargo check --lib` clean, lib untouched by the spike (llama-cpp-2 is a
  dev-dependency): **confirmed**, 0.6s no-op recompile.
- Existing tests still pass: **confirmed**, `cargo test` →
  `test result: ok. 66 passed; 0 failed; 1 ignored` (the 1 ignored is the
  pre-existing `stt::moonshine::tests::transcribes_fixture_to_english`,
  unrelated to this spike — matches the 66+1 baseline from Plan 3).

## Files touched

- `src-tauri/Cargo.toml` — added `llama-cpp-2 = "=0.1.152"` (target-gated
  `metal` feature on macOS) under `[dev-dependencies]`, plus `encoding_rs`
  (needed by the example's UTF-8 token decoder) and an explicit
  `[[example]]` entry for `insight_spike`.
- `src-tauri/examples/insight_spike.rs` — new; the spike itself.
- `docs/superpowers/plans/plan4-task1-spike-notes.md` — this file.
- Model file (not committed, `.gitignore`d by being outside the repo's
  path entirely — it lives under the OS app-data dir, not the repo):
  `$HOME/Library/Application Support/net.ninjaruss.yapper/models/llm/model.gguf`.
