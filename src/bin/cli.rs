//! R51 CLI entry point: reads AttentionQueue JSON from stdin, writes
//! SubstrategyDecision JSON to stdout. Never logs to stdout — logs go
//! to stderr so the TS client can tail them separately.
//!
//! 2026-04-25 feedback-tuning extension:
//!   If `PILAF_SUBSTRATEGY_WEIGHTS_PATH` is set to an absolute path of
//!   a JSON file matching `ScoringWeights`, the CLI loads that file
//!   instead of using the built-in defaults. Weight loading happens
//!   ONCE at startup; the stdin/stdout contract is unchanged.
//!
//!   Any failure to load the weights file (missing, unreadable, wrong
//!   shape, wrong schema_version) falls back to built-in defaults with
//!   a warning to stderr. This keeps the crate robust — a corrupted
//!   weights file never takes layer 3 offline.

use std::io::{self, Read, Write};

const WEIGHTS_PATH_ENV: &str = "PILAF_SUBSTRATEGY_WEIGHTS_PATH";

fn main() {
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("[pilaf-substrategy] stdin read failed: {e}");
        std::process::exit(1);
    }

    let queue: pilaf_substrategy::AttentionQueue = match serde_json::from_str(&input) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("[pilaf-substrategy] stdin parse failed: {e}");
            std::process::exit(2);
        }
    };

    let (weights, weights_source) = load_weights();

    let decision = pilaf_substrategy::plan_with_weights(&queue, &weights, weights_source);

    let out = match serde_json::to_string(&decision) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[pilaf-substrategy] serialize failed: {e}");
            std::process::exit(3);
        }
    };

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    if handle.write_all(out.as_bytes()).is_err() || handle.write_all(b"\n").is_err() {
        std::process::exit(4);
    }
}

/// Returns (weights, source_tag). Source is "default" if env unset or
/// loading failed; "tuned" if a valid file was loaded from the env
/// path. The tag flows into `SubstrategyDecision.weights_source` so
/// the TS side can observe which path ran.
fn load_weights() -> (pilaf_substrategy::ScoringWeights, &'static str) {
    let path = match std::env::var(WEIGHTS_PATH_ENV) {
        Ok(p) if !p.trim().is_empty() => p,
        _ => return (pilaf_substrategy::ScoringWeights::default(), "default"),
    };

    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[pilaf-substrategy] weights file read failed ({path}): {e} — using defaults"
            );
            return (pilaf_substrategy::ScoringWeights::default(), "default");
        }
    };

    let parsed: pilaf_substrategy::ScoringWeights = match serde_json::from_str(&raw) {
        Ok(w) => w,
        Err(e) => {
            eprintln!(
                "[pilaf-substrategy] weights file parse failed ({path}): {e} — using defaults"
            );
            return (pilaf_substrategy::ScoringWeights::default(), "default");
        }
    };

    if parsed.schema_version != pilaf_substrategy::ScoringWeights::default().schema_version {
        eprintln!(
            "[pilaf-substrategy] weights schema mismatch (got {}, want {}) — using defaults",
            parsed.schema_version,
            pilaf_substrategy::ScoringWeights::default().schema_version
        );
        return (pilaf_substrategy::ScoringWeights::default(), "default");
    }

    // Defensive clamp in case a hand-edited weights file has out-of-band values.
    let mut w = parsed;
    w.clamp();
    (w, "tuned")
}
