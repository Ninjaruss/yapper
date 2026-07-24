//! LlamaEngine: the real local LLM behind `InsightEngine`, backed by
//! `llama-cpp-2` (in-process llama.cpp bindings, Metal-accelerated on
//! macOS). API sequence pinned by the Task 1 spike
//! (`docs/superpowers/plans/plan4-task1-spike-notes.md`,
//! `src-tauri/examples/insight_spike.rs`) — follow the spike notes over
//! this file if the crate's API ever shifts underneath us.

use std::num::NonZeroU32;
use std::path::Path;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

use crate::error::YapperError;
use crate::insight::InsightEngine;

/// The single model file this engine expects inside its model directory.
pub const LLM_MODEL_FILE: &str = "model.gguf";

/// System message paired with `build_prompt`'s output. `build_prompt`
/// (prompt.rs) already opens with a full instruction block — its own
/// "You are a silent note-taking companion... Reply with STRICT JSON
/// only..." header plus the schema, rules, and data. Duplicating that as a
/// separate system message would risk drift between two copies of the same
/// instruction (and Task 4's tests assert on the exact wording living in
/// `build_prompt`). So `build_prompt`'s full output is sent as the single
/// user message, and this system message is deliberately a short,
/// non-conflicting reinforcement of format only — it must never restate or
/// contradict any of `build_prompt`'s content rules (outline/question/
/// wrapup/shine semantics), only the "reply with JSON, nothing else"
/// framing shared by both.
const SYSTEM_PROMPT: &str = "You are a silent note-taking companion. Reply with STRICT JSON only, no prose, no code fences.";

/// Context window budget — matches the Task 1 spike's chosen budget (the
/// model's trained context is far larger; this is a deliberate cap, not a
/// model limitation). `build_prompt`'s output must fit comfortably inside
/// this minus `MAX_OUTPUT_TOKENS`.
const N_CTX: u32 = 2048;

/// Output token budget per insight call — generous headroom over the
/// ~80-100 tokens observed in the Task 1 spike for a similarly-shaped
/// response.
const MAX_OUTPUT_TOKENS: i32 = 400;

/// Local LLM engine. Holds the backend and loaded model for the lifetime of
/// the session; a fresh `LlamaContext` is created per `insight()` call.
///
/// Fresh-context-per-call is acceptable at the slow lane's ~45s call
/// cadence (per the Task 1 spike's gotcha #7 — context creation cost is
/// small since model weights are already resident). KV cache reuse across
/// calls is a possible future optimization, not attempted here.
/// The llama.cpp backend is a PROCESS-GLOBAL singleton: `LlamaBackend::init`
/// errors with `BackendAlreadyInitialized` on any second call. The engine is
/// rebuilt per session, so the backend must live in a `OnceLock` shared by
/// every engine — without this, every session after the first silently lost
/// insight ("thinking model is off"). Regression-guarded by the ignored
/// `llama_engine_survives_reconstruction` test.
static LLAMA_BACKEND: std::sync::OnceLock<LlamaBackend> = std::sync::OnceLock::new();
static BACKEND_INIT_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn global_backend() -> Result<&'static LlamaBackend, YapperError> {
    if let Some(b) = LLAMA_BACKEND.get() {
        return Ok(b);
    }
    // Serialize first-time init so concurrent engine constructions can't
    // both call LlamaBackend::init (the loser would error spuriously).
    let _guard = BACKEND_INIT_GUARD
        .lock()
        .map_err(|_| YapperError::State("llm backend init guard poisoned".into()))?;
    if let Some(b) = LLAMA_BACKEND.get() {
        return Ok(b);
    }
    let backend = LlamaBackend::init()
        .map_err(|e| YapperError::Audio(format!("llm init: backend init: {e}")))?;
    Ok(LLAMA_BACKEND.get_or_init(|| backend))
}

pub struct LlamaEngine {
    backend: &'static LlamaBackend,
    model: LlamaModel,
    chat_template: LlamaChatTemplate,
}

impl LlamaEngine {
    /// Loads `<model_dir>/model.gguf`, offloading every layer to GPU on
    /// macOS (Metal) / CPU elsewhere (llama.cpp clamps an oversized
    /// `n_gpu_layers` to the model's real layer count).
    pub fn new(model_dir: &Path) -> Result<Self, YapperError> {
        let path = model_dir.join(LLM_MODEL_FILE);

        let backend = global_backend()?;

        let model_params = LlamaModelParams::default().with_n_gpu_layers(1_000_000);
        let model = LlamaModel::load_from_file(backend, &path, &model_params)
            .map_err(|e| YapperError::Audio(format!("llm init: model load: {e}")))?;

        // Prefer the chat template baked into the GGUF; only a model with
        // no baked-in template at all falls back to hand-rolled ChatML.
        let chat_template = model.chat_template(None).or_else(|_| {
            LlamaChatTemplate::new("chatml")
                .map_err(|e| YapperError::Audio(format!("llm init: chat template: {e}")))
        })?;

        Ok(Self {
            backend,
            model,
            chat_template,
        })
    }
}

impl InsightEngine for LlamaEngine {
    fn insight(&mut self, prompt: &str) -> Result<String, YapperError> {
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(N_CTX))
            .with_n_threads(8)
            .with_n_threads_batch(8);
        let mut ctx = self
            .model
            .new_context(self.backend, ctx_params)
            .map_err(|e| YapperError::Audio(format!("llm context: {e}")))?;

        let messages = [
            LlamaChatMessage::new("system".to_string(), SYSTEM_PROMPT.to_string())
                .map_err(|e| YapperError::Audio(format!("llm chat message: {e}")))?,
            LlamaChatMessage::new("user".to_string(), prompt.to_string())
                .map_err(|e| YapperError::Audio(format!("llm chat message: {e}")))?,
        ];
        // add_ass = true: leaves the prompt ending on the assistant's
        // opening tag so the model doesn't waste tokens re-emitting a turn
        // header before the JSON (Task 1 spike gotcha #4).
        let rendered = self
            .model
            .apply_chat_template(&self.chat_template, &messages, true)
            .map_err(|e| YapperError::Audio(format!("llm apply_chat_template: {e}")))?;

        // The chat template's own special tokens already delimit turns;
        // AddBos::Never avoids double-adding a BOS on top of that.
        let tokens = self
            .model
            .str_to_token(&rendered, AddBos::Never)
            .map_err(|e| YapperError::Audio(format!("llm tokenize: {e}")))?;

        if tokens.len() as i32 > N_CTX as i32 - MAX_OUTPUT_TOKENS {
            return Err(YapperError::Audio(format!(
                "llm prompt too long: {} tokens leaves too little room for {MAX_OUTPUT_TOKENS} output tokens in a {N_CTX}-token context",
                tokens.len()
            )));
        }

        // Batch capacity must cover the whole prompt in one go (this is a
        // single-shot prefill, not a per-step limit) — size it to the full
        // context window rather than the Task 1 spike's 512, since
        // `build_prompt`'s real instruction block (schema + rules text) is
        // longer than the spike's miniature prompt and can exceed 512
        // tokens on its own.
        let mut batch = LlamaBatch::new(N_CTX as usize, 1);
        let last_index = tokens.len() as i32 - 1;
        for (i, token) in (0_i32..).zip(tokens) {
            batch
                .add(token, i, &[0], i == last_index)
                .map_err(|e| YapperError::Audio(format!("llm batch: {e}")))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| YapperError::Audio(format!("llm prefill decode: {e}")))?;

        // Sampler chain per Task 1 spike: filters first, temperature, then
        // the final distribution draw. Seeded from the system clock so
        // repeated calls within a session don't replay the same path.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u32)
            .unwrap_or(0);
        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.9, 1),
            LlamaSampler::temp(0.3),
            LlamaSampler::dist(seed),
        ]);

        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut raw = String::new();
        let mut n_cur = batch.n_tokens();
        let mut output_tokens = 0;

        while output_tokens < MAX_OUTPUT_TOKENS {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);

            if self.model.is_eog_token(token) {
                break;
            }

            let piece = self
                .model
                .token_to_piece(token, &mut decoder, true, None)
                .map_err(|e| YapperError::Audio(format!("llm token_to_piece: {e}")))?;
            raw.push_str(&piece);

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| YapperError::Audio(format!("llm batch: {e}")))?;
            n_cur += 1;
            output_tokens += 1;

            ctx.decode(&mut batch)
                .map_err(|e| YapperError::Audio(format!("llm decode: {e}")))?;
        }

        Ok(raw)
    }
}

#[cfg(test)]
mod tests {
    // Regression: the backend is process-global; constructing a SECOND
    // engine used to fail with BackendAlreadyInitialized (every session
    // after the first lost insight).
    #[test]
    #[ignore = "needs downloaded llm model; run manually"]
    fn llama_engine_survives_reconstruction() {
        let dir = model_dir();
        let first = LlamaEngine::new(&dir).expect("first engine");
        drop(first);
        LlamaEngine::new(&dir).expect("second engine must init (global backend)");
    }

    use super::*;

    fn model_dir() -> std::path::PathBuf {
        let home = std::env::var("HOME").expect("HOME env var required");
        std::path::PathBuf::from(home)
            .join("Library/Application Support/net.ninjaruss.yapper/models/llm")
    }

    #[test]
    #[ignore = "needs downloaded llm model; run manually"]
    fn llama_engine_answers_parseable_json() {
        use crate::insight::prompt::build_prompt;
        use crate::insight::{OutlineEntry, OutlineStatus};

        let outline = vec![
            OutlineEntry {
                label: "Q3 funnel drop".to_string(),
                status: OutlineStatus::Current,
            },
            OutlineEntry {
                label: "Mobile testing".to_string(),
                status: OutlineStatus::IntentUntouched,
            },
        ];
        let recent = vec![
            (
                0,
                "Okay so I pulled the funnel numbers again this morning.".to_string(),
            ),
            (
                5_000,
                "Signups are flat but activation dropped about twelve percent in September."
                    .to_string(),
            ),
            (
                12_000,
                "I think it's the new verification step we added, people are bouncing there."
                    .to_string(),
            ),
            (
                19_000,
                "Support tickets about the verification email spiked around the same time."
                    .to_string(),
            ),
            (
                26_000,
                "I haven't looked at whether it's worse on mobile versus desktop though."
                    .to_string(),
            ),
            (
                33_000,
                "Next step is probably to split the funnel by device and see.".to_string(),
            ),
        ];
        let prompt = build_prompt(
            "Figure out why the Q3 onboarding funnel dropped and decide what to test next.",
            &outline,
            &recent,
            40_000,
        );

        let mut engine = LlamaEngine::new(&model_dir()).expect("engine init");

        let start = std::time::Instant::now();
        let raw = engine.insight(&prompt).expect("insight call");
        let elapsed = start.elapsed();

        println!("--- raw output ({elapsed:.2?}) ---\n{raw}\n--- end raw ---");

        let update = crate::insight::prompt::parse_update(&raw, None);
        assert!(update.is_some(), "expected parseable JSON, got: {raw}");
        let update = update.unwrap();
        assert!(!update.outline.is_empty(), "expected a non-empty outline");
    }
}
