//! R51 weight-tuner binary (2026-04-25).
//!
//! Reads a feedback history JSON array from stdin or `--feedback <path>`,
//! optionally reads a current weights file via `--current <path>` (or
//! uses the built-in defaults), and emits a proposed new weights file
//! as a `TuneResult` JSON to stdout.
//!
//! Usage:
//!   pilaf-substrategy-tune                                   # stdin + defaults
//!   pilaf-substrategy-tune --feedback history.json
//!   pilaf-substrategy-tune --feedback h.json --current w.json
//!   pilaf-substrategy-tune --feedback h.json --output new.json
//!   pilaf-substrategy-tune --feedback h.json --dry-run       # print diff to stderr, nothing to stdout
//!   pilaf-substrategy-tune --feedback h.json --apply         # alias for not-dry-run (explicit)
//!
//! CI-gated self-modification flow:
//!   1. A scheduled CI job (or human operator) runs this binary in
//!      `--dry-run` mode and inspects the diff printed to stderr.
//!   2. If the diff looks sane, they re-run without `--dry-run` to
//!      emit the new weights file, open a PR, and verify the
//!      `new_checksum` in the PR body matches what the CI job
//!      re-computes from the same feedback file.
//!   3. Only after PR approval is the new weights file deployed (by
//!      setting `PILAF_SUBSTRATEGY_WEIGHTS_PATH` to its location).
//!
//! The tuner is a pure function (see `pilaf_substrategy::tune_weights`)
//! — same inputs always produce the same checksum, so CI can reproduce
//! the diff independently.

use std::io::{self, Read, Write};

struct Args {
    feedback_path: Option<String>,
    current_path: Option<String>,
    output_path: Option<String>,
    dry_run: bool,
    show_help: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut out = Args {
        feedback_path: None,
        current_path: None,
        output_path: None,
        dry_run: false,
        show_help: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--feedback" => {
                out.feedback_path = Some(
                    args.next()
                        .ok_or("--feedback requires a path argument".to_string())?,
                );
            }
            "--current" => {
                out.current_path = Some(
                    args.next()
                        .ok_or("--current requires a path argument".to_string())?,
                );
            }
            "--output" | "-o" => {
                out.output_path = Some(
                    args.next()
                        .ok_or("--output requires a path argument".to_string())?,
                );
            }
            "--dry-run" => {
                out.dry_run = true;
            }
            "--apply" => {
                out.dry_run = false;
            }
            "--help" | "-h" => {
                out.show_help = true;
            }
            other => {
                return Err(format!("unknown argument: {other}"));
            }
        }
    }
    Ok(out)
}

fn print_help() {
    eprintln!(
        "pilaf-substrategy-tune — R51 CI-gated weight tuner

Usage:
  pilaf-substrategy-tune [--feedback PATH] [--current PATH]
                         [--output PATH] [--dry-run] [--apply]

Options:
  --feedback PATH   Path to a JSON file containing Feedback[].
                    If omitted, feedback is read from stdin.
  --current PATH    Path to current weights JSON. Defaults to built-in.
  --output PATH     Write TuneResult JSON to this file instead of stdout.
  --dry-run         Print diff summary to stderr; emit nothing to stdout/file.
  --apply           Explicit opposite of --dry-run (default behaviour).
  --help, -h        Show this help.

Exit codes:
  0 = success (including --dry-run with no changes)
  1 = argument / IO error
  2 = parse error
  3 = serialize / write error
"
    );
}

fn read_file_or_stdin(path: Option<&str>) -> Result<String, String> {
    match path {
        Some(p) => std::fs::read_to_string(p).map_err(|e| format!("read {p}: {e}")),
        None => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("stdin: {e}"))?;
            Ok(buf)
        }
    }
}

fn load_current(
    path: Option<&str>,
) -> Result<pilaf_substrategy::ScoringWeights, String> {
    let p = match path {
        Some(p) => p,
        None => return Ok(pilaf_substrategy::ScoringWeights::default()),
    };
    let raw = std::fs::read_to_string(p).map_err(|e| format!("read current {p}: {e}"))?;
    let w: pilaf_substrategy::ScoringWeights =
        serde_json::from_str(&raw).map_err(|e| format!("parse current {p}: {e}"))?;
    Ok(w)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[pilaf-substrategy-tune] {e}");
            print_help();
            std::process::exit(1);
        }
    };

    if args.show_help {
        print_help();
        return;
    }

    let feedback_raw = match read_file_or_stdin(args.feedback_path.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[pilaf-substrategy-tune] feedback read failed: {e}");
            std::process::exit(1);
        }
    };

    let history: Vec<pilaf_substrategy::Feedback> = match serde_json::from_str(&feedback_raw) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[pilaf-substrategy-tune] feedback parse failed: {e}");
            std::process::exit(2);
        }
    };

    let current = match load_current(args.current_path.as_deref()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[pilaf-substrategy-tune] current weights load failed: {e}");
            std::process::exit(2);
        }
    };

    let result = pilaf_substrategy::tune_weights(&history, &current);

    // Human-readable diff summary to stderr (always — so --dry-run and
    // --apply both produce the same visible trace, CI can diff them).
    eprintln!(
        "[pilaf-substrategy-tune] feedback_rows={} changes={} old_checksum={} new_checksum={}",
        result.total_feedback_rows,
        result.changes.len(),
        result.old_checksum,
        result.new_checksum
    );
    for c in &result.changes {
        eprintln!(
            "[pilaf-substrategy-tune]   {}: {:.3} → {:.3} (Δ{:+.3}, n={}, sr={:.2}, {})",
            c.target, c.old, c.new, c.delta, c.sample_size, c.success_rate, c.reason
        );
    }

    if args.dry_run {
        eprintln!("[pilaf-substrategy-tune] --dry-run: no output emitted");
        return;
    }

    // Always emit the NEW WEIGHTS (not the full TuneResult) when writing
    // to --output or stdout, so the file is directly consumable by
    // PILAF_SUBSTRATEGY_WEIGHTS_PATH. The change log is on stderr.
    let weights_json = match serde_json::to_string_pretty(&result.new_weights) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[pilaf-substrategy-tune] serialize failed: {e}");
            std::process::exit(3);
        }
    };

    // Also emit the full result as a sidecar JSON on stdout for audit
    // trails — unless --output is set, in which case stdout gets the
    // full TuneResult and the weights go to --output.
    match args.output_path.as_deref() {
        Some(out_path) => {
            if let Err(e) = std::fs::write(out_path, format!("{weights_json}\n")) {
                eprintln!("[pilaf-substrategy-tune] write {out_path} failed: {e}");
                std::process::exit(3);
            }
            // Full TuneResult to stdout for CI capture.
            let result_json = match serde_json::to_string(&result) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[pilaf-substrategy-tune] tune-result serialize failed: {e}");
                    std::process::exit(3);
                }
            };
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            if handle.write_all(result_json.as_bytes()).is_err()
                || handle.write_all(b"\n").is_err()
            {
                std::process::exit(3);
            }
        }
        None => {
            // No --output: emit new weights to stdout so `> file` works
            // as a natural flow. CI can capture stderr for the diff log.
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            if handle.write_all(weights_json.as_bytes()).is_err()
                || handle.write_all(b"\n").is_err()
            {
                std::process::exit(3);
            }
        }
    }
}
