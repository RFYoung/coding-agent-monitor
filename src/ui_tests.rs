//! Tests for the `agent-monitor-ui` binary, included via `#[path]` so they
//! can exercise the binary crate's private rendering helpers.

use super::*;

#[test]
fn parses_workspace_argument() {
    assert_eq!(
        parse_ui_options(["--workspace=F:/repo".to_string()]).workspaces,
        vec![PathBuf::from("F:/repo")]
    );
}

#[test]
fn parses_multiple_workspace_arguments() {
    assert_eq!(
        parse_ui_options([
            "--workspace=F:/repo-a".to_string(),
            "--workspace=F:/repo-b".to_string(),
        ])
        .workspaces,
        vec![PathBuf::from("F:/repo-a"), PathBuf::from("F:/repo-b")]
    );
}

#[test]
fn workspace_arguments_are_deduplicated() {
    assert_eq!(
        parse_ui_options([
            "--workspace=F:/repo-a".to_string(),
            "--workspace=F:/repo-a".to_string(),
        ])
        .workspaces,
        vec![PathBuf::from("F:/repo-a")]
    );
}

#[test]
fn ui_options_default_to_foreground_current_workspace() {
    assert_eq!(
        parse_ui_options(["--other=value".to_string()]),
        UiOptions {
            workspaces: vec![PathBuf::from(".")],
            background: false,
        }
    );
}

#[test]
fn parses_background_flag() {
    assert!(parse_ui_options(["--background".to_string()]).background);
}

#[test]
fn tray_menu_ids_map_to_commands() {
    assert_eq!(
        tray_command_from_id(&MenuId::new(TRAY_SHOW_ID)),
        Some(TrayCommand::Show)
    );
    assert_eq!(
        tray_command_from_id(&MenuId::new(TRAY_HIDE_ID)),
        Some(TrayCommand::Hide)
    );
    assert_eq!(
        tray_command_from_id(&MenuId::new(TRAY_QUIT_ID)),
        Some(TrayCommand::Quit)
    );
}

#[test]
fn background_viewport_starts_hidden_and_off_taskbar() {
    let viewport = build_viewport(true);

    assert_eq!(viewport.visible, Some(false));
    assert_eq!(viewport.taskbar, Some(false));
}

#[test]
fn foreground_viewport_starts_visible_and_on_taskbar() {
    let viewport = build_viewport(false);

    assert_eq!(viewport.visible, Some(true));
    assert_eq!(viewport.taskbar, Some(true));
}

#[test]
fn light_theme_configuration_forces_active_light_theme() {
    let ctx = egui::Context::default();
    ctx.set_theme(egui::Theme::Dark);

    configure_light_theme(&ctx);

    assert_eq!(ctx.theme(), egui::Theme::Light);
    assert_eq!(
        ctx.global_style().visuals.panel_fill,
        egui::Color32::from_rgb(246, 248, 251)
    );
}

#[test]
fn formats_unix_epoch_as_utc_timestamp() {
    assert_eq!(format_utc_seconds(0), "1970-01-01T00:00:00Z");
}

#[test]
fn formats_known_utc_timestamp() {
    assert_eq!(format_utc_seconds(1_782_130_800), "2026-06-22T12:20:00Z");
}

#[test]
fn workspace_status_is_empty_without_activity() {
    let workspace = WorkspaceState::new(PathBuf::from("F:/repo"));

    assert_eq!(workspace_status(&workspace), WorkspaceStatus::Empty);
}

#[test]
fn workspace_status_follows_snapshot_severity() {
    let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
    workspace.snapshot.event_count = 3;
    workspace.snapshot.severity = DashboardSeverity::Critical;

    assert_eq!(workspace_status(&workspace), WorkspaceStatus::Critical);
}

#[test]
fn workspace_status_reports_warning_for_stale_agents() {
    let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
    workspace.snapshot.event_count = 3;
    workspace
        .snapshot
        .agent_sessions
        .push(coding_agent_monitor::AgentSession {
            agent: "codex".into(),
            status: AgentActivityStatus::Stale,
            score: 0,
            events: 3,
            interventions: 0,
            last_seen: Some("2026-06-22T12:00:00Z".into()),
        });

    assert_eq!(workspace_status(&workspace), WorkspaceStatus::Warning);
}

#[test]
fn fleet_status_counts_workspace_states() {
    let empty = WorkspaceState::new(PathBuf::from("F:/empty"));
    let mut healthy = WorkspaceState::new(PathBuf::from("F:/healthy"));
    healthy.snapshot.event_count = 1;
    let mut critical = WorkspaceState::new(PathBuf::from("F:/critical"));
    critical.snapshot.event_count = 1;
    critical.snapshot.severity = DashboardSeverity::Critical;

    let status = fleet_status(&[empty, healthy, critical]);

    assert_eq!(
        status,
        FleetStatus {
            total: 3,
            empty: 1,
            healthy: 1,
            warning: 0,
            critical: 1,
        }
    );
}

#[test]
fn fleet_status_label_prioritizes_critical_over_warning() {
    let (label, _) = fleet_status_label(FleetStatus {
        total: 2,
        empty: 0,
        healthy: 0,
        warning: 1,
        critical: 1,
    });

    assert_eq!(label, "Critical");
}

#[test]
fn fleet_summary_reports_no_workspaces_when_empty() {
    assert_eq!(
        fleet_summary_text(FleetStatus::default()),
        "no workspaces configured"
    );
}

#[test]
fn fleet_summary_omits_zero_categories_and_orders_by_severity() {
    let status = FleetStatus {
        total: 4,
        empty: 1,
        healthy: 1,
        warning: 0,
        critical: 2,
    };

    assert_eq!(
        fleet_summary_text(status),
        "4 workspaces · 2 critical, 1 healthy, 1 empty"
    );
}

#[test]
fn fleet_summary_uses_singular_workspace_noun() {
    let status = FleetStatus {
        total: 1,
        empty: 0,
        healthy: 1,
        warning: 0,
        critical: 0,
    };

    assert_eq!(fleet_summary_text(status), "1 workspace · 1 healthy");
}

#[test]
fn relative_age_uses_just_now_for_fresh_refresh() {
    assert_eq!(format_relative_age(Duration::from_millis(400)), "just now");
}

#[test]
fn relative_age_scales_units() {
    assert_eq!(format_relative_age(Duration::from_secs(5)), "5s ago");
    assert_eq!(format_relative_age(Duration::from_secs(150)), "2m ago");
    assert_eq!(format_relative_age(Duration::from_secs(7_400)), "2h ago");
}

#[test]
fn capture_summary_counts_kinds_and_severities() {
    let rows = [
        DashboardRow {
            number: 1,
            kind: DashboardRowKind::Event,
            severity: DashboardSeverity::Healthy,
            agent: Some("codex".into()),
            protocol: "ModelMessage".into(),
            summary: "ok".into(),
            detail: String::new(),
        },
        DashboardRow {
            number: 2,
            kind: DashboardRowKind::Intervention,
            severity: DashboardSeverity::Warning,
            agent: Some("codex".into()),
            protocol: "ServiceFailure".into(),
            summary: "retry".into(),
            detail: String::new(),
        },
        DashboardRow {
            number: 3,
            kind: DashboardRowKind::RepoHunkFile,
            severity: DashboardSeverity::Warning,
            agent: None,
            protocol: "repo-hunk-file".into(),
            summary: "src/lib.rs".into(),
            detail: String::new(),
        },
        DashboardRow {
            number: 4,
            kind: DashboardRowKind::Intervention,
            severity: DashboardSeverity::Critical,
            agent: Some("pi".into()),
            protocol: "AgentDegraded".into(),
            summary: "spawn".into(),
            detail: String::new(),
        },
    ];
    let refs = rows.iter().collect::<Vec<_>>();

    assert_eq!(
        capture_summary(&refs),
        CaptureSummary {
            total: 4,
            events: 2,
            interventions: 2,
            warning: 2,
            critical: 1,
        }
    );
}

#[test]
fn capture_summary_is_empty_for_no_rows() {
    assert_eq!(capture_summary(&[]), CaptureSummary::default());
    assert_eq!(
        capture_summary_text(CaptureSummary::default()),
        "0 rows · 0 events · 0 interventions · 0 warning · 0 critical"
    );
}

#[test]
fn dashboard_metric_items_show_replay_side_logs_and_locks_separately() {
    let mut snapshot = empty_snapshot();
    snapshot.event_count = 3;
    snapshot.intervention_count = 1;
    snapshot.design_count = 2;
    snapshot.trace_count = 4;
    snapshot.advice_count = 5;
    snapshot.packet_count = 6;
    snapshot.dispatch_count = 7;
    snapshot.outcome_count = 8;
    snapshot.lock_event_count = 9;

    let metrics = dashboard_metric_items(&snapshot);

    assert_eq!(
        metrics,
        vec![
            ("Events", 3),
            ("Interventions", 1),
            ("Design", 2),
            ("Trace", 4),
            ("Replay", 26),
            ("Locks", 9),
            ("Agents", 0),
        ]
    );
}

#[test]
fn truncate_summary_keeps_short_text_intact() {
    assert_eq!(truncate_summary("  hello  ", 20), "hello");
}

#[test]
fn truncate_summary_adds_ellipsis_when_too_long() {
    assert_eq!(truncate_summary("abcdefghij", 5), "abcd…");
}

#[test]
fn truncate_summary_counts_characters_not_bytes() {
    assert_eq!(truncate_summary("ßßßßß", 3), "ßß…");
}

#[test]
fn attention_items_include_workspace_errors() {
    let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
    workspace.last_error = Some("decode failed".into());

    let items = attention_items(&[workspace]);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].level, AttentionLevel::Critical);
    assert_eq!(items[0].message, "decode failed");
}

#[test]
fn attention_items_include_stale_and_degraded_agents() {
    let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
    workspace
        .snapshot
        .agent_sessions
        .push(coding_agent_monitor::AgentSession {
            agent: "codex".into(),
            status: AgentActivityStatus::Stale,
            score: 0,
            events: 4,
            interventions: 0,
            last_seen: Some("2026-06-22T12:00:00Z".into()),
        });
    workspace
        .snapshot
        .agent_sessions
        .push(coding_agent_monitor::AgentSession {
            agent: "claude-code".into(),
            status: AgentActivityStatus::Degraded,
            score: -3,
            events: 2,
            interventions: 1,
            last_seen: Some("2026-06-22T12:02:00Z".into()),
        });

    let items = attention_items(&[workspace]);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].level, AttentionLevel::Critical);
    assert!(items[0].message.contains("claude-code"));
    assert_eq!(items[1].level, AttentionLevel::Warning);
    assert!(items[1].message.contains("codex"));
}

#[test]
fn attention_items_include_critical_advisor_status() {
    let mut workspace = WorkspaceState::new(PathBuf::from("F:/repo"));
    workspace.snapshot.advisor_status = DashboardAdvisorStatus {
        enabled: true,
        credential_source: coding_agent_monitor::AdvisorCredentialSource::CodingPlan,
        credential_kind: DashboardAdvisorCredentialKind::JwtBearer,
        uses_dedicated_profile: true,
        endpoint: "https://api.openai.com/v1/chat/completions".into(),
        endpoint_host: Some("api.openai.com".into()),
        model: "gpt-5.5".into(),
        credential_file: Some("credentials/coding-plan/auth.json".into()),
        severity: DashboardSeverity::Critical,
        message: "JWT/OAuth-style coding-plan credential is incompatible with api.openai.com"
            .into(),
    };

    let items = attention_items(&[workspace]);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].level, AttentionLevel::Critical);
    assert!(items[0].message.contains("advisor"));
    assert!(items[0].message.contains("api.openai.com"));
}

#[test]
fn advisor_summary_names_source_without_exposing_token_material() {
    let status = DashboardAdvisorStatus {
        enabled: true,
        credential_source: coding_agent_monitor::AdvisorCredentialSource::CodingPlan,
        credential_kind: DashboardAdvisorCredentialKind::JwtBearer,
        uses_dedicated_profile: true,
        endpoint: "https://coding-plan.example.test/v1/chat/completions".into(),
        endpoint_host: Some("coding-plan.example.test".into()),
        model: "coding-plan-advisor".into(),
        credential_file: Some("credentials/coding-plan/auth.json".into()),
        severity: DashboardSeverity::Healthy,
        message: "dedicated coding-plan advisor endpoint configured".into(),
    };

    let text = advisor_summary_text(&status);

    assert!(text.contains("coding_plan"));
    assert!(text.contains("jwt_bearer"));
    assert!(text.contains("coding-plan.example.test"));
    assert!(!text.contains("eyJ"));
    assert!(!text.contains("OPENAI_API_KEY"));
}

#[test]
fn requirement_proof_trail_summarizes_bounded_history() {
    let row = DashboardRow {
        number: 7,
        kind: DashboardRowKind::Requirement,
        severity: DashboardSeverity::Warning,
        agent: None,
        protocol: "requirement".into(),
        summary: "covered proof 85: Advisor decisions must cite evidence.".into(),
        detail: r#"{
          "requirement": {
            "requirement_id": "req-advisor-proof",
            "text": "Advisor decisions must cite evidence."
          },
          "proofs": [
            {
              "case_file_id": "case-new",
              "built_at": "2026-06-24T09:00:00Z",
              "status": "covered",
              "latest_status": "passed",
              "latest_verification_evidence_id": "evt-new-verifier",
              "proof_strength": {
                "score": 85,
                "signals": ["trace_refs", "outcome_refs"],
                "gaps": []
              },
              "trace_refs": [{"file": "src/lib.rs", "necessity": "necessary"}],
              "repo_hunks": [{"history_id": "hunk-1"}],
              "control_refs": [{"advice_id": "adv-1"}],
              "outcome_refs": [{"outcome_id": "out-1"}]
            },
            {
              "case_file_id": "case-old",
              "built_at": "2026-06-23T09:00:00Z",
              "status": "stale",
              "latest_status": "stale",
              "proof_strength": {
                "score": 35,
                "signals": ["verification_ref"],
                "gaps": ["no_trace_refs", "no_outcome_refs"]
              },
              "trace_refs": [],
              "repo_hunks": [],
              "control_refs": [],
              "outcome_refs": []
            },
            {
              "case_file_id": "case-older",
              "built_at": "2026-06-22T09:00:00Z",
              "status": "unverified",
              "proof_strength": { "score": 10, "signals": [], "gaps": ["no_verifier"] }
            }
          ]
        }"#
        .into(),
    };

    let trail = requirement_proof_trail(&row, 2).expect("proof trail");

    assert_eq!(trail.requirement_id, "req-advisor-proof");
    assert_eq!(trail.text, "Advisor decisions must cite evidence.");
    assert_eq!(trail.steps.len(), 2);
    assert_eq!(trail.hidden_count, 1);
    assert!(trail.steps[0].summary.contains("case-new"));
    assert!(trail.steps[0].summary.contains("covered"));
    assert!(trail.steps[0].summary.contains("proof 85"));
    assert!(trail.steps[0].summary.contains("passed"));
    assert!(trail.steps[0].evidence_summary.contains("1 trace"));
    assert!(trail.steps[0].evidence_summary.contains("1 outcome"));
    assert!(trail.steps[1].gap_summary.contains("no_trace_refs"));
    assert!(trail.steps[1].gap_summary.contains("no_outcome_refs"));
}

#[test]
fn requirement_proof_trail_ignores_non_requirement_rows() {
    let row = DashboardRow {
        number: 1,
        kind: DashboardRowKind::Event,
        severity: DashboardSeverity::Healthy,
        agent: Some("codex".into()),
        protocol: "model".into(),
        summary: "not a requirement".into(),
        detail: r#"{"proofs":[{"case_file_id":"case"}]}"#.into(),
    };

    assert_eq!(requirement_proof_trail(&row, 4), None);
}

#[test]
fn requirement_proof_trail_ignores_invalid_requirement_detail_json() {
    let row = DashboardRow {
        number: 1,
        kind: DashboardRowKind::Requirement,
        severity: DashboardSeverity::Warning,
        agent: None,
        protocol: "requirement".into(),
        summary: "broken detail".into(),
        detail: "{not-json".into(),
    };

    assert_eq!(requirement_proof_trail(&row, 4), None);
}

#[test]
fn repo_hunk_file_detail_summarizes_counts() {
    let row = DashboardRow {
        number: 8,
        kind: DashboardRowKind::RepoHunkFile,
        severity: DashboardSeverity::Warning,
        agent: None,
        protocol: "repo-hunk-file".into(),
        summary: "src/lib.rs".into(),
        detail: r#"{
          "path": "src/lib.rs",
          "entry_count": 3,
          "traced_count": 1,
          "missing_rationale_count": 1,
          "untraced_count": 1,
          "matching_trace_count": 2,
          "worst_trace_status": "untraced",
          "latest_trace_status": "missing_rationale",
          "latest_history_id": "hist-new",
          "latest_observed_at": "2026-06-24T09:00:00Z"
        }"#
        .into(),
    };

    let detail = repo_hunk_file_detail(&row).expect("repo hunk file detail");

    assert_eq!(detail.path, "src/lib.rs");
    assert_eq!(detail.total, 3);
    assert_eq!(detail.traced, 1);
    assert_eq!(detail.missing_rationale, 1);
    assert_eq!(detail.untraced, 1);
    assert_eq!(detail.matching_traces, 2);
    assert_eq!(detail.worst_status, "untraced");
    assert_eq!(detail.latest_status, "missing_rationale");
    assert_eq!(detail.latest_history_id, "hist-new");
    assert_eq!(detail.latest_observed_at, "2026-06-24T09:00:00Z");
}

#[test]
fn repo_hunk_file_detail_ignores_non_file_rows() {
    let row = DashboardRow {
        number: 1,
        kind: DashboardRowKind::RepoHunk,
        severity: DashboardSeverity::Warning,
        agent: None,
        protocol: "repo-hunk".into(),
        summary: "raw hunk".into(),
        detail: r#"{"path":"src/lib.rs","entry_count":3}"#.into(),
    };

    assert_eq!(repo_hunk_file_detail(&row), None);
}

#[test]
fn dev_history_detail_summarizes_sources_and_finding() {
    let row = DashboardRow {
        number: 9,
        kind: DashboardRowKind::DevHistory,
        severity: DashboardSeverity::Critical,
        agent: None,
        protocol: "dev-history".into(),
        summary: "critical verification_entropy: stale verification risk".into(),
        detail: r#"{
          "generated_at": "2026-06-24T02:17:50Z",
          "workspace": "F:/rag_sys",
          "sources": [
            {
              "source": "codex",
              "files": 36,
              "sessions": 36,
              "lines": 21319,
              "bytes": 82563658,
              "history_root": "C:/Users/yys/.codex/sessions"
            },
            {
              "source": "claude-code",
              "files": 432,
              "sessions": 4,
              "lines": 77842,
              "bytes": 369301667,
              "history_root": "C:/Users/yys/.claude/projects/F--rag-sys"
            }
          ],
          "finding": {
            "kind": "verification_entropy",
            "severity": "critical",
            "summary": "History shows stale verification risk.",
            "evidence": ["14136 verification or unverified-stop signals"],
            "monitor_response": ["Force verification before continue."]
          }
        }"#
        .into(),
    };

    let detail = dev_history_detail(&row).expect("dev-history detail");

    assert_eq!(detail.kind, "verification_entropy");
    assert_eq!(detail.severity, "critical");
    assert_eq!(detail.generated_at, "2026-06-24T02:17:50Z");
    assert_eq!(detail.workspace, "F:/rag_sys");
    assert_eq!(
        detail.source_summary,
        "codex 36 file(s)/36 session(s); claude-code 432 file(s)/4 session(s)"
    );
    assert_eq!(
        detail.evidence_summary,
        "14136 verification or unverified-stop signals"
    );
    assert_eq!(
        detail.monitor_response_summary,
        "Force verification before continue."
    );
}

#[test]
fn dev_history_detail_ignores_non_dev_history_rows() {
    let row = DashboardRow {
        number: 1,
        kind: DashboardRowKind::Event,
        severity: DashboardSeverity::Healthy,
        agent: Some("codex".into()),
        protocol: "model".into(),
        summary: "not dev history".into(),
        detail: r#"{"finding":{"kind":"verification_entropy"}}"#.into(),
    };

    assert_eq!(dev_history_detail(&row), None);
}

#[test]
fn probe_run_detail_summarizes_probe_payload() {
    let row = DashboardRow {
        number: 10,
        kind: DashboardRowKind::ProbeRun,
        severity: DashboardSeverity::Healthy,
        agent: None,
        protocol: "probe".into(),
        summary: "local_evidence: succeeded".into(),
        detail: r#"{
          "probe_run_id": "probe-run-local",
          "advice_id": "advice-probe",
          "probe": {
            "kind": "local_evidence",
            "target": "routine_next_step"
          },
          "status": "succeeded",
          "summary": "local evidence probe observed recent events",
          "evidence_ids": ["evt-user", "repo-audit-src-lib-rs"]
        }"#
        .into(),
    };

    let detail = probe_run_detail(&row).expect("probe detail");

    assert_eq!(detail.probe_run_id, "probe-run-local");
    assert_eq!(detail.advice_id, "advice-probe");
    assert_eq!(detail.probe_kind, "local_evidence");
    assert_eq!(detail.target, "routine_next_step");
    assert_eq!(detail.status, "succeeded");
    assert_eq!(detail.evidence_count, 2);
    assert!(detail.summary.contains("recent events"));
}

#[test]
fn decision_trail_detail_summarizes_control_chain() {
    let row = DashboardRow {
        number: 11,
        kind: DashboardRowKind::DecisionTrail,
        severity: DashboardSeverity::Warning,
        agent: Some("codex".into()),
        protocol: "decision-trail".into(),
        summary: "force_verification -> codex".into(),
        detail: r#"{
          "advice": {
            "advice_id": "advice-force",
            "final_action": {
              "type": "force_verification",
              "suite": "targeted",
              "blocking": true
            },
            "control_rationale": {
              "selected_action": "force_verification",
              "reason": "verification is stale"
            }
          },
          "packet": {
            "packet_id": "packet-force",
            "target_agent": "codex",
            "urgency": "blocking"
          },
          "dispatch_result": {
            "packet_id": "packet-force",
            "target_agent": "codex",
            "status": "outbox_written"
          },
          "outcomes": [
            { "outcome_id": "out-pass", "status": "succeeded" },
            { "outcome_id": "out-fail", "status": "failed" }
          ]
        }"#
        .into(),
    };

    let detail = decision_trail_detail(&row).expect("decision detail");

    assert_eq!(detail.advice_id, "advice-force");
    assert_eq!(detail.action, "force_verification");
    assert_eq!(detail.target_agent, "codex");
    assert_eq!(detail.packet_id, "packet-force");
    assert_eq!(detail.urgency, "blocking");
    assert_eq!(detail.dispatch_status, "outbox_written");
    assert_eq!(detail.outcome_count, 2);
    assert_eq!(detail.failed_outcome_count, 1);
    assert_eq!(detail.rationale, "verification is stale");
}

#[test]
fn tray_toggle_id_maps_to_toggle_command() {
    assert_eq!(
        tray_command_from_id(&MenuId::new(TRAY_TOGGLE_ID)),
        Some(TrayCommand::Toggle)
    );
}

#[test]
fn running_agent_summary_includes_process_and_workspace() {
    let agent = RunningAgent::new(42, coding_agent_monitor::AgentKind::Codex, "codex.exe")
        .with_cwd(Some(PathBuf::from("F:/repo")));

    assert_eq!(
        running_agent_summary(&agent),
        "pid 42 · codex.exe · F:/repo"
    );
}

#[test]
fn running_agent_summary_reports_missing_workspace() {
    let agent = RunningAgent::new(42, coding_agent_monitor::AgentKind::ClaudeCode, "node.exe");

    assert_eq!(
        running_agent_summary(&agent),
        "pid 42 · node.exe · cwd unavailable"
    );
}

#[test]
fn review_summary_reports_intervention_count() {
    let report = coding_agent_monitor::AgentReviewReport {
        workspace: "F:/repo".into(),
        status: coding_agent_monitor::AgentReviewStatus::Intervene,
        findings: vec![coding_agent_monitor::AgentReviewFinding {
            severity: DashboardSeverity::Critical,
            category: "unverified_completion".into(),
            agent: Some("codex".into()),
            evidence: "done without tests".into(),
            recommended_action: coding_agent_monitor::AgentReviewAction::ForceVerification,
        }],
    };

    assert_eq!(review_summary_text(&report), "Intervene · 1 finding");
}
