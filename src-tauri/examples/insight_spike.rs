//! Spike: prove a local llama.cpp model (Metal, in-process) answers Yapper's
//! insight prompt in reliably parseable JSON. Canonical API reference for
//! insight/llama.rs (Task 5) and insight/prompt.rs (Task 4).
//!
//! Run: cargo run --release --example insight_spike
//! Model path defaults to
//! `$HOME/Library/Application Support/net.ninjaruss.yapper/models/llm/model.gguf`
//! — override with the `YAPPER_LLM_MODEL` env var or a CLI arg.
//!
//! Runs the same miniature insight request 3 times back to back (fresh
//! context each run, same model load) and reports whether the raw output
//! parses as the InsightUpdate JSON shape each time, plus tokens/sec.

use std::path::PathBuf;
use std::time::Instant;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

/// Mirrors the real `insight/mod.rs::InsightEngine` system prompt intent.
const SYSTEM_PROMPT: &str =
    "You are a silent note-taking companion. Reply with STRICT JSON only, no prose, no code fences.";

/// A miniature stand-in for a real session: what the speaker set out to
/// figure out, plus ~6 short transcript lines of them thinking out loud.
const INTENT: &str =
    "Figure out why the Q3 onboarding funnel dropped and decide what to test next.";

const TRANSCRIPT_LINES: &[&str] = &[
    "Okay so I pulled the funnel numbers again this morning.",
    "Signups are flat but activation dropped about twelve percent in September.",
    "I think it's the new verification step we added, people are bouncing there.",
    "Support tickets about the verification email spiked around the same time.",
    "I haven't looked at whether it's worse on mobile versus desktop though.",
    "Next step is probably to split the funnel by device and see.",
];

/// Output token budget — the real worker asks for a short outline + one
/// optional question, so this should be generous headroom, not a target.
const MAX_OUTPUT_TOKENS: i32 = 400;

/// Context window budget the real prompt builder (Task 4) must respect.
const N_CTX: u32 = 2048;

fn build_user_prompt() -> String {
    let transcript = TRANSCRIPT_LINES
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{}. {l}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "INTENT: {INTENT}\n\n\
         RECENT TRANSCRIPT:\n{transcript}\n\n\
         Return ONLY a JSON object with this exact shape, no other text:\n\
         {{\"outline\":[{{\"label\":\"...\",\"status\":\"covered\"|\"current\"|\"intent_untouched\"}}],\
         \"question\":\"...\"|null,\"wrapup_ready\":false,\"shine\":false}}\n\n\
         Rules: outline is at most 6 short topic labels in the speaker's own words \
         (present tense, a few words each), covering what has been said so far plus \
         any part of the intent not yet touched (status \"intent_untouched\"). The \
         topic being discussed right now is \"current\"; earlier topics are \"covered\". \
         question is a short curious-listener follow-up in the speaker's own vocabulary, \
         or null if nothing is genuinely worth asking — never an interviewer question, \
         never something already covered. wrapup_ready is true only if the speaker \
         sounds like they are winding down. shine is true only if the last stretch \
         went notably deep or personal. Output strict JSON matching that shape exactly."
    )
}

/// Lenient parse identical in spirit to the real `insight/prompt.rs::parse_update`:
/// strip code fences, slice from the first `{` to the last `}`, then parse.
fn try_parse_insight_json(raw: &str) -> Result<serde_json::Value, String> {
    let stripped = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let start = stripped.find('{').ok_or("no '{' found")?;
    let end = stripped.rfind('}').ok_or("no '}' found")?;
    if end < start {
        return Err("'}' before '{'".into());
    }
    let slice = &stripped[start..=end];
    let value: serde_json::Value = serde_json::from_str(slice).map_err(|e| e.to_string())?;

    // Shape-check against the InsightUpdate schema (mirrors Task 2's struct).
    let outline = value
        .get("outline")
        .and_then(|v| v.as_array())
        .ok_or("missing/invalid 'outline' array")?;
    for entry in outline {
        let label = entry.get("label").and_then(|v| v.as_str());
        let status = entry.get("status").and_then(|v| v.as_str());
        if label.is_none() {
            return Err("outline entry missing string 'label'".into());
        }
        match status {
            Some("covered") | Some("current") | Some("intent_untouched") => {}
            other => return Err(format!("outline entry has invalid status: {other:?}")),
        }
    }
    if !value
        .get("question")
        .map(|v| v.is_string() || v.is_null())
        .unwrap_or(false)
    {
        return Err("missing/invalid 'question' (must be string or null)".into());
    }
    if !value
        .get("wrapup_ready")
        .map(|v| v.is_boolean())
        .unwrap_or(false)
    {
        return Err("missing/invalid boolean 'wrapup_ready'".into());
    }
    if !value.get("shine").map(|v| v.is_boolean()).unwrap_or(false) {
        return Err("missing/invalid boolean 'shine'".into());
    }
    Ok(value)
}

fn model_path() -> PathBuf {
    if let Ok(p) = std::env::var("YAPPER_LLM_MODEL") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::args().nth(1) {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home)
        .join("Library/Application Support/net.ninjaruss.yapper/models/llm/model.gguf")
}

struct RunResult {
    raw: String,
    prompt_tokens: i32,
    output_tokens: i32,
    prompt_secs: f32,
    decode_secs: f32,
    parse_result: Result<serde_json::Value, String>,
}

fn run_once(
    backend: &LlamaBackend,
    model: &LlamaModel,
    tmpl: &LlamaChatTemplate,
    run_index: usize,
) -> RunResult {
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(N_CTX))
        .with_n_threads(8)
        .with_n_threads_batch(8);
    let mut ctx = model
        .new_context(backend, ctx_params)
        .expect("failed to create llama context");

    let messages = [
        LlamaChatMessage::new("system".to_string(), SYSTEM_PROMPT.to_string()).unwrap(),
        LlamaChatMessage::new("user".to_string(), build_user_prompt()).unwrap(),
    ];
    // add_ass = true so the prompt ends with the assistant's opening tag —
    // without it the model tends to re-emit a chat-turn header before the JSON.
    let prompt = model
        .apply_chat_template(tmpl, &messages, true)
        .expect("apply_chat_template failed");

    // The template's own special tokens (e.g. <|im_start|>) already delimit
    // turns, so we do not add another BOS on top of it.
    let tokens = model
        .str_to_token(&prompt, AddBos::Never)
        .expect("tokenize failed");
    let prompt_tokens = tokens.len() as i32;
    assert!(
        prompt_tokens < N_CTX as i32 - MAX_OUTPUT_TOKENS,
        "prompt ({prompt_tokens} tok) leaves too little room for {MAX_OUTPUT_TOKENS} output tokens in a {N_CTX}-token context"
    );

    let mut batch = LlamaBatch::new(512, 1);
    let last_index = tokens.len() as i32 - 1;
    for (i, token) in (0_i32..).zip(tokens) {
        batch.add(token, i, &[0], i == last_index).unwrap();
    }

    let prompt_start = Instant::now();
    ctx.decode(&mut batch).expect("prefill decode failed");
    let prompt_secs = prompt_start.elapsed().as_secs_f32();

    // Low temperature + top_p per plan; top_k as a cheap upstream filter.
    // seeded per-run so 3 runs aren't just replaying the same sample path.
    let seed = 1000 + run_index as u32;
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

    let decode_start = Instant::now();
    while output_tokens < MAX_OUTPUT_TOKENS {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);

        if model.is_eog_token(token) {
            break;
        }

        let piece = model
            .token_to_piece(token, &mut decoder, true, None)
            .expect("token_to_piece failed");
        raw.push_str(&piece);

        batch.clear();
        batch.add(token, n_cur, &[0], true).unwrap();
        n_cur += 1;
        output_tokens += 1;

        ctx.decode(&mut batch).expect("decode failed");
    }
    let decode_secs = decode_start.elapsed().as_secs_f32();

    let parse_result = try_parse_insight_json(&raw);

    RunResult {
        raw,
        prompt_tokens,
        output_tokens,
        prompt_secs,
        decode_secs,
        parse_result,
    }
}

fn main() {
    let path = model_path();
    assert!(
        path.exists(),
        "model not found at {path:?} — download it first (see spike notes)"
    );

    println!("loading model: {path:?}");
    let backend = LlamaBackend::init().expect("backend init failed");

    // n_gpu_layers large enough to offload every layer of a ~3B model to
    // Metal; llama.cpp clamps to the model's actual layer count.
    let model_params = LlamaModelParams::default().with_n_gpu_layers(1_000_000);
    let load_start = Instant::now();
    let model =
        LlamaModel::load_from_file(&backend, &path, &model_params).expect("model load failed");
    println!("model loaded in {:.2?}", load_start.elapsed());

    let tmpl = model
        .chat_template(None)
        .expect("model has no baked-in chat template — would need hand-rolled ChatML fallback");
    println!(
        "chat template (baked into gguf): {:?}",
        tmpl.to_string().unwrap()
    );

    let mut all_ok = true;
    for run in 0..3 {
        println!("\n=== run {} ===", run + 1);
        let result = run_once(&backend, &model, &tmpl, run);

        println!("--- raw output ---\n{}\n--- end raw ---", result.raw);
        println!(
            "prompt_tokens={} output_tokens={} prompt_time={:.2}s decode_time={:.2}s ({:.1} tok/s decode, {:.1} tok/s prefill)",
            result.prompt_tokens,
            result.output_tokens,
            result.prompt_secs,
            result.decode_secs,
            result.output_tokens as f32 / result.decode_secs.max(0.001),
            result.prompt_tokens as f32 / result.prompt_secs.max(0.001),
        );
        match &result.parse_result {
            Ok(v) => println!("PARSE OK: {v}"),
            Err(e) => {
                all_ok = false;
                println!("PARSE FAILED: {e}");
            }
        }
    }

    println!("\n=== summary ===");
    if all_ok {
        println!("all 3 runs produced parseable InsightUpdate-shaped JSON.");
    } else {
        println!("at least one run failed to parse — see PARSE FAILED lines above.");
        std::process::exit(1);
    }
}
