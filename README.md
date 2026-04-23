# pilaf-substrategy

**R51 — Pilaf layer-3 sub-strategy evolution (2026-04-24).**

Rust crate that turns an ACPrompt attention queue (from `acp/src/lib/pilaf-brain.ts`)
into a ranked decision plan for Pilaf's MCP session to consume.

## Architecture

Per R46.0 D9 lock, Pilaf has three layers:

| Layer | What | Where | Mutability |
|-------|------|-------|------------|
| 1. Core Identity | Personality, speech rules, reserved names | `acp/src/lib/system-agents.ts` | **immutable** (R46.0 D7) |
| 2. Strategy | Signal detection → attention queue | `acp/src/lib/pilaf-brain.ts` | evolves with caution |
| 3. Sub-strategy | Decision scoring + action suggestion | **this crate** | **evolves freely** |

Layer 3 is allowed to rewrite itself as long as the IO contract holds.

## IO contract

```
stdin:  AttentionQueue JSON   (shape: acp/src/lib/pilaf-brain.ts)
stdout: SubstrategyDecision JSON (shape: src/lib.rs → SubstrategyDecision)
stderr: free-form logs (TS client tails them but ignores them)
exit:   0 on success; 1/2/3/4 on stdin read / parse / serialize / write failures
```

## Build

```bash
cargo build --release
# binary: target/release/pilaf-substrategy
```

## Dev

```bash
cargo test
```

## TS integration

`acp/src/lib/pilaf-substrategy-client.ts` calls this binary via
`child_process.spawn`. If the binary is missing or errors, the TS
client falls back to a pure-JS default heuristic so production stays
up. Deploy this crate separately; acp never bundles it.
