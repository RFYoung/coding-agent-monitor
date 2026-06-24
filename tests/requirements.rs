use coding_agent_monitor::{
    AcceptanceCoverageStatus, ActionOutcome, AdviceRun, CompletionCertificateStatus, ControlAction,
    ControlActionKind, ControlCaseFile, ControlPacket, ControlRationale, DashboardSnapshot,
    DispatchResult, DispatchStatus, OutcomeStatus, PacketPreconditions, PacketUrgency,
    ProjectStore, RepoChangeKind, RepoHunkHistoryEntry, RepoHunkTraceRef, RepoTraceStatus,
    RequirementEvidenceNecessity, RequirementGraphQuery, RequirementNode, RequirementSource,
    TraceEntry, ValidationOutcome, VerificationStatus, build_control_case_file,
    load_completion_certificate_report, load_requirement_graph, record_trace_entry,
};
use std::path::Path;

fn requirement(id: &str, text: &str, status: AcceptanceCoverageStatus) -> RequirementNode {
    RequirementNode {
        requirement_id: id.into(),
        source: RequirementSource::AcceptanceCriterion,
        text: text.into(),
        source_event_id: Some(format!("evt-source-{id}")),
        evidence_ids: vec![format!("evt-source-{id}")],
        evidence_refs: Vec::new(),
        verifier_ids: vec![format!("verifier-{id}")],
        verifier_commands: vec![format!("cargo test {id}")],
        latest_verification_evidence_id: Some(format!("evt-verify-{id}")),
        status,
        latest_status: Some(match status {
            AcceptanceCoverageStatus::Covered => VerificationStatus::Passed,
            AcceptanceCoverageStatus::Failed => VerificationStatus::Failed,
            AcceptanceCoverageStatus::Stale => VerificationStatus::Stale,
            AcceptanceCoverageStatus::Unverified | AcceptanceCoverageStatus::Unmapped => {
                VerificationStatus::NotRun
            }
        }),
    }
}

fn append_case_file(
    store: &mut ProjectStore,
    workspace: &Path,
    case_file_id: &str,
    requirements: Vec<RequirementNode>,
) {
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let mut case_file: ControlCaseFile = build_control_case_file(workspace, &snapshot);
    case_file.case_file_id = case_file_id.into();
    case_file.requirements = requirements;
    store.append_case_file(&case_file).expect("case file");
}

#[test]
fn completion_certificate_report_blocks_empty_requirement_scope() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_case_file(&mut store, temp.path(), "case-without-scope", Vec::new());

    let report = load_completion_certificate_report(
        temp.path(),
        RequirementGraphQuery {
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("completion certificate report");

    assert_eq!(
        report.certificate.status,
        CompletionCertificateStatus::Blocked
    );
    assert_eq!(report.certificate.scoped_requirement_ids.len(), 0);
    assert!(
        report
            .certificate
            .unresolved_incidents
            .iter()
            .any(|incident| incident.kind == "requirement_scope"
                && incident.missing_evidence.contains(
                    &"extracted acceptance criteria or durable scoped requirements".into()
                )),
        "{report:?}"
    );
}

#[test]
fn completion_certificate_report_groups_latest_requirement_closure() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_case_file(
        &mut store,
        temp.path(),
        "case-old",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Stale,
        )],
    );
    append_case_file(
        &mut store,
        temp.path(),
        "case-new",
        vec![
            requirement(
                "req-api",
                "API endpoint must call the dedicated advisor.",
                AcceptanceCoverageStatus::Covered,
            ),
            requirement(
                "req-docs",
                "Documentation must explain coding-plan credentials.",
                AcceptanceCoverageStatus::Unmapped,
            ),
        ],
    );

    let report = load_completion_certificate_report(
        temp.path(),
        RequirementGraphQuery {
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("completion certificate report");

    assert_eq!(
        report.certificate.status,
        CompletionCertificateStatus::Blocked
    );
    assert_eq!(report.certificate.scoped_requirement_ids.len(), 2);
    assert_eq!(
        report.certificate.closed_requirement_ids,
        vec!["req-api".to_string()]
    );
    assert_eq!(
        report.certificate.unresolved_requirement_ids,
        vec!["req-docs".to_string()]
    );
    assert!(
        report
            .certificate
            .unresolved_incidents
            .iter()
            .any(|incident| incident.summary.contains("requirement closure")),
        "{report:?}"
    );
}

#[test]
fn completion_certificate_report_surfaces_verification_and_proof_gaps() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_case_file(
        &mut store,
        temp.path(),
        "case-claim-only",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Covered,
        )],
    );

    let report = load_completion_certificate_report(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-api".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("completion certificate report");

    assert_eq!(
        report.certificate.status,
        CompletionCertificateStatus::Blocked
    );
    assert_eq!(
        report.certificate.verification_status,
        VerificationStatus::Passed
    );
    assert!(
        report
            .certificate
            .verifier_commands
            .contains(&"cargo test req-api".to_string())
    );
    assert_eq!(report.proof_gaps.len(), 1);
    assert_eq!(report.proof_gaps[0].requirement_id, "req-api");
    assert!(
        report.proof_gaps[0]
            .gaps
            .contains(&"no_trace_refs".to_string()),
        "{report:?}"
    );
    assert!(
        report
            .certificate
            .unresolved_incidents
            .iter()
            .any(|incident| incident.summary.contains("proof gap")),
        "{report:?}"
    );
}

#[test]
fn requirements_query_returns_latest_requirement_nodes_with_filters() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_case_file(
        &mut store,
        temp.path(),
        "case-old",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Stale,
        )],
    );
    append_case_file(
        &mut store,
        temp.path(),
        "case-new",
        vec![
            requirement(
                "req-api",
                "API endpoint must call the dedicated advisor.",
                AcceptanceCoverageStatus::Covered,
            ),
            requirement(
                "req-docs",
                "Documentation must explain coding-plan credentials.",
                AcceptanceCoverageStatus::Unmapped,
            ),
        ],
    );

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            status: None,
            requirement_id: None,
            text: None,
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.case_file_count, 2);
    assert_eq!(report.requirement_count, 2);
    assert_eq!(report.requirements.len(), 2);
    assert_eq!(report.requirements[0].requirement_id, "req-api");
    assert_eq!(
        report.requirements[0].status,
        AcceptanceCoverageStatus::Covered
    );

    let filtered = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            status: Some(AcceptanceCoverageStatus::Unmapped),
            requirement_id: None,
            text: Some("coding-plan".into()),
            limit: 1,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("filtered requirements report");

    assert_eq!(filtered.case_file_count, 2);
    assert_eq!(filtered.requirement_count, 1);
    assert_eq!(filtered.requirements.len(), 1);
    assert_eq!(filtered.requirements[0].requirement_id, "req-docs");
}

#[test]
fn requirements_query_returns_proof_history_for_latest_requirement() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_case_file(
        &mut store,
        temp.path(),
        "case-old",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Stale,
        )],
    );
    append_case_file(
        &mut store,
        temp.path(),
        "case-new",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Covered,
        )],
    );

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.requirements.len(), 1);
    assert_eq!(report.requirements[0].requirement_id, "req-api");
    assert_eq!(
        report.requirements[0].status,
        AcceptanceCoverageStatus::Covered
    );
    assert_eq!(report.proofs.len(), 2);
    assert_eq!(report.proofs[0].requirement_id, "req-api");
    assert_eq!(report.proofs[0].case_file_id, "case-new");
    assert_eq!(report.proofs[0].status, AcceptanceCoverageStatus::Covered);
    assert_eq!(
        report.proofs[0].latest_verification_evidence_id.as_deref(),
        Some("evt-verify-req-api")
    );
    assert_eq!(report.proofs[1].requirement_id, "req-api");
    assert_eq!(report.proofs[1].case_file_id, "case-old");
    assert_eq!(report.proofs[1].status, AcceptanceCoverageStatus::Stale);
}

#[test]
fn requirements_query_preserves_durable_memory_source_in_proof_history() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let mut memory_req = requirement(
        "req-memory-adapter-constraint",
        "Adapters must support Codex, Claude Code, Pi, and OpenCode.",
        AcceptanceCoverageStatus::Covered,
    );
    memory_req.source = RequirementSource::DurableMemory;
    memory_req.verifier_ids.clear();
    memory_req.verifier_commands.clear();
    memory_req.latest_verification_evidence_id = None;
    append_case_file(&mut store, temp.path(), "case-memory", vec![memory_req]);

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-memory-adapter-constraint".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.requirements.len(), 1);
    assert_eq!(
        report.requirements[0].source,
        RequirementSource::DurableMemory
    );
    assert_eq!(report.proofs.len(), 1);
    assert_eq!(report.proofs[0].source, RequirementSource::DurableMemory);
    assert!(report.proofs[0].verifier_ids.is_empty());
}

#[test]
fn requirements_query_links_proof_history_to_trace_evidence() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-source-req-api".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(10),
            line_end: Some(12),
            rationale: Some("Implement the API advisor requirement.".into()),
            related_event_ids: vec!["evt-user-api".into()],
            ..TraceEntry::default()
        })
        .expect("trace");
    append_case_file(
        &mut store,
        temp.path(),
        "case-with-trace-proof",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Covered,
        )],
    );

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-api".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.proofs.len(), 1);
    assert_eq!(report.proofs[0].trace_refs.len(), 1);
    assert_eq!(
        report.proofs[0].trace_refs[0].event_id.as_deref(),
        Some("evt-source-req-api")
    );
    assert_eq!(report.proofs[0].trace_refs[0].file, "src/lib.rs");
    assert_eq!(report.proofs[0].trace_refs[0].line, Some(10));
    assert_eq!(report.proofs[0].trace_refs[0].line_end, Some(12));
    assert_eq!(
        report.proofs[0].trace_refs[0].necessity,
        RequirementEvidenceNecessity::Necessary
    );
    assert_eq!(
        report.proofs[0].trace_refs[0].rationale.as_deref(),
        Some("Implement the API advisor requirement.")
    );
}

#[test]
fn requirements_query_marks_shared_source_trace_as_correlated_for_sibling_requirement() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&coding_agent_monitor::Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-user-acceptance".into()),
            agent: "user".into(),
            kind: coding_agent_monitor::EventKind::UserInstruction,
            content: Some(
                "Acceptance criteria:\n- nested parser behavior passes.\n- export CSV report."
                    .into(),
            ),
            ..coding_agent_monitor::Event::default()
        })
        .expect("user event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    store.append_case_file(&case_file).expect("case file");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-parser-trace".into()),
            agent: "codex".into(),
            file: "src/parser.rs".into(),
            rationale: Some("Implement nested parser behavior.".into()),
            related_event_ids: vec!["evt-user-acceptance".into()],
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-export-csv-report".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.proofs.len(), 1);
    assert_eq!(report.proofs[0].trace_refs.len(), 1);
    assert_eq!(
        report.proofs[0].trace_refs[0].necessity,
        RequirementEvidenceNecessity::Correlated
    );
    assert!(
        !report.proofs[0]
            .proof_strength
            .signals
            .contains(&"direct_trace_rationale".into())
    );
    assert!(
        report.proofs[0]
            .proof_strength
            .gaps
            .contains(&"no_necessary_trace_refs".into())
    );
}

#[test]
fn requirements_query_treats_trace_requirement_id_as_necessary_proof() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&coding_agent_monitor::Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-user-acceptance".into()),
            agent: "user".into(),
            kind: coding_agent_monitor::EventKind::UserInstruction,
            content: Some(
                "Acceptance criteria:\n- nested parser behavior passes.\n- export CSV report."
                    .into(),
            ),
            ..coding_agent_monitor::Event::default()
        })
        .expect("user event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    store.append_case_file(&case_file).expect("case file");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-csv-trace".into()),
            agent: "codex".into(),
            file: "src/export.rs".into(),
            rationale: Some("Implement export CSV report requirement.".into()),
            related_event_ids: vec!["evt-user-acceptance".into()],
            requirement_ids: vec!["req-export-csv-report".into()],
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-export-csv-report".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.proofs.len(), 1);
    assert_eq!(report.proofs[0].trace_refs.len(), 1);
    assert_eq!(
        report.proofs[0].trace_refs[0].necessity,
        RequirementEvidenceNecessity::Necessary
    );
    assert_eq!(
        report.proofs[0].trace_refs[0].requirement_ids,
        vec!["req-export-csv-report".to_string()]
    );
    assert!(
        report.proofs[0]
            .proof_strength
            .signals
            .contains(&"direct_trace_rationale".into())
    );
}

#[test]
fn record_trace_entry_links_requirement_id_to_necessary_proof() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_case_file(
        &mut store,
        temp.path(),
        "case-with-recorded-trace",
        vec![requirement(
            "req-contract-every-meaningful-change",
            "Every meaningful change needs trace rationale.",
            AcceptanceCoverageStatus::Unverified,
        )],
    );

    record_trace_entry(
        temp.path(),
        TraceEntry {
            event_id: Some("evt-recorded-trace".into()),
            agent: "monitor".into(),
            file: "src/lib.rs".into(),
            rationale: Some("Record rationale for the project-contract requirement.".into()),
            requirement_ids: vec!["req-contract-every-meaningful-change".into()],
            ..TraceEntry::default()
        },
    )
    .expect("record trace");

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-contract-every-meaningful-change".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.proofs.len(), 1);
    assert_eq!(report.proofs[0].trace_refs.len(), 1);
    assert_eq!(
        report.proofs[0].trace_refs[0].necessity,
        RequirementEvidenceNecessity::Necessary
    );
    assert!(
        report.proofs[0]
            .proof_strength
            .signals
            .contains(&"direct_trace_rationale".into()),
        "{:?}",
        report.proofs[0].proof_strength
    );
}

#[test]
fn requirement_id_trace_links_matching_repo_hunk_as_necessary_proof() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            event_id: Some("evt-contract-trace".into()),
            agent: "codex".into(),
            file: "src/lib.rs".into(),
            line: Some(10),
            line_end: Some(20),
            rationale: Some("Implement the trace-rationale contract.".into()),
            requirement_ids: vec!["req-contract-trace-rationale".into()],
            ..TraceEntry::default()
        })
        .expect("trace");
    store
        .append_repo_hunk_history(&RepoHunkHistoryEntry {
            history_id: "hunk-contract-trace".into(),
            observed_at: "2026-06-24T15:10:00Z".into(),
            workspace: temp.path().display().to_string(),
            path: "src/lib.rs".into(),
            kind: RepoChangeKind::Modified,
            hunk_index: 0,
            old_start: 10,
            old_lines: 2,
            new_start: 10,
            new_lines: 3,
            trace_status: RepoTraceStatus::Traced,
            matching_trace_count: 1,
            change_trace_status: RepoTraceStatus::Traced,
            modified_at: None,
            matching_trace_refs: vec![RepoHunkTraceRef {
                event_id: Some("evt-contract-trace".into()),
                agent: Some("codex".into()),
                session: None,
                line: Some(10),
                line_end: Some(20),
                rationale: Some("Implement the trace-rationale contract.".into()),
                related_event_ids: Vec::new(),
            }],
        })
        .expect("repo hunk history");
    append_case_file(
        &mut store,
        temp.path(),
        "case-with-requirement-id-hunk",
        vec![requirement(
            "req-contract-trace-rationale",
            "Every meaningful change needs trace rationale.",
            AcceptanceCoverageStatus::Unverified,
        )],
    );

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-contract-trace-rationale".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.proofs[0].repo_hunks.len(), 1);
    assert_eq!(
        report.proofs[0].repo_hunks[0].necessity,
        RequirementEvidenceNecessity::Necessary
    );
    assert!(
        report.proofs[0]
            .proof_strength
            .signals
            .contains(&"repo_hunk_traced".into()),
        "{:?}",
        report.proofs[0].proof_strength
    );
    assert!(
        !report.proofs[0]
            .proof_strength
            .gaps
            .contains(&"no_repo_hunk_refs".into()),
        "{:?}",
        report.proofs[0].proof_strength
    );
}

#[test]
fn requirements_query_links_proof_history_to_repo_hunks_via_trace_evidence() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_repo_hunk_history(&RepoHunkHistoryEntry {
            history_id: "hunk-req-api".into(),
            observed_at: "2026-06-22T12:00:00Z".into(),
            workspace: temp.path().display().to_string(),
            path: "src/lib.rs".into(),
            kind: RepoChangeKind::Modified,
            hunk_index: 0,
            old_start: 10,
            old_lines: 2,
            new_start: 10,
            new_lines: 3,
            trace_status: RepoTraceStatus::Traced,
            matching_trace_count: 1,
            change_trace_status: RepoTraceStatus::Traced,
            modified_at: None,
            matching_trace_refs: vec![RepoHunkTraceRef {
                event_id: Some("evt-source-req-api".into()),
                agent: Some("codex".into()),
                session: Some("s1".into()),
                line: Some(10),
                line_end: Some(12),
                rationale: Some("Implement the API advisor requirement.".into()),
                related_event_ids: vec!["evt-user-api".into()],
            }],
        })
        .expect("repo hunk history");
    append_case_file(
        &mut store,
        temp.path(),
        "case-with-hunk-proof",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Covered,
        )],
    );

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-api".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.proofs.len(), 1);
    assert_eq!(report.proofs[0].repo_hunks.len(), 1);
    assert_eq!(report.proofs[0].repo_hunks[0].history_id, "hunk-req-api");
    assert_eq!(report.proofs[0].repo_hunks[0].path, "src/lib.rs");
    assert_eq!(report.proofs[0].repo_hunks[0].hunk_index, 0);
    assert_eq!(
        report.proofs[0].repo_hunks[0].trace_status,
        RepoTraceStatus::Traced
    );
    assert_eq!(
        report.proofs[0].repo_hunks[0].trace_event_ids,
        vec!["evt-source-req-api".to_string()]
    );
}

#[test]
fn requirements_query_links_proof_history_to_control_decisions_and_outcomes() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_case_file(
        &mut store,
        temp.path(),
        "case-with-control-proof",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Covered,
        )],
    );
    let packet = ControlPacket {
        packet_id: "packet-requirement-proof".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::Verification,
        title: "Verify requirement implementation".into(),
        summary: "Run the verifier for the API requirement.".into(),
        instructions: Vec::new(),
        evidence_refs: vec!["evt-source-req-api".into()],
        forbidden: Vec::new(),
        success_criteria: Vec::new(),
        preconditions: PacketPreconditions::default(),
    };
    store
        .append_advice(&AdviceRun {
            advice_id: "advice-requirement-proof".into(),
            case_file_id: "case-with-control-proof".into(),
            advisor_used: false,
            advisor_error: None,
            advisor_decision: None,
            validation_outcome: ValidationOutcome::Approved(ControlAction::ForceVerification {
                suite: coding_agent_monitor::VerificationSuite::Targeted,
                blocking: true,
            }),
            final_action: ControlAction::ForceVerification {
                suite: coding_agent_monitor::VerificationSuite::Targeted,
                blocking: true,
            },
            control_rationale: ControlRationale {
                selected_action: ControlActionKind::ForceVerification,
                dominant_entropy: None,
                reason: "Requirement needs fresh verification.".into(),
                expected_entropy_delta: Vec::new(),
                evidence_ids: vec!["evt-source-req-api".into()],
                requirement_ids: Vec::new(),
            },
            packet: packet.clone(),
            dispatch_result: DispatchResult {
                dispatch_id: "dispatch-requirement-proof".into(),
                packet_id: packet.packet_id.clone(),
                target_agent: packet.target_agent.clone(),
                status: DispatchStatus::OutboxWritten,
                path: Some(".agent-monitor/outbox/codex/latest.md".into()),
                reason: None,
            },
            packet_path: Some(".agent-monitor/outbox/codex/latest.md".into()),
        })
        .expect("advice");
    store
        .append_action_outcome(&ActionOutcome {
            outcome_id: "outcome-requirement-proof".into(),
            advice_id: "advice-requirement-proof".into(),
            action: ControlActionKind::ForceVerification,
            status: OutcomeStatus::Succeeded,
            expected_entropy_delta: Vec::new(),
            observed_entropy_delta: Vec::new(),
            observed_entropy_delta_evidence: Vec::new(),
            evidence_ids: vec!["evt-verify-req-api".into()],
            requirement_ids: Vec::new(),
            note: Some("Verifier passed.".into()),
        })
        .expect("outcome");

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-api".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.proofs.len(), 1);
    assert_eq!(report.proofs[0].control_refs.len(), 1);
    assert_eq!(
        report.proofs[0].control_refs[0].advice_id,
        "advice-requirement-proof"
    );
    assert_eq!(
        report.proofs[0].control_refs[0].action,
        ControlActionKind::ForceVerification
    );
    assert_eq!(
        report.proofs[0].control_refs[0].dispatch_id.as_deref(),
        Some("dispatch-requirement-proof")
    );
    assert_eq!(report.proofs[0].outcome_refs.len(), 1);
    assert_eq!(
        report.proofs[0].outcome_refs[0].outcome_id,
        "outcome-requirement-proof"
    );
    assert_eq!(
        report.proofs[0].outcome_refs[0].status,
        OutcomeStatus::Succeeded
    );
    assert_eq!(
        report.proofs[0].outcome_refs[0].evidence_ids,
        vec!["evt-verify-req-api".to_string()]
    );
}

#[test]
fn requirements_query_treats_control_and_outcome_requirement_ids_as_necessary_proof() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&coding_agent_monitor::Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-user-acceptance".into()),
            agent: "user".into(),
            kind: coding_agent_monitor::EventKind::UserInstruction,
            content: Some(
                "Acceptance criteria:\n- nested parser behavior passes.\n- export CSV report."
                    .into(),
            ),
            ..coding_agent_monitor::Event::default()
        })
        .expect("user event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    store.append_case_file(&case_file).expect("case file");
    let packet = ControlPacket {
        packet_id: "packet-csv-requirement-proof".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::Verification,
        title: "Verify CSV requirement".into(),
        summary: "Run the verifier for the CSV requirement.".into(),
        instructions: Vec::new(),
        evidence_refs: vec!["evt-user-acceptance".into()],
        forbidden: Vec::new(),
        success_criteria: Vec::new(),
        preconditions: PacketPreconditions::default(),
    };
    store
        .append_advice(&AdviceRun {
            advice_id: "advice-csv-requirement-proof".into(),
            case_file_id: case_file.case_file_id.clone(),
            advisor_used: false,
            advisor_error: None,
            advisor_decision: None,
            validation_outcome: ValidationOutcome::Approved(ControlAction::ForceVerification {
                suite: coding_agent_monitor::VerificationSuite::Targeted,
                blocking: true,
            }),
            final_action: ControlAction::ForceVerification {
                suite: coding_agent_monitor::VerificationSuite::Targeted,
                blocking: true,
            },
            control_rationale: ControlRationale {
                selected_action: ControlActionKind::ForceVerification,
                dominant_entropy: None,
                reason: "CSV requirement needs fresh verification.".into(),
                expected_entropy_delta: Vec::new(),
                evidence_ids: vec!["evt-user-acceptance".into()],
                requirement_ids: vec!["req-export-csv-report".into()],
            },
            packet: packet.clone(),
            dispatch_result: DispatchResult {
                dispatch_id: "dispatch-csv-requirement-proof".into(),
                packet_id: packet.packet_id.clone(),
                target_agent: packet.target_agent.clone(),
                status: DispatchStatus::OutboxWritten,
                path: Some(".agent-monitor/outbox/codex/latest.md".into()),
                reason: None,
            },
            packet_path: Some(".agent-monitor/outbox/codex/latest.md".into()),
        })
        .expect("advice");
    store
        .append_action_outcome(&ActionOutcome {
            outcome_id: "outcome-csv-requirement-proof".into(),
            advice_id: "advice-csv-requirement-proof".into(),
            action: ControlActionKind::ForceVerification,
            status: OutcomeStatus::Succeeded,
            expected_entropy_delta: Vec::new(),
            observed_entropy_delta: Vec::new(),
            observed_entropy_delta_evidence: Vec::new(),
            evidence_ids: vec!["evt-user-acceptance".into()],
            requirement_ids: vec!["req-export-csv-report".into()],
            note: Some("CSV verifier passed.".into()),
        })
        .expect("outcome");

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-export-csv-report".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.proofs.len(), 1);
    assert_eq!(
        report.proofs[0].control_refs[0].requirement_ids,
        vec!["req-export-csv-report".to_string()]
    );
    assert_eq!(
        report.proofs[0].control_refs[0].necessity,
        RequirementEvidenceNecessity::Necessary
    );
    assert_eq!(
        report.proofs[0].outcome_refs[0].requirement_ids,
        vec!["req-export-csv-report".to_string()]
    );
    assert_eq!(
        report.proofs[0].outcome_refs[0].necessity,
        RequirementEvidenceNecessity::Necessary
    );
    assert!(
        report.proofs[0]
            .proof_strength
            .signals
            .contains(&"monitor_control_decision".into())
    );
    assert!(
        report.proofs[0]
            .proof_strength
            .signals
            .contains(&"successful_outcome".into())
    );
}

#[test]
fn requirements_query_scores_cross_source_proof_strength() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-source-req-api".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(10),
            line_end: Some(12),
            rationale: Some("Implement the API advisor requirement.".into()),
            related_event_ids: vec!["evt-user-api".into()],
            ..TraceEntry::default()
        })
        .expect("trace");
    store
        .append_repo_hunk_history(&RepoHunkHistoryEntry {
            history_id: "hunk-req-api".into(),
            observed_at: "2026-06-22T12:00:00Z".into(),
            workspace: temp.path().display().to_string(),
            path: "src/lib.rs".into(),
            kind: RepoChangeKind::Modified,
            hunk_index: 0,
            old_start: 10,
            old_lines: 2,
            new_start: 10,
            new_lines: 3,
            trace_status: RepoTraceStatus::Traced,
            matching_trace_count: 1,
            change_trace_status: RepoTraceStatus::Traced,
            modified_at: None,
            matching_trace_refs: vec![RepoHunkTraceRef {
                event_id: Some("evt-source-req-api".into()),
                agent: Some("codex".into()),
                session: Some("s1".into()),
                line: Some(10),
                line_end: Some(12),
                rationale: Some("Implement the API advisor requirement.".into()),
                related_event_ids: vec!["evt-user-api".into()],
            }],
        })
        .expect("repo hunk history");
    append_case_file(
        &mut store,
        temp.path(),
        "case-with-full-proof",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Covered,
        )],
    );
    let packet = ControlPacket {
        packet_id: "packet-requirement-proof".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::Verification,
        title: "Verify requirement implementation".into(),
        summary: "Run the verifier for the API requirement.".into(),
        instructions: Vec::new(),
        evidence_refs: vec!["evt-source-req-api".into()],
        forbidden: Vec::new(),
        success_criteria: Vec::new(),
        preconditions: PacketPreconditions::default(),
    };
    store
        .append_advice(&AdviceRun {
            advice_id: "advice-requirement-proof".into(),
            case_file_id: "case-with-full-proof".into(),
            advisor_used: false,
            advisor_error: None,
            advisor_decision: None,
            validation_outcome: ValidationOutcome::Approved(ControlAction::ForceVerification {
                suite: coding_agent_monitor::VerificationSuite::Targeted,
                blocking: true,
            }),
            final_action: ControlAction::ForceVerification {
                suite: coding_agent_monitor::VerificationSuite::Targeted,
                blocking: true,
            },
            control_rationale: ControlRationale {
                selected_action: ControlActionKind::ForceVerification,
                dominant_entropy: None,
                reason: "Requirement needs fresh verification.".into(),
                expected_entropy_delta: Vec::new(),
                evidence_ids: vec!["evt-source-req-api".into()],
                requirement_ids: Vec::new(),
            },
            packet: packet.clone(),
            dispatch_result: DispatchResult {
                dispatch_id: "dispatch-requirement-proof".into(),
                packet_id: packet.packet_id.clone(),
                target_agent: packet.target_agent.clone(),
                status: DispatchStatus::OutboxWritten,
                path: Some(".agent-monitor/outbox/codex/latest.md".into()),
                reason: None,
            },
            packet_path: Some(".agent-monitor/outbox/codex/latest.md".into()),
        })
        .expect("advice");
    store
        .append_action_outcome(&ActionOutcome {
            outcome_id: "outcome-requirement-proof".into(),
            advice_id: "advice-requirement-proof".into(),
            action: ControlActionKind::ForceVerification,
            status: OutcomeStatus::Succeeded,
            expected_entropy_delta: Vec::new(),
            observed_entropy_delta: Vec::new(),
            observed_entropy_delta_evidence: Vec::new(),
            evidence_ids: vec!["evt-verify-req-api".into()],
            requirement_ids: Vec::new(),
            note: Some("Verifier passed.".into()),
        })
        .expect("outcome");

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-api".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    let strength = &report.proofs[0].proof_strength;
    assert!(strength.score >= 85, "{strength:?}");
    assert!(strength.signals.contains(&"direct_trace_rationale".into()));
    assert!(strength.signals.contains(&"repo_hunk_traced".into()));
    assert!(
        strength
            .signals
            .contains(&"monitor_control_decision".into())
    );
    assert!(strength.signals.contains(&"successful_outcome".into()));
    assert!(strength.gaps.is_empty(), "{strength:?}");
}

#[test]
fn requirements_query_reports_proof_strength_gaps_for_claim_only_proof() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_case_file(
        &mut store,
        temp.path(),
        "case-with-claim-only-proof",
        vec![requirement(
            "req-api",
            "API endpoint must call the dedicated advisor.",
            AcceptanceCoverageStatus::Unverified,
        )],
    );

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some("req-api".into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    let strength = &report.proofs[0].proof_strength;
    assert!(strength.score < 50, "{strength:?}");
    assert!(strength.signals.contains(&"source_evidence".into()));
    assert!(strength.gaps.contains(&"no_trace_refs".into()));
    assert!(strength.gaps.contains(&"no_repo_hunk_refs".into()));
    assert!(strength.gaps.contains(&"no_control_refs".into()));
    assert!(strength.gaps.contains(&"no_outcome_refs".into()));
}

#[test]
fn requirements_query_filters_by_max_proof_strength() {
    let temp = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-source-req-strong".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(10),
            line_end: Some(12),
            rationale: Some("Implement the strongly traced requirement.".into()),
            related_event_ids: Vec::new(),
            ..TraceEntry::default()
        })
        .expect("trace");
    store
        .append_repo_hunk_history(&RepoHunkHistoryEntry {
            history_id: "hunk-req-strong".into(),
            observed_at: "2026-06-22T12:00:00Z".into(),
            workspace: temp.path().display().to_string(),
            path: "src/lib.rs".into(),
            kind: RepoChangeKind::Modified,
            hunk_index: 0,
            old_start: 10,
            old_lines: 2,
            new_start: 10,
            new_lines: 3,
            trace_status: RepoTraceStatus::Traced,
            matching_trace_count: 1,
            change_trace_status: RepoTraceStatus::Traced,
            modified_at: None,
            matching_trace_refs: vec![RepoHunkTraceRef {
                event_id: Some("evt-source-req-strong".into()),
                agent: Some("codex".into()),
                session: Some("s1".into()),
                line: Some(10),
                line_end: Some(12),
                rationale: Some("Implement the strongly traced requirement.".into()),
                related_event_ids: Vec::new(),
            }],
        })
        .expect("repo hunk history");
    append_case_file(
        &mut store,
        temp.path(),
        "case-with-mixed-proof-strength",
        vec![
            requirement(
                "req-weak",
                "Weak requirement has only claim evidence.",
                AcceptanceCoverageStatus::Covered,
            ),
            requirement(
                "req-strong",
                "Strong requirement has trace and hunk evidence.",
                AcceptanceCoverageStatus::Covered,
            ),
        ],
    );

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            max_proof_score: Some(49),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.requirement_count, 1);
    assert_eq!(report.requirements.len(), 1);
    assert_eq!(report.requirements[0].requirement_id, "req-weak");
    assert_eq!(report.proofs.len(), 1);
    assert_eq!(report.proofs[0].requirement_id, "req-weak");
    assert!(report.proofs[0].proof_strength.score <= 49);
}
