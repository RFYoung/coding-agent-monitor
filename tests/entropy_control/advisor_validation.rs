use super::*;

#[test]
fn advisor_validation_rejects_evidence_ids_outside_case_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-missing-evidence".into()),
        dominant_entropy: EntropyKind::Verification,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Verification,
            AdvisorEntropyEstimate {
                score: 80,
                confidence: 85,
            },
        )]),
        top_evidence: vec![],
        cited_evidence_ids: vec!["evt-missing".into()],
        missing_evidence: vec!["passing verification result".into()],
        proposed_action: ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Verification,
            delta: -55,
        }],
        packet_intent: Some("require verification".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::Urgent,
            summary: "Verify before continuing.".into(),
            instructions: vec!["Run cargo test.".into()],
            evidence_refs: vec![],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("unknown evidence should be rejected");

    assert!(err.to_string().contains("evt-missing"));
}

#[test]
fn advisor_validation_rejects_unknown_top_evidence_and_packet_refs() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-unknown-ref".into()),
        dominant_entropy: EntropyKind::Plan,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Plan,
            AdvisorEntropyEstimate {
                score: 65,
                confidence: 80,
            },
        )]),
        top_evidence: vec![AdvisorEvidenceRef {
            event_id: "evt-missing-top".into(),
            why_it_matters: "The advisor must cite only case-file evidence.".into(),
        }],
        cited_evidence_ids: vec!["evt-known".into()],
        missing_evidence: vec!["next concrete step".into()],
        proposed_action: ControlAction::SendFollowUp { target_agent: None },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Plan,
            delta: -25,
        }],
        packet_intent: Some("require a bounded next step".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::FollowUp,
            summary: "Continue with one bounded next step.".into(),
            instructions: vec!["Take one concrete implementation step.".into()],
            evidence_refs: vec!["evt-missing-packet".into()],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("unknown advisor evidence refs should be rejected");

    assert!(
        err.to_string().contains("evt-missing-top")
            || err.to_string().contains("evt-missing-packet")
    );
}

#[test]
fn advisor_validation_rejects_out_of_range_entropy_estimates() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-bad-score".into()),
        dominant_entropy: EntropyKind::Plan,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Plan,
            AdvisorEntropyEstimate {
                score: 101,
                confidence: 80,
            },
        )]),
        top_evidence: vec![AdvisorEvidenceRef {
            event_id: "evt-known".into(),
            why_it_matters: "The agent needs a concrete next step.".into(),
        }],
        cited_evidence_ids: vec!["evt-known".into()],
        missing_evidence: vec!["next concrete step".into()],
        proposed_action: ControlAction::SendFollowUp { target_agent: None },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Plan,
            delta: -25,
        }],
        packet_intent: Some("require a bounded next step".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::FollowUp,
            summary: "Continue with one bounded next step.".into(),
            instructions: vec!["Take one concrete implementation step.".into()],
            evidence_refs: vec!["evt-known".into()],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("out-of-range advisor score should be rejected");

    assert!(err.to_string().contains("score"));
}

#[test]
fn advisor_validation_rejects_missing_dominant_entropy_score() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-missing-dominant-score".into()),
        dominant_entropy: EntropyKind::Verification,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Plan,
            AdvisorEntropyEstimate {
                score: 65,
                confidence: 80,
            },
        )]),
        top_evidence: vec![AdvisorEvidenceRef {
            event_id: "evt-known".into(),
            why_it_matters: "The agent needs verification before completion.".into(),
        }],
        cited_evidence_ids: vec!["evt-known".into()],
        missing_evidence: vec!["passing verifier result".into()],
        proposed_action: ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Verification,
            delta: -55,
        }],
        packet_intent: Some("require verification".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::Urgent,
            summary: "Verify before continuing.".into(),
            instructions: vec!["Run the full verifier.".into()],
            evidence_refs: vec!["evt-known".into()],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("dominant entropy must have a score");

    assert!(err.to_string().contains("dominant entropy"));
}

#[test]
fn advisor_validation_rejects_out_of_range_expected_entropy_delta() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-bad-delta".into()),
        dominant_entropy: EntropyKind::Plan,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Plan,
            AdvisorEntropyEstimate {
                score: 65,
                confidence: 80,
            },
        )]),
        top_evidence: vec![AdvisorEvidenceRef {
            event_id: "evt-known".into(),
            why_it_matters: "The agent needs a concrete next step.".into(),
        }],
        cited_evidence_ids: vec!["evt-known".into()],
        missing_evidence: vec!["next concrete step".into()],
        proposed_action: ControlAction::SendFollowUp { target_agent: None },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Plan,
            delta: -150,
        }],
        packet_intent: Some("require a bounded next step".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::FollowUp,
            summary: "Continue with one bounded next step.".into(),
            instructions: vec!["Take one concrete implementation step.".into()],
            evidence_refs: vec!["evt-known".into()],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("out-of-range advisor delta should be rejected");

    assert!(err.to_string().contains("entropy delta"));
}

#[test]
fn advisor_validation_rejects_duplicate_expected_entropy_delta_kind() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-duplicate-delta".into()),
        dominant_entropy: EntropyKind::Plan,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Plan,
            AdvisorEntropyEstimate {
                score: 65,
                confidence: 80,
            },
        )]),
        top_evidence: vec![AdvisorEvidenceRef {
            event_id: "evt-known".into(),
            why_it_matters: "The agent needs a concrete next step.".into(),
        }],
        cited_evidence_ids: vec!["evt-known".into()],
        missing_evidence: vec!["next concrete step".into()],
        proposed_action: ControlAction::SendFollowUp { target_agent: None },
        expected_entropy_delta: vec![
            EntropyDelta {
                kind: EntropyKind::Plan,
                delta: -25,
            },
            EntropyDelta {
                kind: EntropyKind::Plan,
                delta: -10,
            },
        ],
        packet_intent: Some("require a bounded next step".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::FollowUp,
            summary: "Continue with one bounded next step.".into(),
            instructions: vec!["Take one concrete implementation step.".into()],
            evidence_refs: vec!["evt-known".into()],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("duplicate advisor delta kind should be rejected");

    assert!(err.to_string().contains("duplicate entropy delta"));
}

#[test]
fn advisor_validation_rejects_unknown_explicit_target_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-unknown-target".into()),
        dominant_entropy: EntropyKind::Plan,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Plan,
            AdvisorEntropyEstimate {
                score: 65,
                confidence: 80,
            },
        )]),
        top_evidence: vec![AdvisorEvidenceRef {
            event_id: "evt-known".into(),
            why_it_matters: "The agent needs a concrete next step.".into(),
        }],
        cited_evidence_ids: vec!["evt-known".into()],
        missing_evidence: vec!["next concrete step".into()],
        proposed_action: ControlAction::SendFollowUp {
            target_agent: Some("invented-agent".into()),
        },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Plan,
            delta: -25,
        }],
        packet_intent: Some("require a bounded next step".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::FollowUp,
            summary: "Continue with one bounded next step.".into(),
            instructions: vec!["Take one concrete implementation step.".into()],
            evidence_refs: vec!["evt-known".into()],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("unknown advisor target should be rejected");

    assert!(err.to_string().contains("invented-agent"));
}

#[test]
fn advisor_validation_rejects_tainted_non_packet_diagnostics() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-tainted-diagnostic".into()),
        dominant_entropy: EntropyKind::UserDecision,
        entropy_scores: BTreeMap::from([(
            EntropyKind::UserDecision,
            AdvisorEntropyEstimate {
                score: 90,
                confidence: 80,
            },
        )]),
        top_evidence: vec![AdvisorEvidenceRef {
            event_id: "evt-known".into(),
            why_it_matters: "authorization: bearer leaked-token".into(),
        }],
        cited_evidence_ids: vec!["evt-known".into()],
        missing_evidence: vec!["api_key=missing-evidence-secret".into()],
        proposed_action: ControlAction::AskUser {
            question: "Need a product decision.".into(),
        },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::UserDecision,
            delta: -70,
        }],
        packet_intent: Some("ask one bounded question".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::FollowUp,
            summary: "Ask the user only for the unresolved decision.".into(),
            instructions: vec!["Wait for the bounded decision before editing.".into()],
            evidence_refs: vec!["evt-known".into()],
        },
        ask_user: Some(json!({
            "question": "Which credential should use password=secret?",
            "options": ["Use default"]
        })),
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("tainted advisor diagnostics should be rejected");

    assert!(err.to_string().contains("tainted"));
}

#[test]
fn advisor_validation_rejects_pause_even_if_advisor_attempts_to_stop_work() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-pause".into()),
        dominant_entropy: EntropyKind::Plan,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Plan,
            AdvisorEntropyEstimate {
                score: 20,
                confidence: 80,
            },
        )]),
        top_evidence: vec![],
        cited_evidence_ids: vec![],
        missing_evidence: vec![],
        proposed_action: ControlAction::Pause {
            reason: "advisor wants to stop".into(),
        },
        expected_entropy_delta: vec![],
        packet_intent: Some("pause".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::Urgent,
            summary: "Pause.".into(),
            instructions: vec!["Stop work.".into()],
            evidence_refs: vec![],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    assert!(
        !case_file
            .allowed_actions
            .contains(&ControlActionKind::Pause)
    );
    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("advisor should not be allowed to pause the monitor");

    assert!(err.to_string().contains("forbidden action"));
}

#[test]
fn advisor_validation_rejects_unsupported_run_probe_spec_even_when_run_probe_is_allowed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-probe-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Fix the dashboard issue without asking me routine questions.".into()),
            ..Event::default()
        })
        .expect("goal event");
    store
        .append_event(&Event {
            event_id: Some("evt-probe-question".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Which browser validation should I run next?".into()),
            ..Event::default()
        })
        .expect("question event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    assert!(
        case_file
            .allowed_actions
            .contains(&ControlActionKind::RunProbe),
        "case file should allow run_probe for probe-worthy plan entropy: {:?}",
        case_file.allowed_actions
    );
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-unsupported-probe".into()),
        dominant_entropy: EntropyKind::Plan,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Plan,
            AdvisorEntropyEstimate {
                score: 75,
                confidence: 80,
            },
        )]),
        top_evidence: vec![AdvisorEvidenceRef {
            event_id: "evt-probe-question".into(),
            why_it_matters: "The agent asked a routine local-evidence question.".into(),
        }],
        cited_evidence_ids: vec!["evt-probe-question".into()],
        missing_evidence: vec!["monitor-owned browser validation result".into()],
        proposed_action: ControlAction::RunProbe {
            probe: ProbeSpec::BrowserValidation {
                target: Some("dashboard".into()),
            },
        },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Plan,
            delta: -20,
        }],
        packet_intent: Some("collect browser evidence".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::FollowUp,
            summary: "Run browser validation.".into(),
            instructions: vec!["Use browser validation.".into()],
            evidence_refs: vec!["evt-probe-question".into()],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("unsupported probe specs should be rejected before dispatch");

    assert!(err.to_string().contains("unsupported probe"));
}

#[test]
fn advisor_validation_accepts_runtime_validation_probe_when_run_probe_is_allowed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-runtime-probe-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some(
                "Fix the mobile flow without asking routine validation questions.".into(),
            ),
            ..Event::default()
        })
        .expect("goal event");
    store
        .append_event(&Event {
            event_id: Some("evt-runtime-probe-question".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Should I run the emulator flow or ask you first?".into()),
            ..Event::default()
        })
        .expect("question event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    assert!(
        case_file
            .allowed_actions
            .contains(&ControlActionKind::RunProbe),
        "case file should allow run_probe for probe-worthy plan entropy: {:?}",
        case_file.allowed_actions
    );
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-runtime-probe".into()),
        dominant_entropy: EntropyKind::Plan,
        entropy_scores: BTreeMap::from([(
            EntropyKind::Plan,
            AdvisorEntropyEstimate {
                score: 75,
                confidence: 80,
            },
        )]),
        top_evidence: vec![AdvisorEvidenceRef {
            event_id: "evt-runtime-probe-question".into(),
            why_it_matters: "The agent asked a routine runtime validation question.".into(),
        }],
        cited_evidence_ids: vec!["evt-runtime-probe-question".into()],
        missing_evidence: vec!["monitor-owned runtime validation evidence for mobile app".into()],
        proposed_action: ControlAction::RunProbe {
            probe: ProbeSpec::RuntimeValidation {
                surface: RuntimeValidationSurface::MobileApp,
                target: Some("login flow".into()),
            },
        },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Plan,
            delta: -20,
        }],
        packet_intent: Some("collect runtime validation evidence".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::FollowUp,
            summary: "Run runtime validation evidence probe.".into(),
            instructions: vec!["Use the mobile runtime surface, not a browser-only probe.".into()],
            evidence_refs: vec!["evt-runtime-probe-question".into()],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    validate_advisor_decision(&decision, &case_file)
        .expect("runtime_validation is a supported generic probe spec");
}

#[test]
fn advisor_validation_rejects_handoff_when_case_file_has_no_safe_adapter_target() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut config = ProjectConfig::default();
    config.adapters.codex.enabled = Some(false);
    config.adapters.claude_code.enabled = Some(false);
    config.adapters.opencode.enabled = Some(false);
    let snapshot =
        DashboardSnapshot::load(ProjectStore::open(temp.path()).expect("store").root(), 20)
            .expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let decision = AdvisorDecision {
        diagnosis_id: Some("diagnosis-no-target".into()),
        dominant_entropy: EntropyKind::AgentHealth,
        entropy_scores: BTreeMap::from([(
            EntropyKind::AgentHealth,
            AdvisorEntropyEstimate {
                score: 85,
                confidence: 80,
            },
        )]),
        top_evidence: vec![],
        cited_evidence_ids: vec![],
        missing_evidence: vec![],
        proposed_action: ControlAction::SwitchAgent {
            target_agent: "opencode".into(),
        },
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::AgentHealth,
            delta: -30,
        }],
        packet_intent: Some("switch away from degraded agent".into()),
        packet_draft: AdvisorPacketDraft {
            urgency: PacketUrgency::Urgent,
            summary: "Switch agent.".into(),
            instructions: vec!["Start a fallback agent.".into()],
            evidence_refs: vec![],
        },
        ask_user: None,
        confidence: 0.8,
        raw: json!({}),
    };

    let err = validate_advisor_decision(&decision, &case_file)
        .expect_err("handoff should be forbidden when no safe target exists");

    match err {
        coding_agent_monitor::AdvisorValidationError::ForbiddenAction { action } => {
            assert_eq!(action, ControlActionKind::SwitchAgent);
        }
        other => panic!("expected forbidden switch_agent action, got {other:?}"),
    }
}
