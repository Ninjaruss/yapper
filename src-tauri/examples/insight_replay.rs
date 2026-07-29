//! Replay a fixture transcript through the REAL insight pipeline — real
//! prompt builder, real local model, real parse + guardrails + spacing
//! gates — and print what would have been shown to the user and why, pass
//! by pass. The proof mechanism for every prompt tweak: if a change
//! doesn't look better here, it doesn't ship.
//!
//! Run: cargo run --release --example insight_replay -- fixtures/first-week-solo.txt
//! Model dir defaults to the installed app's LLM dir
//! (`$HOME/Library/Application Support/net.ninjaruss.yapper/models/llm`);
//! override with `--model /path/to/dir` (dir containing model.gguf) or the
//! `YAPPER_LLM_MODEL` env var.
//!
//! Gate constants are duplicated from insight/worker.rs (they are private
//! there); keep in sync by hand — this is dev tooling, not production.

use std::path::PathBuf;

use yapper_lib::insight::guard;
use yapper_lib::insight::llama::LlamaEngine;
use yapper_lib::insight::prompt::{build_prompt, parse_update, MAX_OUTLINE_ENTRIES};
use yapper_lib::insight::{InsightEngine, OutlineEntry};

const CADENCE_MS: i64 = 45_000;
const RECENT_WINDOW_MS: i64 = 90_000;
const QUESTION_SPACING_MS: i64 = 120_000;
const FIRST_QUESTION_MIN_ELAPSED_MS: i64 = 60_000;

fn parse_fixture(text: &str) -> (String, Vec<(i64, String)>) {
    let mut intent = String::new();
    let mut segments = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("INTENT:") {
            intent = rest.trim().to_string();
            continue;
        }
        // "[m:ss] text"
        if !line.starts_with('[') {
            continue;
        }
        let Some(close) = line.find(']') else {
            continue;
        };
        // Safe to slice at byte 1: `starts_with('[')` guarantees the first
        // byte is the single-byte ASCII '['.
        let stamp = &line[1..close];
        let text = line[close + 1..].trim().to_string();
        let Some((m, s)) = stamp.split_once(':') else {
            continue;
        };
        let (Ok(m), Ok(s)) = (m.parse::<i64>(), s.parse::<i64>()) else {
            continue;
        };
        segments.push(((m * 60 + s) * 1000, text));
    }
    (intent, segments)
}

fn fmt_mmss(ms: i64) -> String {
    format!("{}:{:02}", ms / 60_000, (ms % 60_000) / 1000)
}

fn print_outline_diff(old: &[OutlineEntry], new: &[OutlineEntry]) {
    for entry in new {
        match old.iter().find(|o| o.label == entry.label) {
            None => println!("  outline + {:?} [{:?}]", entry.label, entry.status),
            Some(o) if o.status != entry.status => {
                println!(
                    "  outline ~ {:?} [{:?} → {:?}]",
                    entry.label, o.status, entry.status
                )
            }
            Some(_) => {}
        }
    }
    for gone in old
        .iter()
        .filter(|o| !new.iter().any(|n| n.label == o.label))
    {
        println!("  outline - {:?} (dropped by model)", gone.label);
    }
}

/// Runs one insight pass against `recent`/`elapsed_ms`: build the prompt,
/// call the engine, parse the result, damp + diff-print the outline, apply
/// the question/shine/wrapup gates. Shared by the cadence loop and the
/// final flush pass so both go through identical logic — mirrors
/// `insight/worker.rs::run_insight_pass`.
#[allow(clippy::too_many_arguments)]
fn run_pass(
    engine: &mut LlamaEngine,
    intent: &str,
    outline: &mut Vec<OutlineEntry>,
    last_question: &mut Option<String>,
    last_question_at: &mut Option<i64>,
    recent: &[(i64, String)],
    elapsed_ms: i64,
) {
    let prompt = build_prompt(intent, outline, recent, elapsed_ms);
    let raw = match engine.insight(&prompt) {
        Ok(raw) => raw,
        Err(e) => {
            println!("  engine error: {e}");
            return;
        }
    };
    let Some(update) = parse_update(&raw, last_question.as_deref()) else {
        println!("  unparseable output, skipped: {raw:?}");
        return;
    };

    if !update.outline.is_empty() {
        let damped_stage = guard::damp_labels(outline, &update.outline);
        // Report renames by membership, not position — damping can dedup,
        // and the merge below reorders, so a positional zip lies.
        for inc in &update.outline {
            if !damped_stage.iter().any(|e| e.label == inc.label) {
                if let Some(cur) = outline
                    .iter()
                    .find(|c| guard::labels_match(&c.label, &inc.label))
                {
                    println!(
                        "  outline ✗ {:?} damped rename → kept {:?}",
                        inc.label, cur.label
                    );
                }
            }
        }
        let merged = guard::merge_outline(outline, &damped_stage, MAX_OUTLINE_ENTRIES);
        let finalized = guard::enforce_single_current(&merged);
        print_outline_diff(outline, &finalized);
        *outline = finalized;
    }

    match (update.question.as_deref(), update.sparked_by.as_deref()) {
        (None, _) => {}
        (Some(q), None) => println!("  question ✗ dropped (no sparked_by): {q:?}"),
        (Some(q), Some(spark)) => {
            if !guard::is_grounded(spark, recent) {
                println!("  question ✗ dropped (ungrounded {spark:?}): {q:?}");
            } else {
                let allowed = match *last_question_at {
                    Some(last) => elapsed_ms - last >= QUESTION_SPACING_MS,
                    None => elapsed_ms >= FIRST_QUESTION_MIN_ELAPSED_MS,
                };
                if !allowed {
                    println!("  question ✗ dropped (spacing gate): {q:?}");
                } else {
                    println!("  question ✓ SHOWN: {q:?}");
                    println!("           sparked_by {spark:?}");
                    *last_question = Some(q.to_string());
                    *last_question_at = Some(elapsed_ms);
                }
            }
        }
    }
    if update.shine {
        println!("  shine    vote yes");
    }
    if update.wrapup_ready {
        println!("  wrapup   vote yes (worker would also gate on elapsed + intent)");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fixture_path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .expect("usage: insight_replay <fixture.txt> [--model <dir>]");
    let model_dir = args
        .iter()
        .position(|a| a == "--model")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .or_else(|| std::env::var("YAPPER_LLM_MODEL").ok().map(PathBuf::from))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").expect("HOME not set"))
                .join("Library/Application Support/net.ninjaruss.yapper/models/llm")
        });

    let text = std::fs::read_to_string(fixture_path).expect("read fixture");
    let (intent, segments) = parse_fixture(&text);
    println!("fixture: {} segments · intent: {intent:?}", segments.len());
    println!("loading model from {} …", model_dir.display());
    let mut engine = LlamaEngine::new(&model_dir).expect("load LLM");

    let mut outline: Vec<OutlineEntry> = Vec::new();
    let mut last_question: Option<String> = None;
    let mut last_question_at: Option<i64> = None;
    let mut buffer: Vec<(i64, String)> = Vec::new();
    let mut next_pass_at = CADENCE_MS;
    // Elapsed_ms of the last pass that actually fired, so we can tell
    // whether any segment arrived after it (mirrors the worker's
    // `new_since_last_call` flag for its own final-flush-on-disconnect).
    let mut last_fired_pass_at: Option<i64> = None;

    for (start_ms, seg_text) in &segments {
        buffer.push((*start_ms, seg_text.clone()));
        if *start_ms < next_pass_at {
            continue;
        }
        next_pass_at = start_ms + CADENCE_MS;
        let elapsed_ms = *start_ms;
        let recent: Vec<(i64, String)> = buffer
            .iter()
            .filter(|(ms, _)| *ms >= elapsed_ms - RECENT_WINDOW_MS)
            .cloned()
            .collect();

        println!(
            "\n── pass · {} ──────────────────────────",
            fmt_mmss(elapsed_ms)
        );
        run_pass(
            &mut engine,
            &intent,
            &mut outline,
            &mut last_question,
            &mut last_question_at,
            &recent,
            elapsed_ms,
        );
        last_fired_pass_at = Some(elapsed_ms);
    }

    // Mirror the worker's disconnect-time final pass: if segments arrived
    // after the last cadence pass fired, replay one more pass over them so
    // the fixture's tail (anything after the last 45s boundary) isn't
    // silently dropped from the replay.
    if let Some((last_start_ms, _)) = buffer.last() {
        let has_unseen_tail = match last_fired_pass_at {
            Some(last) => *last_start_ms > last,
            None => true,
        };
        if has_unseen_tail {
            let elapsed_ms = *last_start_ms;
            let recent: Vec<(i64, String)> = buffer
                .iter()
                .filter(|(ms, _)| *ms >= elapsed_ms - RECENT_WINDOW_MS)
                .cloned()
                .collect();

            println!(
                "\n── final pass (session end) · {} ──────────────────────────",
                fmt_mmss(elapsed_ms)
            );
            run_pass(
                &mut engine,
                &intent,
                &mut outline,
                &mut last_question,
                &mut last_question_at,
                &recent,
                elapsed_ms,
            );
        }
    }

    println!("\n── final outline ─────────────────────────");
    for e in &outline {
        println!("  {:?} [{:?}]", e.label, e.status);
    }
}
