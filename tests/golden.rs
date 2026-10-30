// R142 — cross-language golden vector for the weight tuner.
//
// The TS platform side mirrors `tune_weights` by hand
// (acp/src/lib/pilaf-outcome-feedback.ts). Nothing compared the two: the TS
// file claimed a CI cross-check that never existed, and its own checksum
// section admits the checksums are not comparable across languages (JSON key
// order). This test pins the Rust side to the same fixture the TS test uses.
//
// Deliberate non-goal: bit-identical output. Rust weights are f32, TS weights
// are f64, so 0.7f32 - 0.1f32 = 0.59999996 while 0.7 - 0.1 = 0.6 exactly.
// What must match is the DECISION: same weights moved, same direction, same
// sample sizes, same success rates, and weights equal within 1e-6.
//
// The fixture exists in both repositories (they are separate repos). Its
// SHA256 is pinned on both sides, so editing it on one side fails the other
// side's test immediately — a copy that silently diverges is worse than no
// fixture at all.

use pilaf_substrategy::{tune_weights, Feedback, ScoringWeights};
use serde::Deserialize;
use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect::<String>().to_uppercase()
}

/// Identity of tests/fixtures/pilaf-tune-v1.json. Update ONLY together with the
/// acp copy (acp/tests/golden/pilaf-tune-v1.json) — a one-sided edit is the
/// drift this file exists to prevent.
const FIXTURE_SHA256: &str = "AC24136F4A750984F2D7F1574A952D9042F7A70DC781225F447A7FDAAA9C8B98";

#[derive(Debug, Deserialize)]
struct Change {
    target: String,
    old: f32,
    new: f32,
    delta: f32,
    sample_size: usize,
    success_rate: f32,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct Expected {
    total_feedback_rows: usize,
    new_weights: ScoringWeights,
    changes: Vec<Change>,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    #[serde(rename = "fixture_version")]
    _fixture_version: u32,
    tolerance: f32,
    current: ScoringWeights,
    feedback: Vec<Feedback>,
    expected: Expected,
}

fn load() -> (Fixture, String) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pilaf-tune-v1.json");
    let raw = std::fs::read_to_string(path).expect("fixture missing");
    let fixture: Fixture = serde_json::from_str(&raw).expect("fixture is not valid JSON");
    (fixture, raw)
}

#[test]
fn fixture_identity_matches_the_ts_copy() {
    let (_fixture, raw) = load();
    // Normalise CRLF: the fixture is stored with autocrlf=true on Windows, so a
    // fresh checkout can hand us CRLF while CI sees LF. What is pinned is the
    // content, not the line endings.
    let normalised = raw.replace("\r\n", "\n");
    assert_eq!(
        sha256_hex(normalised.as_bytes()),
        FIXTURE_SHA256,
        "pilaf-tune-v1.json changed on one side only. Re-generate from the Rust \
         reference and copy the identical bytes into both repos."
    );
}

#[test]
fn tune_weights_reproduces_the_golden_vector() {
    let (fixture, _raw) = load();
    let result = tune_weights(&fixture.feedback, &fixture.current);
    let tol = fixture.tolerance;

    assert_eq!(result.total_feedback_rows, fixture.expected.total_feedback_rows);

    // Weights: tolerance, not equality (f32 arithmetic on both sides here, but a
    // future switch to f64 must not silently change the contract).
    let kinds: [(&str, f32, f32); 6] = [
        ("dispute_open", result.new_weights.kind.dispute_open, fixture.expected.new_weights.kind.dispute_open),
        ("anomaly_burst", result.new_weights.kind.anomaly_burst, fixture.expected.new_weights.kind.anomaly_burst),
        ("proposal_pending", result.new_weights.kind.proposal_pending, fixture.expected.new_weights.kind.proposal_pending),
        ("task_overdue_deadline", result.new_weights.kind.task_overdue_deadline, fixture.expected.new_weights.kind.task_overdue_deadline),
        ("module_draft_pending", result.new_weights.kind.module_draft_pending, fixture.expected.new_weights.kind.module_draft_pending),
        ("task_stale_claim", result.new_weights.kind.task_stale_claim, fixture.expected.new_weights.kind.task_stale_claim),
    ];
    for (name, actual, expected) in kinds {
        assert!(
            (actual - expected).abs() <= tol,
            "kind.{name}: rust={actual} fixture={expected} (|delta|>{tol})"
        );
    }
    assert!((result.new_weights.urgency.high - fixture.expected.new_weights.urgency.high).abs() <= tol);
    assert!((result.new_weights.urgency.medium - fixture.expected.new_weights.urgency.medium).abs() <= tol);
    assert!((result.new_weights.urgency.low - fixture.expected.new_weights.urgency.low).abs() <= tol);

    // Decisions: exact.
    assert_eq!(result.changes.len(), fixture.expected.changes.len(), "change count");
    for (i, expected) in fixture.expected.changes.iter().enumerate() {
        let actual = &result.changes[i];
        assert_eq!(actual.target, expected.target, "changes[{i}].target");
        assert_eq!(actual.reason, expected.reason, "changes[{i}].reason");
        assert_eq!(actual.sample_size, expected.sample_size, "changes[{i}].sample_size");
        assert!((actual.success_rate - expected.success_rate).abs() <= tol, "changes[{i}].success_rate");
        assert!((actual.delta - expected.delta).abs() <= tol, "changes[{i}].delta");
        assert!((actual.old - expected.old).abs() <= tol, "changes[{i}].old");
        assert!((actual.new - expected.new).abs() <= tol, "changes[{i}].new");
    }
}

#[test]
fn below_threshold_kind_is_left_alone() {
    let (fixture, _raw) = load();
    let count = fixture
        .feedback
        .iter()
        .filter(|f| format!("{:?}", f.kind) == "ModuleDraftPending")
        .count();
    assert_eq!(count, 2, "fixture should carry exactly 2 module_draft_pending rows");
    assert!(
        (fixture.expected.new_weights.kind.module_draft_pending
            - fixture.current.kind.module_draft_pending)
            .abs()
            < f32::EPSILON
    );
}
