use super::*;

#[test]
fn policy_validator_replaces_low_value_ask_user_with_continue_working() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::AskUser {
            question: "Should I continue?".into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert!(matches!(original, ControlAction::AskUser { .. }));
            assert_eq!(replacement, ControlAction::ContinueWorking);
            assert!(reason.contains("user-decision entropy is not high enough"));
        }
        other => panic!("expected ask_user replacement, got {other:?}"),
    }
}

#[test]
fn policy_validator_replaces_low_entropy_switch_agent_with_continue_working() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SwitchAgent {
                    target_agent: "claude-code".into(),
                }
            );
            assert_eq!(replacement, ControlAction::ContinueWorking);
            assert!(reason.contains("agent-health entropy"));
        }
        other => panic!("expected low-entropy switch replacement, got {other:?}"),
    }
}

#[test]
fn policy_validator_replaces_low_entropy_spawn_fresh_with_continue_working() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SpawnFreshAgent {
                    target_agent: Some("claude-code".into()),
                }
            );
            assert_eq!(replacement, ControlAction::ContinueWorking);
            assert!(reason.contains("context or agent-health entropy"));
        }
        other => panic!("expected low-entropy spawn replacement, got {other:?}"),
    }
}

#[test]
fn policy_validator_replaces_switch_agent_with_force_verification_when_verification_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-switch-unverified-completion".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Implementation complete; tests should pass now.".into()),
            ..Event::default()
        })
        .expect("completion claim");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-switch-service-failure-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("cargo test".into()),
                content: Some("provider unavailable while running verifier".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("service failure");
    }
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(
        case_file
            .entropy
            .score(EntropyKind::Verification)
            .is_some_and(|score| score.score >= 75),
        "fixture must have high verification entropy: {:?}",
        case_file.entropy.score(EntropyKind::Verification)
    );
    assert!(
        case_file
            .entropy
            .score(EntropyKind::AgentHealth)
            .is_some_and(|score| score.score >= 80),
        "fixture must have high agent-health entropy: {:?}",
        case_file.entropy.score(EntropyKind::AgentHealth)
    );

    let outcome = validate_control_action_detailed(
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SwitchAgent {
                    target_agent: "claude-code".into(),
                }
            );
            assert_eq!(
                replacement,
                ControlAction::ForceVerification {
                    suite: VerificationSuite::Full,
                    blocking: true,
                }
            );
            assert!(reason.contains("verification entropy is high"));
        }
        other => panic!("expected switch handoff to force verification, got {other:?}"),
    }
}

#[test]
fn policy_validator_replaces_spawn_fresh_with_force_verification_when_verification_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-spawn-lost-context".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("context loss");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-spawn-unverified-completion".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Implementation complete; tests should pass now.".into()),
            ..Event::default()
        })
        .expect("completion claim");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(
        case_file
            .entropy
            .score(EntropyKind::Verification)
            .is_some_and(|score| score.score >= 75),
        "fixture must have high verification entropy: {:?}",
        case_file.entropy.score(EntropyKind::Verification)
    );
    assert!(
        case_file
            .entropy
            .score(EntropyKind::Context)
            .is_some_and(|score| score.score >= 80),
        "fixture must have high context entropy: {:?}",
        case_file.entropy.score(EntropyKind::Context)
    );

    let outcome = validate_control_action_detailed(
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SpawnFreshAgent {
                    target_agent: Some("claude-code".into()),
                }
            );
            assert_eq!(
                replacement,
                ControlAction::ForceVerification {
                    suite: VerificationSuite::Full,
                    blocking: true,
                }
            );
            assert!(reason.contains("verification entropy is high"));
        }
        other => panic!("expected fresh handoff to force verification, got {other:?}"),
    }
}

#[test]
fn policy_validator_rejects_invalid_target_handoff_rewrite_when_entropy_is_low() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut config = ProjectConfig::default();
    config.adapters.claude_code.enabled = Some(false);
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);

    let outcome = validate_control_action_detailed(
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SpawnFreshAgent {
                    target_agent: Some("claude-code".into()),
                }
            );
            assert_eq!(replacement, ControlAction::ContinueWorking);
            assert!(reason.contains("disabled"));
            assert!(reason.contains("context or agent-health entropy"));
        }
        other => panic!("expected low-entropy target rewrite rejection, got {other:?}"),
    }
}

#[test]
fn policy_validator_replaces_low_entropy_retry_agent_with_continue_working() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::RetryAgent {
                    target_agent: Some("codex".into()),
                    max_attempts: 1,
                }
            );
            assert_eq!(replacement, ControlAction::ContinueWorking);
            assert!(reason.contains("agent-health entropy"));
        }
        other => panic!("expected low-entropy retry replacement, got {other:?}"),
    }
}

#[test]
fn policy_validator_allows_retry_agent_at_agent_health_retry_threshold() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let mut case_file = build_control_case_file(temp.path(), &snapshot);
    let score = case_file
        .entropy
        .scores
        .iter_mut()
        .find(|score| score.kind == EntropyKind::AgentHealth)
        .expect("agent-health score");
    score.score = 75;
    score.confidence = 90;
    score.trend = EntropyTrend::Rising;
    score.top_causes = vec!["codex repeated transient command failure".into()];

    let outcome = validate_control_action_detailed(
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        },
        &case_file,
    );

    assert!(matches!(
        outcome,
        ValidationOutcome::Approved(ControlAction::RetryAgent { .. })
    ));
}

#[test]
fn policy_validator_replaces_low_entropy_send_follow_up_with_continue_working() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::SendFollowUp { target_agent: None },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(original, ControlAction::SendFollowUp { target_agent: None });
            assert_eq!(replacement, ControlAction::ContinueWorking);
            assert!(reason.contains("follow-up entropy"));
        }
        other => panic!("expected low-entropy follow-up replacement, got {other:?}"),
    }
}

#[test]
fn user_decision_entropy_rises_for_credential_or_authority_blocker() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-credential-needed".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "I need the user's API key before I can verify the production integration.".into(),
            ),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let user_decision = case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .expect("user-decision score");

    assert!(user_decision.score >= 80);
    assert!(
        user_decision
            .evidence_ids
            .contains(&"evt-credential-needed".into())
    );
    assert!(
        user_decision
            .missing_evidence
            .contains(&"user authorization or required external input".into())
    );
}

#[test]
fn user_decision_entropy_rises_for_security_deny_path_file_change() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-env-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some(".env.production".into()),
            rationale: Some("Try production credentials during verification.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let score = case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .expect("user decision score");

    assert!(score.score >= 80, "{score:?}");
    assert!(score.evidence_ids.contains(&"evt-env-write".into()));
    assert!(
        score
            .top_causes
            .iter()
            .any(|cause| cause.contains("security deny path") && cause.contains(".env.production"))
    );
    assert!(
        score
            .missing_evidence
            .contains(&"user authorization or required external input".into())
    );
}

#[test]
fn user_decision_entropy_rises_for_nested_env_file_by_default() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-nested-env-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("services/api/.env.production".into()),
            rationale: Some("Try service credentials during verification.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let score = case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .expect("user decision score");

    assert!(score.score >= 80, "{score:?}");
    assert!(score.evidence_ids.contains(&"evt-nested-env-write".into()));
}

#[test]
fn user_decision_entropy_normalizes_dot_segments_for_security_paths() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-dot-segment-infra-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/../infra/prod/main.tf".into()),
            rationale: Some("Change production infrastructure default.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let score = case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .expect("user decision score");

    assert!(score.score >= 80, "{score:?}");
    assert!(
        score
            .top_causes
            .iter()
            .any(|cause| cause.contains("infra/prod/main.tf"))
    );
    assert!(
        score
            .evidence_ids
            .contains(&"evt-dot-segment-infra-write".into())
    );
}

#[test]
fn security_redact_env_false_disables_implicit_env_path_classification() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "security": {
            "redact_env": false,
            "redact_auth_files": false,
            "protected_paths": []
          }
        }"#,
    )
    .expect("config");
    store
        .append_event(&Event {
            event_id: Some("evt-env-write-disabled".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some(".env.production".into()),
            rationale: Some("Document local test environment.".into()),
            ..Event::default()
        })
        .expect("event");

    let config = ProjectConfig::load(store.root()).expect("config");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let score = case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .expect("user decision score");

    assert!(
        score.score < 80,
        "redact_env=false should suppress implicit env path classification: {score:?}"
    );
}

#[test]
fn advise_workspace_asks_user_for_protected_path_file_change() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-infra-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("infra/prod/main.tf".into()),
            rationale: Some("Change production infrastructure default.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    match advice.final_action {
        ControlAction::AskUser { question } => {
            assert!(question.contains("security protected path"));
            assert!(question.contains("infra/prod/main.tf"));
        }
        other => panic!("expected ask_user for protected path, got {other:?}"),
    }
    assert!(advice.packet.summary.contains("infra/prod/main.tf"));
}

#[test]
fn user_decision_entropy_rises_for_destructive_command_intent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-destructive-command".into()),
            agent: "codex".into(),
            kind: EventKind::CommandOutput,
            content: Some("tool command: git reset --hard".into()),
            command: Some("git reset --hard".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let user_decision = case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .expect("user-decision score");

    assert!(user_decision.score >= 80);
    assert!(
        user_decision
            .evidence_ids
            .contains(&"evt-destructive-command".into())
    );
    assert!(user_decision.top_causes.iter().any(|cause| {
        cause.contains("destructive command requires explicit user authorization")
    }));
}

#[test]
fn user_decision_entropy_rises_for_split_rm_recursive_force_flags() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-rm-split-force".into()),
            agent: "codex".into(),
            kind: EventKind::CommandOutput,
            content: Some("tool command: rm -r -f build".into()),
            command: Some("rm -r -f build".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let user_decision = case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .expect("user-decision score");

    assert!(user_decision.score >= 80);
    assert!(
        user_decision
            .evidence_ids
            .contains(&"evt-rm-split-force".into())
    );
}

#[test]
fn ordinary_readonly_command_does_not_raise_user_decision_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-readonly-command".into()),
            agent: "codex".into(),
            kind: EventKind::CommandOutput,
            content: Some("tool command: git status".into()),
            command: Some("git status".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(
        case_file
            .entropy
            .score(EntropyKind::UserDecision)
            .is_none_or(|score| score.score < 80)
    );
}

#[test]
fn git_clean_without_force_flag_does_not_raise_user_decision_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-git-clean-path".into()),
            agent: "codex".into(),
            kind: EventKind::CommandOutput,
            content: Some("tool command: git clean foo.txt".into()),
            command: Some("git clean foo.txt".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(
        case_file
            .entropy
            .score(EntropyKind::UserDecision)
            .is_none_or(|score| score.score < 80)
    );
}

#[test]
fn ordinary_continue_question_does_not_raise_user_decision_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-continue-question".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("This is a good point to stop. Should I continue?".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(
        case_file
            .entropy
            .score(EntropyKind::UserDecision)
            .is_none_or(|score| score.score < 70)
    );
    assert!(
        case_file
            .entropy
            .score(EntropyKind::Plan)
            .is_some_and(|score| score.score >= 60)
    );
}

#[test]
fn policy_validator_approves_ask_user_when_user_decision_entropy_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-delete-prod".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "Should I delete the production database backup before continuing?".into(),
            ),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let bounded_question = "User authorization is required before continuing: destructive or external side-effect consent is required. Provide the required decision or input.";
    let outcome = validate_control_action_detailed(
        ControlAction::AskUser {
            question: bounded_question.into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Approved(ControlAction::AskUser { question }) => {
            assert_eq!(question, bounded_question);
        }
        other => panic!("expected ask_user approval, got {other:?}"),
    }
}

#[test]
fn policy_validator_rewrites_high_entropy_ask_user_to_bounded_monitor_question() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-delete-prod".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "Should I delete the production database backup before continuing?".into(),
            ),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let outcome = validate_control_action_detailed(
        ControlAction::AskUser {
            question: "Should I continue?".into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::AskUser {
                    question: "Should I continue?".into(),
                }
            );
            let ControlAction::AskUser { question } = replacement else {
                panic!("expected replacement ask_user, got {replacement:?}");
            };
            assert!(question.contains("User authorization is required"));
            assert!(question.contains("destructive or external side-effect consent is required"));
            assert!(!question.contains("Should I continue?"));
            assert!(reason.contains("bounded monitor question"));
        }
        other => panic!("expected ask_user rewrite, got {other:?}"),
    }
}

#[test]
fn policy_validator_replaces_mid_band_ask_user_until_user_decision_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let mut case_file = build_control_case_file(temp.path(), &snapshot);
    let score = case_file
        .entropy
        .scores
        .iter_mut()
        .find(|score| score.kind == EntropyKind::UserDecision)
        .expect("user-decision score");
    score.score = 75;
    score.confidence = 90;
    score.top_causes = vec!["credentials or secret access are required".into()];

    let outcome = validate_control_action_detailed(
        ControlAction::AskUser {
            question: "Please provide the required credential.".into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert!(matches!(original, ControlAction::AskUser { .. }));
            assert_eq!(replacement, ControlAction::ContinueWorking);
            assert!(reason.contains("user-decision entropy is not high enough"));
        }
        other => panic!("expected mid-band ask_user replacement, got {other:?}"),
    }
}

#[test]
fn advise_workspace_selects_bounded_ask_user_for_user_authority_blocker() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-needs-credential".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "Need credentials from the user before calling the external billing API.".into(),
            ),
            ..Event::default()
        })
        .expect("event");
    drop(store);

    let advice = coding_agent_monitor::advise_workspace(temp.path()).expect("advice");

    assert!(matches!(advice.final_action, ControlAction::AskUser { .. }));
    assert_eq!(advice.packet.urgency, PacketUrgency::Urgent);
    assert_eq!(advice.packet.title, "User decision required");
    assert!(
        advice
            .packet
            .summary
            .contains("User authorization is required")
    );
    assert!(
        advice
            .packet
            .evidence_refs
            .contains(&"evt-needs-credential".into())
    );
}

#[test]
fn advise_workspace_selects_bounded_ask_user_for_destructive_command_intent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-git-clean".into()),
            agent: "codex".into(),
            kind: EventKind::CommandOutput,
            content: Some("tool command: git clean -fdx".into()),
            command: Some("git clean -fdx".into()),
            ..Event::default()
        })
        .expect("event");
    drop(store);

    let advice = coding_agent_monitor::advise_workspace(temp.path()).expect("advice");

    assert!(matches!(advice.final_action, ControlAction::AskUser { .. }));
    assert_eq!(advice.packet.urgency, PacketUrgency::Urgent);
    assert_eq!(advice.packet.title, "User decision required");
    assert!(
        advice
            .packet
            .summary
            .contains("destructive command requires explicit user authorization")
    );
    assert!(
        advice
            .packet
            .evidence_refs
            .contains(&"evt-git-clean".into())
    );
}

#[test]
fn advise_workspace_downgrades_ask_user_when_hourly_interrupt_budget_is_exhausted() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "max_user_questions_per_hour": 1
          }
        }"#,
    )
    .expect("config");
    store
        .append_event(&Event {
            event_id: Some("evt-needs-credential-budget".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Need credentials from the user before calling production API.".into()),
            ..Event::default()
        })
        .expect("event");
    drop(store);

    let first_advice = coding_agent_monitor::advise_workspace(temp.path()).expect("first advice");
    assert!(matches!(
        first_advice.final_action,
        ControlAction::AskUser { .. }
    ));

    let advice = coding_agent_monitor::advise_workspace(temp.path()).expect("advice");

    assert!(matches!(
        advice.validation_outcome,
        ValidationOutcome::Modified { .. }
    ));
    assert!(matches!(advice.final_action, ControlAction::Pause { .. }));
    assert_eq!(advice.packet.urgency, PacketUrgency::Urgent);
    assert_eq!(advice.packet.title, "Monitor paused");
    let ValidationOutcome::Modified { reason, .. } = advice.validation_outcome else {
        panic!("expected modified advice");
    };
    assert!(reason.contains("user interrupt budget"));
}

#[test]
fn advise_workspace_allows_ask_user_when_prior_interrupt_is_outside_hour_window() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "max_user_questions_per_hour": 1
          }
        }"#,
    )
    .expect("config");
    append_ask_user_advice_at(&mut store, temp.path(), "1970-01-01T00:00:00Z");
    store
        .append_event(&Event {
            event_id: Some("evt-needs-credential-old-budget".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Need credentials from the user before calling production API.".into()),
            ..Event::default()
        })
        .expect("event");
    drop(store);

    let advice = coding_agent_monitor::advise_workspace(temp.path()).expect("advice");

    assert!(matches!(advice.final_action, ControlAction::AskUser { .. }));
}

#[test]
fn advisor_request_forbids_ask_user_when_interrupt_budget_is_exhausted() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_ask_user_advice_at(&mut store, temp.path(), "9999-01-01T00:00:00Z");
    store
        .append_event(&Event {
            event_id: Some("evt-needs-credential-advisor-budget".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Need credentials from the user before calling production API.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-budget-pruned",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 30, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [],
        "packet_intent": "continue without interrupting the user",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue without another user interrupt.",
            "instructions": ["Continue with available evidence."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.7
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_ASK_BUDGET_PRUNED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "policy": {
                "max_user_questions_per_hour": 1
            },
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_used);
    assert!(!allowed.contains(&"ask_user"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("ask_user")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("user interrupt budget"))
    }));
}

#[test]
fn advisor_request_forbids_ask_user_when_user_decision_entropy_is_low() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-continue-question-advisor".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("This is a good point to stop. Should I continue?".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-low-user-decision",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 60, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": { "type": "continue_working" },
        "expected_entropy_delta": [],
        "packet_intent": "continue without interrupting the user",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue without asking the user.",
            "instructions": ["Continue with the obvious next step."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.7
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_LOW_USER_DECISION_PRUNED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_used);
    assert!(!allowed.contains(&"ask_user"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("ask_user")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("user-decision entropy"))
    }));
}

#[test]
fn advisor_request_allows_ask_user_when_user_decision_entropy_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-needs-credential-advisor".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Need credentials from the user before calling production API.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-user-decision-allowed",
        "dominant_entropy": "user_decision",
        "entropy_scores": {
            "user_decision": { "score": 90, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-needs-credential-advisor",
                "why_it_matters": "Credentials require user authority."
            }
        ],
        "cited_evidence_ids": ["evt-needs-credential-advisor"],
        "missing_evidence": ["user credential decision"],
        "proposed_action": {
            "type": "ask_user",
            "question": "Should I continue?"
        },
        "expected_entropy_delta": [
            { "kind": "user_decision", "delta": -80 }
        ],
        "packet_intent": "ask for credential authorization",
        "packet_draft": {
            "urgency": "urgent",
            "summary": "Ask for the credential decision.",
            "instructions": ["Ask the bounded authorization question."],
            "evidence_refs": ["evt-needs-credential-advisor"]
        },
        "ask_user": {
            "question": "Should I continue?",
            "options": ["Yes", "No"]
        },
        "confidence": 0.7
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_HIGH_USER_DECISION_ALLOWED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_used);
    assert!(advice.advisor_error.is_none());
    assert!(allowed.contains(&"ask_user"));
    assert!(matches!(advice.final_action, ControlAction::AskUser { .. }));
}
