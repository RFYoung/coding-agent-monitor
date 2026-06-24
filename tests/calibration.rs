use coding_agent_monitor::{
    ActionOutcome, CalibrationQuery, ControlAction, ControlActionKind, ControlPacket,
    DashboardSnapshot, DispatchResult, DispatchStatus, EntropyDelta, EntropyDeltaEvidence,
    EntropyKind, OutcomeStatus, PacketPreconditions, PacketUrgency, ProjectStore,
    ValidationOutcome, VerificationSuite, build_control_case_file, load_calibration_report,
};
use std::path::Path;

fn append_advice(
    store: &mut ProjectStore,
    workspace: &Path,
    action: ControlAction,
    advice_id: &str,
    expected_entropy_delta: Vec<EntropyDelta>,
) {
    append_advice_for_target(
        store,
        workspace,
        action,
        advice_id,
        "codex",
        expected_entropy_delta,
    );
}

fn append_advice_for_target(
    store: &mut ProjectStore,
    workspace: &Path,
    action: ControlAction,
    advice_id: &str,
    target_agent: &str,
    expected_entropy_delta: Vec<EntropyDelta>,
) {
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(workspace, &snapshot);
    store.append_case_file(&case_file).expect("case file");
    let action_kind = action.kind();
    let packet = ControlPacket {
        packet_id: format!("packet-{advice_id}"),
        target_agent: target_agent.into(),
        urgency: PacketUrgency::FollowUp,
        title: "Calibration fixture".into(),
        summary: "Calibration fixture packet.".into(),
        instructions: Vec::new(),
        evidence_refs: Vec::new(),
        forbidden: Vec::new(),
        success_criteria: Vec::new(),
        preconditions: PacketPreconditions::default(),
    };
    let dispatch = DispatchResult {
        dispatch_id: format!("dispatch-{advice_id}"),
        packet_id: packet.packet_id.clone(),
        target_agent: packet.target_agent.clone(),
        status: DispatchStatus::OutboxWritten,
        path: Some(format!("outbox/{advice_id}.md")),
        reason: None,
    };
    store
        .append_advice(&coding_agent_monitor::AdviceRun {
            advice_id: advice_id.into(),
            case_file_id: case_file.case_file_id.clone(),
            advisor_used: false,
            advisor_error: None,
            advisor_decision: None,
            validation_outcome: ValidationOutcome::Approved(action.clone()),
            final_action: action,
            control_rationale: coding_agent_monitor::ControlRationale {
                selected_action: action_kind,
                dominant_entropy: None,
                reason: "calibration fixture".into(),
                expected_entropy_delta,
                evidence_ids: Vec::new(),
                requirement_ids: Vec::new(),
            },
            packet,
            dispatch_result: dispatch,
            packet_path: Some(format!("outbox/{advice_id}.md")),
        })
        .expect("advice");
}

#[test]
fn calibration_report_aggregates_outcomes_and_unresolved_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_advice(
        &mut store,
        temp.path(),
        ControlAction::ForceVerification {
            suite: VerificationSuite::Targeted,
            blocking: true,
        },
        "advice-force-success",
        vec![EntropyDelta {
            kind: EntropyKind::Verification,
            delta: -55,
        }],
    );
    append_advice(
        &mut store,
        temp.path(),
        ControlAction::ForceVerification {
            suite: VerificationSuite::Targeted,
            blocking: true,
        },
        "advice-force-unresolved",
        vec![EntropyDelta {
            kind: EntropyKind::Verification,
            delta: -55,
        }],
    );
    store
        .append_action_outcome(&ActionOutcome {
            outcome_id: "outcome-force-success".into(),
            advice_id: "advice-force-success".into(),
            action: ControlActionKind::ForceVerification,
            status: OutcomeStatus::Succeeded,
            expected_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -55,
            }],
            observed_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -40,
            }],
            observed_entropy_delta_evidence: vec![EntropyDeltaEvidence {
                kind: EntropyKind::Verification,
                evidence_ids: vec!["evt-stale-verifier".into(), "verifier-run-1".into()],
                cause_evidence_ids: vec!["evt-stale-verifier".into()],
                result_evidence_ids: vec!["verifier-run-1".into()],
            }],
            evidence_ids: vec!["verifier-run-1".into()],
            requirement_ids: Vec::new(),
            note: Some("Verifier passed with partial confidence recovery.".into()),
        })
        .expect("outcome");

    let report = load_calibration_report(
        temp.path(),
        CalibrationQuery {
            limit: 5,
            action: None,
        },
    )
    .expect("calibration report");

    assert_eq!(report.advice_count, 2);
    assert_eq!(report.outcome_count, 1);
    assert_eq!(report.unresolved_advice_count, 1);
    assert_eq!(report.recent_outcomes.len(), 1);
    assert_eq!(report.recent_outcomes[0].absolute_error, 15);
    assert_eq!(
        report.recent_outcomes[0].observed_entropy_delta_evidence,
        vec![EntropyDeltaEvidence {
            kind: EntropyKind::Verification,
            evidence_ids: vec!["evt-stale-verifier".into(), "verifier-run-1".into()],
            cause_evidence_ids: vec!["evt-stale-verifier".into()],
            result_evidence_ids: vec!["verifier-run-1".into()],
        }]
    );
    assert_eq!(
        report.recent_outcomes[0].necessary_evidence_ids,
        vec!["verifier-run-1".to_string()]
    );
    assert_eq!(
        report.recent_outcomes[0].correlated_evidence_ids,
        vec!["evt-stale-verifier".to_string()]
    );

    let force = report
        .actions
        .iter()
        .find(|action| action.action == ControlActionKind::ForceVerification)
        .expect("force verification action summary");
    assert_eq!(force.advice_count, 2);
    assert_eq!(force.outcome_count, 1);
    assert_eq!(force.succeeded, 1);
    assert_eq!(force.unresolved_advice_count, 1);
    assert_eq!(
        force.expected_entropy_delta,
        vec![EntropyDelta {
            kind: EntropyKind::Verification,
            delta: -55,
        }]
    );
    assert_eq!(
        force.observed_entropy_delta,
        vec![EntropyDelta {
            kind: EntropyKind::Verification,
            delta: -40,
        }]
    );
    assert_eq!(force.absolute_error, 15);
}

#[test]
fn calibration_report_aggregates_action_outcomes_by_target_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_advice_for_target(
        &mut store,
        temp.path(),
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        },
        "advice-switch-claude",
        "claude-code",
        vec![EntropyDelta {
            kind: EntropyKind::AgentHealth,
            delta: -45,
        }],
    );
    append_advice_for_target(
        &mut store,
        temp.path(),
        ControlAction::SwitchAgent {
            target_agent: "opencode".into(),
        },
        "advice-switch-opencode",
        "opencode",
        vec![EntropyDelta {
            kind: EntropyKind::AgentHealth,
            delta: -45,
        }],
    );
    store
        .append_action_outcome(&ActionOutcome {
            outcome_id: "outcome-switch-claude".into(),
            advice_id: "advice-switch-claude".into(),
            action: ControlActionKind::SwitchAgent,
            status: OutcomeStatus::Failed,
            expected_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::AgentHealth,
                delta: -45,
            }],
            observed_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::AgentHealth,
                delta: 0,
            }],
            observed_entropy_delta_evidence: Vec::new(),
            evidence_ids: vec!["evt-claude-failed".into()],
            requirement_ids: Vec::new(),
            note: None,
        })
        .expect("failed outcome");
    store
        .append_action_outcome(&ActionOutcome {
            outcome_id: "outcome-switch-opencode".into(),
            advice_id: "advice-switch-opencode".into(),
            action: ControlActionKind::SwitchAgent,
            status: OutcomeStatus::Succeeded,
            expected_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::AgentHealth,
                delta: -45,
            }],
            observed_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::AgentHealth,
                delta: -40,
            }],
            observed_entropy_delta_evidence: Vec::new(),
            evidence_ids: vec!["evt-opencode-success".into()],
            requirement_ids: Vec::new(),
            note: None,
        })
        .expect("successful outcome");

    let report = load_calibration_report(
        temp.path(),
        CalibrationQuery {
            limit: 5,
            action: Some(ControlActionKind::SwitchAgent),
        },
    )
    .expect("calibration report");

    assert_eq!(report.targets.len(), 2);
    let claude = report
        .targets
        .iter()
        .find(|target| target.target_agent == "claude-code")
        .expect("claude target");
    assert_eq!(claude.action, ControlActionKind::SwitchAgent);
    assert_eq!(claude.advice_count, 1);
    assert_eq!(claude.outcome_count, 1);
    assert_eq!(claude.failed, 1);
    assert_eq!(claude.absolute_error, 45);

    let opencode = report
        .targets
        .iter()
        .find(|target| target.target_agent == "opencode")
        .expect("opencode target");
    assert_eq!(opencode.succeeded, 1);
    assert_eq!(opencode.absolute_error, 5);
}
