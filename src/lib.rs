//! R51 — Pilaf layer-3 sub-strategy (2026-04-24)
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
//! What layer 3 decides: for each queue item, WHICH action Pilaf should
//! consider taking. Layer 3 does NOT execute the action — that stays in
//! TS/REST where audit + rate limits + signing live. Layer 3 just
//! produces a ranked plan; Pilaf's LLM session uses it as guidance.
//!
//! This V1 scaffold uses a trivial rule-based scorer. Future iterations
//! may swap in an RL policy, a frozen-embedding model, or yoyo-evolve
//! (R46.0 D10) borrow boundaries that take the TS side's anomaly
//! history as additional input.

use serde::{Deserialize, Serialize};

// ----- Input (mirrors acp/src/lib/pilaf-brain.ts AttentionQueue) ------

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    Medium,
    High,
}

#[derive(Debug, Deserialize, Clone)]
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

#[derive(Debug, Serialize, Clone)]
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
    pub decisions: Vec<Decision>,
}

// ----- Scoring heuristic (evolve freely) -----------------------------

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

fn urgency_weight(u: &Urgency) -> f32 {
    match u {
        Urgency::High => 1.0,
        Urgency::Medium => 0.6,
        Urgency::Low => 0.3,
    }
}

fn kind_weight(k: &Kind) -> f32 {
    match k {
        // Disputes are binding + blocking → top priority
        Kind::DisputeOpen => 1.0,
        // Anomaly bursts tell Pilaf about a concurrent platform problem
        Kind::AnomalyBurst => 0.9,
        // Proposals gate governance; admin-facing
        Kind::ProposalPending => 0.7,
        // Overdue deadlines surface reputation risk for both parties
        Kind::TaskOverdueDeadline => 0.6,
        // Draft modules are slow-burn backlog
        Kind::ModuleDraftPending => 0.5,
        // Stale claims are slow-burn too; usually resolved by poster nudge
        Kind::TaskStaleClaim => 0.4,
    }
}

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

/// Score + rank an attention queue into a decision plan.
pub fn plan(queue: &AttentionQueue) -> SubstrategyDecision {
    let mut decisions: Vec<Decision> = queue
        .items
        .iter()
        .map(|item| {
            let score = urgency_weight(&item.urgency) * kind_weight(&item.kind);
            let suggested_action = suggested_for(&item.kind);
            let rationale = format!(
                "{}/{} → {:.2} (urgency*kind).",
                kind_label(&item.kind),
                urgency_label(&item.urgency),
                score
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
        decisions,
    }
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
}
