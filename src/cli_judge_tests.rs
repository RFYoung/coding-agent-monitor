//! Tests for the external judge command: review-to-intervention mapping
//! and the bounded, evidence-first judge prompt.
//!
//! Included into the module via `#[path]` so they can reach its private
//! helpers as well as the binary crate root.

use super::*;

#[test]
fn judge_review_intervention_preserves_spawn_judge_action() {
    let report = AgentReviewReport {
        workspace: "E:/demo".into(),
        status: coding_agent_monitor::AgentReviewStatus::Intervene,
        findings: vec![coding_agent_monitor::AgentReviewFinding {
            severity: coding_agent_monitor::DashboardSeverity::Critical,
            category: "suspicious_untraced_change".into(),
            agent: Some("codex".into()),
            evidence: "src/lib.rs has dirty git hunks without trace evidence".into(),
            recommended_action: AgentReviewAction::SpawnJudgeAgent,
        }],
    };

    let interventions = interventions_from_review(&report);

    assert_eq!(interventions.len(), 1);
    assert_eq!(
        interventions[0].kind,
        coding_agent_monitor::InterventionKind::SuspiciousChange
    );
    assert_eq!(
        interventions[0].action,
        coding_agent_monitor::Action::SpawnJudgeAgent
    );
}

#[test]
fn external_judge_prompt_is_bounded_and_evidence_first() {
    assert!(EXTERNAL_JUDGE_PROMPT.contains("Output exactly one line"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("continue | force_verification | handoff | restart"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("evidence=<ids/files/tests>"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("Judge the control loop"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("unverified completion"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("stale verification"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("intended-environment validation"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("mobile/native/system/ML"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("Prefer force_verification over handoff"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("Do not propose broad refactors"));
}
