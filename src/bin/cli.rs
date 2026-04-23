//! R51 CLI entry point: reads AttentionQueue JSON from stdin, writes
//! SubstrategyDecision JSON to stdout. Never logs to stdout — logs go
//! to stderr so the TS client can tail them separately.

use std::io::{self, Read, Write};

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

    let decision = pilaf_substrategy::plan(&queue);

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
