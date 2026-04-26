# pilaf-substrategy

**R51 — Pilaf layer-3 sub-strategy evolution (2026-04-24, feedback-tuning pass 2026-04-25).**

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

## IO contract (main CLI)

```
stdin:  AttentionQueue JSON   (shape: acp/src/lib/pilaf-brain.ts)
stdout: SubstrategyDecision JSON (shape: src/lib.rs → SubstrategyDecision)
stderr: free-form logs (TS client tails them but ignores them)
exit:   0 on success; 1/2/3/4 on stdin read / parse / serialize / write failures
```

## Binaries

Two binaries ship from this crate:

- `pilaf-substrategy` — the main CLI consumed by the TS bridge. Reads
  the attention queue from stdin, emits a `SubstrategyDecision`.
- `pilaf-substrategy-tune` — the feedback tuner (2026-04-25). Reads
  recent decision outcomes (`Feedback[]`) and emits a proposed new
  `ScoringWeights` JSON. CI-gated self-modification — humans review
  the diff before the weights go live.

## Tuning weights

The scoring function is:

```
score = urgency_weight[item.urgency] * kind_weight[item.kind]
```

Both weight tables live in `ScoringWeights` (see `src/lib.rs`). Defaults
are baked into the binary; override with a JSON file pointed to by
`PILAF_SUBSTRATEGY_WEIGHTS_PATH`:

```bash
PILAF_SUBSTRATEGY_WEIGHTS_PATH=/etc/pilaf/weights.json \
  pilaf-substrategy < queue.json
```

On any weights file error (missing, wrong schema_version, malformed)
the CLI logs a warning to stderr and falls back to defaults — a
corrupted weights file never takes layer 3 offline.

## CI-gated self-modification flow

The tuner is deliberately **proposal-only**. It never edits weights in
place. The flow is:

1. The ACPrompt cron `/api/cron/pilaf-tune` (weekly) collects recent
   R50 autonomous decision outcomes and runs the equivalent TS tuner
   (see `acp/src/lib/pilaf-outcome-feedback.ts`). It writes a
   "weight tune proposal" memory row for admin review.
2. Admin inspects the proposal (changes[], old_checksum, new_checksum).
3. **(Optional cross-check.)** Operator runs the Rust tuner locally
   against the same feedback JSON to verify the same algorithm
   produces the same weight changes:

   ```bash
   # feedback.json = Feedback[] exported from the proposal
   pilaf-substrategy-tune --feedback feedback.json --dry-run
   ```

   The stderr diff should match the proposal's changes[]. (Note: the
   Rust and TS checksums use different canonicalization — each side is
   self-consistent, but cross-language checksum equality is not
   expected.)
4. If approved, operator commits the `new_weights` JSON to a file and
   updates the runtime's `PILAF_SUBSTRATEGY_WEIGHTS_PATH` to point at
   it, then redeploys.

The main CLI NEVER writes to the weights file. The tuner CLI writes
only when `--apply` is explicit (or `--dry-run` is omitted) and only
to stdout/--output — never to the path the main CLI reads from.

### Tuner safety envelope

- **Min sample size:** weight changes require ≥5 feedback rows per kind.
- **Max delta per run:** ±0.1 per weight (hard cap).
- **Clamp:** every weight clamped to `[0.1, 2.0]`.
- **Kinds only:** urgency weights are never touched by the tuner.
- **Ignored outcomes:** dropped from the denominator (no signal).
- **Pure function:** same input → same output → same checksum.

## Build

```bash
cargo build --release
# binaries: target/release/pilaf-substrategy
#           target/release/pilaf-substrategy-tune
```

## Dev

```bash
cargo test
```

Tests cover the plan ranking, the feedback tuner (min-sample gate,
up-nudge, down-nudge, boundary clamping, ignored outcomes), and
checksum determinism.

## TS integration

`acp/src/lib/pilaf-substrategy-client.ts` calls this binary via
`child_process.spawn`. If the binary is missing or errors, the TS
client falls back to a pure-JS default heuristic so production stays
up. `acp/src/lib/pilaf-outcome-feedback.ts` mirrors the tuner
algorithm in TS for Vercel-runtime use. Deploy this crate separately;
acp never bundles it.
