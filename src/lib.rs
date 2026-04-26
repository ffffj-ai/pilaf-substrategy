//! R51 — Pilaf layer-3 sub-strategy (2026-04-24, feedback-tuning pass 2026-04-25)
//!
//! Per R46.0 D9 lock, Pilaf's three-layer architecture is:
//!   * Layer 1 (Core Identity)  — immutable (acp/src/lib/system-agents.ts)
//!   * Layer 2 (Strategy)       — `pilaf-brain.ts`: signal detection
//!   * Layer 3 (Sub-strategy)   — THIS crate: evolving heuristics
//!
//! R46.0 D7 locks the personality (layer 1) as immutable. R46.0 D9
//! allows layer 3 to evolve FREELY — including rewriting this file —
//! as long as the IO contract stays:
//!
//!   stdin:  AttentionQueue JSON (shape matches acp/src/lib/pilaf-brain.ts)
//!   stdout: SubstrategyDecision JSON (shape defined in this crate)
//!
//! The TS side (`acp/src/lib/pilaf-substrategy-client.ts`) calls this
//! binary via child_process.spawn. If the binary is missing or errors,
//! TS falls back to a pure-JS default heuristic so prod stays up.
//!
//! 2026-04-25 extension: CI-gated self-modification loop.
//! The crate now exposes:
//!   * `ScoringWeights` — the tunable kind/urgency multipliers.
//!   * `plan_with_weights(queue, weights)` — parameterised plan; the
//!     original `plan(queue)` delegates to it with `ScoringWeights::default()`.
//!   * `tune_weights(history, current)` — pure-function tuner that
//!     proposes weight adjustments from recent decision outcomes
//!     (`Feedback[]`). Output includes a deterministic FNV-1a checksum
//!     over the serialized new weights so CI can verify the diff is
//!     reproducible.
//!
//! The CLI still ships with built-in default weights. Operators pass
//! `PILAF_SUBSTRATEGY_WEIGHTS_PATH=/abs/path/to/weights.json` to make
//! the CLI load a tuned weights file. The path is read on startup only
//! — stdin/stdout contract is untouched.
//!
//! Self-modification gate: the tuner is a separate binary
//! (`pilaf-substrategy-tune`). It reads feedback history + current
//! weights and emits a proposed new weights JSON. A human (or CI job)
//! reviews the diff, checks the checksum matches what `tune_weights`
//! produces from the same input, and decides whether to merge. The
//! main CLI never mutates weights on its own — it only reads them.

use serde::{Deserialize, Serialize};

// ----- Input (mirrors acp/src/lib/pilaf-brain.ts AttentionQueue) ------

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    Medium,
    High,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    DisputeOpen,
    ModuleDraftPending,
    TaskStaleClaim,
    TaskOverdueDeadline,
    AnomalyBurst,
    ProposalPending,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AttentionItem {
    pub kind: Kind,
    pub urgency: Urgency,
    pub subject_id: String,
    pub title: String,
    #[serde(default)]
    pub detail: serde_json::Value,
    pub observed_at: String,
}

#[derive(Debug, Deserialize)]
pub struct AttentionQueue {
    pub generated_at: String,
    pub item_count: u32,
    pub items: Vec<AttentionItem>,
}

// ----- Output (the contract this crate owns) -------------------------

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SuggestedAction {
    /// Arbitrate the dispute (Pilaf's binding verdict)
    Arbitrate,
    /// Review + tier-promote / tier-takedown the module
    ReviewModule,
    /// Nudge the claimer via message (delegates to poster, Pilaf sends)
    NudgeTaskClaimer,
    /// Alert admin — this is outside Pilaf's direct authority
    EscalateToAdmin,
    /// Broadcast a short platform-wide message
    Broadcast,
    /// Ignore for now — queue will be re-scanned on next cron
    DeferAndMonitor,
}

#[derive(Debug, Serialize)]
pub struct Decision {
    pub subject_id: String,
    pub kind: String,
    pub urgency: String,
    /// Ranked [0,1]; 1 = highest priority. Drives the order Pilaf's
    /// session works through the plan.
    pub score: f32,
    pub suggested_action: SuggestedAction,
    pub rationale: String,
}

#[derive(Debug, Serialize)]
pub struct SubstrategyDecision {
    pub version: String,
    pub generated_for: String,
    pub weights_source: &'static str,
    pub weights_checksum: String,
    pub decisions: Vec<Decision>,
}

// ----- Tunable weights -----------------------------------------------
//
// ScoringWeights is the SOLE surface the tuner may mutate. Everything
// else (plan algorithm, JSON contract, action mapping) is frozen.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UrgencyWeights {
    pub high: f32,
    pub medium: f32,
    pub low: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KindWeights {
    pub dispute_open: f32,
    pub anomaly_burst: f32,
    pub proposal_pending: f32,
    pub task_overdue_deadline: f32,
    pub module_draft_pending: f32,
    pub task_stale_claim: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoringWeights {
    /// Schema version for the weights file. Bump if the tuner changes
    /// the shape (adds a new kind, etc.). CI should reject mismatched
    /// schema versions.
    pub schema_version: u32,
    pub urgency: UrgencyWeights,
    pub kind: KindWeights,
}

impl Default for UrgencyWeights {
    fn default() -> Self {
        UrgencyWeights {
            high: 1.0,
            medium: 0.6,
            low: 0.3,
        }
    }
}

impl Default for KindWeights {
    fn default() -> Self {
        KindWeights {
            dispute_open: 1.0,
            anomaly_burst: 0.9,
            proposal_pending: 0.7,
            task_overdue_deadline: 0.6,
            module_draft_pending: 0.5,
            task_stale_claim: 0.4,
        }
    }
}

impl Default for ScoringWeights {
    fn default() -> Self {
        ScoringWeights {
            schema_version: 1,
            urgency: UrgencyWeights::default(),
            kind: KindWeights::default(),
        }
    }
}

impl ScoringWeights {
    fn urgency_for(&self, u: &Urgency) -> f32 {
        match u {
            Urgency::High => self.urgency.high,
            Urgency::Medium => self.urgency.medium,
            Urgency::Low => self.urgency.low,
        }
    }

    fn kind_for(&self, k: &Kind) -> f32 {
        match k {
            Kind::DisputeOpen => self.kind.dispute_open,
            Kind::AnomalyBurst => self.kind.anomaly_burst,
            Kind::ProposalPending => self.kind.proposal_pending,
            Kind::TaskOverdueDeadline => self.kind.task_overdue_deadline,
            Kind::ModuleDraftPending => self.kind.module_draft_pending,
            Kind::TaskStaleClaim => self.kind.task_stale_claim,
        }
    }

    /// Clamp every weight into the safe operating band.
    pub fn clamp(&mut self) {
        fn c(v: f32) -> f32 {
            if v.is_nan() {
                0.1
            } else {
                v.max(WEIGHT_MIN).min(WEIGHT_MAX)
            }
        }
        self.urgency.high = c(self.urgency.high);
        self.urgency.medium = c(self.urgency.medium);
        self.urgency.low = c(self.urgency.low);
        self.kind.dispute_open = c(self.kind.dispute_open);
        self.kind.anomaly_burst = c(self.kind.anomaly_burst);
        self.kind.proposal_pending = c(self.kind.proposal_pending);
        self.kind.task_overdue_deadline = c(self.kind.task_overdue_deadline);
        self.kind.module_draft_pending = c(self.kind.module_draft_pending);
        self.kind.task_stale_claim = c(self.kind.task_stale_claim);
    }
}

/// Weights are clamped to this band. A value of 0.0 means "never
/// prioritise" and is dangerous (could starve a kind forever). A
/// value >2.0 lets a single kind dominate regardless of urgency.
pub const WEIGHT_MIN: f32 = 0.1;
pub const WEIGHT_MAX: f32 = 2.0;

// ----- Scoring heuristic (evolve freely) -----------------------------

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

fn suggested_for(k: &Kind) -> SuggestedAction {
    match k {
        Kind::DisputeOpen => SuggestedAction::Arbitrate,
        Kind::AnomalyBurst => SuggestedAction::EscalateToAdmin,
        Kind::ProposalPending => SuggestedAction::EscalateToAdmin,
        Kind::TaskOverdueDeadline => SuggestedAction::NudgeTaskClaimer,
        Kind::ModuleDraftPending => SuggestedAction::ReviewModule,
        Kind::TaskStaleClaim => SuggestedAction::NudgeTaskClaimer,
    }
}

fn kind_label(k: &Kind) -> &'static str {
    match k {
        Kind::DisputeOpen => "dispute_open",
        Kind::ModuleDraftPending => "module_draft_pending",
        Kind::TaskStaleClaim => "task_stale_claim",
        Kind::TaskOverdueDeadline => "task_overdue_deadline",
        Kind::AnomalyBurst => "anomaly_burst",
        Kind::ProposalPending => "proposal_pending",
    }
}

fn urgency_label(u: &Urgency) -> &'static str {
    match u {
        Urgency::High => "high",
        Urgency::Medium => "medium",
        Urgency::Low => "low",
    }
}

/// Score + rank an attention queue into a decision plan using the
/// built-in default weights. Backwards-compatible entry point.
pub fn plan(queue: &AttentionQueue) -> SubstrategyDecision {
    plan_with_weights(queue, &ScoringWeights::default(), "default")
}

/// Score + rank an attention queue with an explicit weights table.
/// `weights_source` is a free-form tag written into the output so
/// downstream observers can tell whether the CLI used built-in defaults
/// or loaded a tuned file.
pub fn plan_with_weights(
    queue: &AttentionQueue,
    weights: &ScoringWeights,
    weights_source: &'static str,
) -> SubstrategyDecision {
    let mut decisions: Vec<Decision> = queue
        .items
        .iter()
        .map(|item| {
            let score = weights.urgency_for(&item.urgency) * weights.kind_for(&item.kind);
            let suggested_action = suggested_for(&item.kind);
            let rationale = format!(
                "{}/{} → {:.2} (urgency*kind; weights={}).",
                kind_label(&item.kind),
                urgency_label(&item.urgency),
                score,
                weights_source
            );
            Decision {
                subject_id: item.subject_id.clone(),
                kind: kind_label(&item.kind).to_string(),
                urgency: urgency_label(&item.urgency).to_string(),
                score,
                suggested_action,
                rationale,
            }
        })
        .collect();

    // Sort descending by score. Stable sort preserves queue order on ties.
    decisions.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    SubstrategyDecision {
        version: CRATE_VERSION.to_string(),
        generated_for: queue.generated_at.clone(),
        weights_source,
        weights_checksum: checksum(weights),
        decisions,
    }
}

// ----- Feedback + tuner ---------------------------------------------

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackOutcome {
    /// The suggested action was executed and judged successful
    /// (e.g. arbitration held, nudge produced a claimer reply).
    Ok,
    /// The suggested action was executed but judged unsuccessful
    /// (e.g. arbitrate returned a platform error, nudge was ignored).
    Failed,
    /// Pilaf chose `defer_and_monitor` or the subject disappeared
    /// (auto-resolved) — not a pure signal either way.
    Ignored,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Feedback {
    pub kind: Kind,
    pub urgency: Urgency,
    pub outcome: FeedbackOutcome,
    /// Free-form tag (subject_id or similar) — not used by tuner
    /// arithmetic, just echoed in explanations.
    #[serde(default)]
    pub subject_id: Option<String>,
    #[serde(default)]
    pub observed_at: Option<String>,
}

/// Minimum number of samples for a single kind before the tuner will
/// touch its weight. Prevents tuning on 1-2 data points.
pub const TUNER_MIN_SAMPLES_PER_KIND: usize = 5;

/// Maximum delta the tuner may apply to any single weight per run.
/// Caps runaway self-modification — CI can still refuse.
pub const TUNER_MAX_DELTA: f32 = 0.1;

/// Success-rate thresholds that trigger a weight nudge.
pub const TUNER_NUDGE_UP_SR: f32 = 0.8; // ≥80% success → bump weight up
pub const TUNER_NUDGE_DOWN_SR: f32 = 0.4; // <40% success → bump weight down

#[derive(Debug, Serialize, PartialEq)]
pub struct WeightChange {
    pub target: String,
    pub old: f32,
    pub new: f32,
    pub delta: f32,
    pub sample_size: usize,
    pub success_rate: f32,
    pub reason: &'static str,
}

#[derive(Debug, Serialize)]
pub struct TuneResult {
    pub schema_version: u32,
    pub total_feedback_rows: usize,
    pub old_checksum: String,
    pub new_checksum: String,
    pub new_weights: ScoringWeights,
    pub changes: Vec<WeightChange>,
}

/// Propose a new set of weights given recent decision outcomes.
///
/// Algorithm (deliberately conservative):
///   1. Group feedback by kind.
///   2. For each kind with ≥TUNER_MIN_SAMPLES_PER_KIND decisions,
///      compute the success rate over Ok/Failed (Ignored is dropped
///      from the denominator — no signal).
///   3. Success rate ≥ TUNER_NUDGE_UP_SR → weight += TUNER_MAX_DELTA.
///      Success rate < TUNER_NUDGE_DOWN_SR → weight -= TUNER_MAX_DELTA.
///      Otherwise no change.
///   4. Clamp every weight into [WEIGHT_MIN, WEIGHT_MAX].
///
/// The tuner intentionally does NOT touch urgency weights (those are
/// already heavily constrained by the signal-detection layer), nor
/// action mappings. A future version may revisit that.
///
/// Pure function — no IO, deterministic, safe to run in CI.
pub fn tune_weights(history: &[Feedback], current: &ScoringWeights) -> TuneResult {
    let old_checksum = checksum(current);
    let mut new_weights = current.clone();
    let mut changes: Vec<WeightChange> = Vec::new();

    // Group feedback by Kind.
    let kinds = [
        Kind::DisputeOpen,
        Kind::AnomalyBurst,
        Kind::ProposalPending,
        Kind::TaskOverdueDeadline,
        Kind::ModuleDraftPending,
        Kind::TaskStaleClaim,
    ];

    for k in kinds.iter() {
        let (ok, failed) = history.iter().fold((0usize, 0usize), |(o, f), row| {
            if row.kind != *k {
                return (o, f);
            }
            match row.outcome {
                FeedbackOutcome::Ok => (o + 1, f),
                FeedbackOutcome::Failed => (o, f + 1),
                FeedbackOutcome::Ignored => (o, f),
            }
        });
        let sample_size = ok + failed;
        if sample_size < TUNER_MIN_SAMPLES_PER_KIND {
            continue;
        }
        let success_rate = ok as f32 / sample_size as f32;
        let (delta, reason): (f32, &'static str) = if success_rate >= TUNER_NUDGE_UP_SR {
            (TUNER_MAX_DELTA, "success_rate_high")
        } else if success_rate < TUNER_NUDGE_DOWN_SR {
            (-TUNER_MAX_DELTA, "success_rate_low")
        } else {
            continue;
        };
        let old = current.kind_for(k);
        let new_val = (old + delta).max(WEIGHT_MIN).min(WEIGHT_MAX);
        if (new_val - old).abs() < 1e-6 {
            // Already at the boundary — skip silently.
            continue;
        }
        // Apply to new_weights.
        match k {
            Kind::DisputeOpen => new_weights.kind.dispute_open = new_val,
            Kind::AnomalyBurst => new_weights.kind.anomaly_burst = new_val,
            Kind::ProposalPending => new_weights.kind.proposal_pending = new_val,
            Kind::TaskOverdueDeadline => new_weights.kind.task_overdue_deadline = new_val,
            Kind::ModuleDraftPending => new_weights.kind.module_draft_pending = new_val,
            Kind::TaskStaleClaim => new_weights.kind.task_stale_claim = new_val,
        }
        changes.push(WeightChange {
            target: format!("kind.{}", kind_label(k)),
            old,
            new: new_val,
            delta: new_val - old,
            sample_size,
            success_rate,
            reason,
        });
    }

    new_weights.clamp();

    TuneResult {
        schema_version: new_weights.schema_version,
        total_feedback_rows: history.len(),
        old_checksum,
        new_checksum: checksum(&new_weights),
        new_weights,
        changes,
    }
}

// ----- Checksum ------------------------------------------------------
//
// Deterministic FNV-1a over the JSON-serialized weights. Pure stdlib,
// no crypto dep. The goal isn't tamper-resistance — it's detecting
// accidental drift in CI (same input → same checksum).

pub fn checksum(weights: &ScoringWeights) -> String {
    let json = serde_json::to_string(weights).unwrap_or_default();
    fnv1a_hex(json.as_bytes())
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a-{:016x}", h)
}

// ----- Tests ---------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_queue() -> AttentionQueue {
        AttentionQueue {
            generated_at: "2026-04-24T00:00:00Z".to_string(),
            item_count: 3,
            items: vec![
                AttentionItem {
                    kind: Kind::DisputeOpen,
                    urgency: Urgency::High,
                    subject_id: "d1".to_string(),
                    title: "open dispute".to_string(),
                    detail: serde_json::json!({}),
                    observed_at: "2026-04-24T00:00:00Z".to_string(),
                },
                AttentionItem {
                    kind: Kind::TaskStaleClaim,
                    urgency: Urgency::Low,
                    subject_id: "t1".to_string(),
                    title: "stale task".to_string(),
                    detail: serde_json::json!({}),
                    observed_at: "2026-04-24T00:00:00Z".to_string(),
                },
                AttentionItem {
                    kind: Kind::AnomalyBurst,
                    urgency: Urgency::Medium,
                    subject_id: "a1".to_string(),
                    title: "anomaly".to_string(),
                    detail: serde_json::json!({}),
                    observed_at: "2026-04-24T00:00:00Z".to_string(),
                },
            ],
        }
    }

    #[test]
    fn dispute_high_beats_others() {
        let q = sample_queue();
        let plan = plan(&q);
        assert_eq!(plan.decisions.len(), 3);
        assert_eq!(plan.decisions[0].kind, "dispute_open");
        assert_eq!(plan.decisions[0].urgency, "high");
    }

    #[test]
    fn empty_queue_yields_empty_plan() {
        let q = AttentionQueue {
            generated_at: "2026-04-24T00:00:00Z".to_string(),
            item_count: 0,
            items: vec![],
        };
        let plan = plan(&q);
        assert!(plan.decisions.is_empty());
    }

    #[test]
    fn default_weights_checksum_is_stable() {
        let w = ScoringWeights::default();
        let c1 = checksum(&w);
        let c2 = checksum(&w);
        assert_eq!(c1, c2);
        assert!(c1.starts_with("fnv1a-"));
    }

    #[test]
    fn tuner_ignores_small_samples() {
        let history = vec![
            Feedback {
                kind: Kind::DisputeOpen,
                urgency: Urgency::High,
                outcome: FeedbackOutcome::Ok,
                subject_id: None,
                observed_at: None,
            },
            Feedback {
                kind: Kind::DisputeOpen,
                urgency: Urgency::High,
                outcome: FeedbackOutcome::Failed,
                subject_id: None,
                observed_at: None,
            },
        ];
        let current = ScoringWeights::default();
        let result = tune_weights(&history, &current);
        assert!(result.changes.is_empty());
        assert_eq!(result.new_weights, current);
    }

    #[test]
    fn tuner_bumps_down_on_low_success() {
        // 6 stale-claim nudges, 5 failed, 1 ok → 16.7% → bump down
        let mut history: Vec<Feedback> = (0..5)
            .map(|_| Feedback {
                kind: Kind::TaskStaleClaim,
                urgency: Urgency::Low,
                outcome: FeedbackOutcome::Failed,
                subject_id: None,
                observed_at: None,
            })
            .collect();
        history.push(Feedback {
            kind: Kind::TaskStaleClaim,
            urgency: Urgency::Low,
            outcome: FeedbackOutcome::Ok,
            subject_id: None,
            observed_at: None,
        });
        let current = ScoringWeights::default();
        let result = tune_weights(&history, &current);
        assert_eq!(result.changes.len(), 1);
        let change = &result.changes[0];
        assert_eq!(change.target, "kind.task_stale_claim");
        assert!(change.delta < 0.0);
        assert_eq!(change.reason, "success_rate_low");
        assert!(result.new_weights.kind.task_stale_claim < current.kind.task_stale_claim);
    }

    #[test]
    fn tuner_bumps_up_on_high_success() {
        // 5 disputes, 5 ok → 100% → bump up
        let history: Vec<Feedback> = (0..5)
            .map(|_| Feedback {
                kind: Kind::DisputeOpen,
                urgency: Urgency::High,
                outcome: FeedbackOutcome::Ok,
                subject_id: None,
                observed_at: None,
            })
            .collect();
        let current = ScoringWeights::default();
        let result = tune_weights(&history, &current);
        assert_eq!(result.changes.len(), 1);
        let change = &result.changes[0];
        assert_eq!(change.target, "kind.dispute_open");
        assert!(change.delta > 0.0);
        assert_eq!(change.reason, "success_rate_high");
        // dispute_open starts at 1.0, bumps to 1.1 (still under WEIGHT_MAX=2.0)
        assert!((result.new_weights.kind.dispute_open - 1.1).abs() < 1e-5);
    }

    #[test]
    fn tuner_clamps_at_boundary() {
        // Push dispute to 1.95, then bump up 0.1 → clamped to 2.0 with delta 0.05
        let mut current = ScoringWeights::default();
        current.kind.dispute_open = 1.95;
        let history: Vec<Feedback> = (0..5)
            .map(|_| Feedback {
                kind: Kind::DisputeOpen,
                urgency: Urgency::High,
                outcome: FeedbackOutcome::Ok,
                subject_id: None,
                observed_at: None,
            })
            .collect();
        let result = tune_weights(&history, &current);
        assert_eq!(result.changes.len(), 1);
        assert!((result.new_weights.kind.dispute_open - 2.0).abs() < 1e-5);
        assert!((result.changes[0].delta - 0.05).abs() < 1e-5);
    }

    #[test]
    fn tuner_ignores_ignored_outcomes() {
        // 4 ignored + 1 ok → effective sample_size 1 → below min
        let mut history: Vec<Feedback> = (0..4)
            .map(|_| Feedback {
                kind: Kind::DisputeOpen,
                urgency: Urgency::High,
                outcome: FeedbackOutcome::Ignored,
                subject_id: None,
                observed_at: None,
            })
            .collect();
        history.push(Feedback {
            kind: Kind::DisputeOpen,
            urgency: Urgency::High,
            outcome: FeedbackOutcome::Ok,
            subject_id: None,
            observed_at: None,
        });
        let current = ScoringWeights::default();
        let result = tune_weights(&history, &current);
        assert!(result.changes.is_empty());
    }

    #[test]
    fn plan_with_weights_uses_custom_weights() {
        let mut weights = ScoringWeights::default();
        // Invert priority: make stale_claim the highest.
        weights.kind.task_stale_claim = 2.0;
        weights.kind.dispute_open = 0.5;
        let q = sample_queue();
        let plan = plan_with_weights(&q, &weights, "test");
        assert_eq!(plan.decisions[0].kind, "task_stale_claim");
        assert!(plan.weights_source == "test");
    }
}
