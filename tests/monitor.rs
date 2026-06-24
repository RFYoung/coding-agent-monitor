use coding_agent_monitor::{Action, Config, Event, EventKind, InterventionKind, Monitor};

#[test]
fn flags_premature_stop_when_work_is_still_open() {
    let mut monitor = Monitor::new(Config {
        open_work: true,
        retry_limit: 2,
        fallback_agents: vec!["claude-code".into(), "opencode".into()],
    });

    let interventions = monitor.ingest(Event {
        agent: "codex".into(),
        session: Some("s1".into()),
        kind: EventKind::ModelMessage,
        content: Some(
            "This is a good point to stop. Ask the user if they want me to do the remaining jobs."
                .into(),
        ),
        ..Event::default()
    });

    assert_eq!(interventions.len(), 1);
    assert_eq!(interventions[0].kind, InterventionKind::PrematureStop);
    assert_eq!(interventions[0].action, Action::ContinueWorking);
    assert!(
        interventions[0].reason.contains("remaining work is open"),
        "unexpected reason: {}",
        interventions[0].reason
    );
}

#[test]
fn retries_transient_failures_then_switches_agent() {
    let mut monitor = Monitor::new(Config {
        open_work: true,
        retry_limit: 2,
        fallback_agents: vec!["claude-code".into(), "opencode".into()],
    });

    let first = monitor.ingest(model_message(
        "codex",
        "connection reset while streaming response",
    ));
    let second = monitor.ingest(model_message("codex", "upstream service unavailable"));
    let third = monitor.ingest(model_message("codex", "context length exceeded"));

    assert_eq!(first[0].action, Action::RetrySameAgent);
    assert_eq!(first[0].agent.as_deref(), Some("codex"));
    assert_eq!(second[0].action, Action::RetrySameAgent);
    assert_eq!(second[0].agent.as_deref(), Some("codex"));
    assert_eq!(third[0].action, Action::SwitchAgent);
    assert_eq!(third[0].agent.as_deref(), Some("claude-code"));
}

#[test]
fn retry_limit_exceeded_without_fallback_keeps_same_agent() {
    let mut monitor = Monitor::new(Config {
        open_work: true,
        retry_limit: 0,
        fallback_agents: Vec::new(),
    });

    let interventions = monitor.ingest(model_message("codex", "upstream service unavailable"));

    assert_eq!(interventions.len(), 1);
    assert_eq!(interventions[0].kind, InterventionKind::ServiceFailure);
    assert_eq!(interventions[0].action, Action::RetrySameAgent);
    assert_eq!(interventions[0].agent.as_deref(), Some("codex"));
    assert!(interventions[0].reason.contains("no fallback"));
}

#[test]
fn healthy_message_resets_transient_failure_retry_count() {
    let mut monitor = Monitor::new(Config {
        open_work: true,
        retry_limit: 2,
        fallback_agents: vec!["claude-code".into(), "opencode".into()],
    });

    let first = monitor.ingest(model_message(
        "codex",
        "connection reset while streaming response",
    ));
    let second = monitor.ingest(model_message("codex", "upstream service unavailable"));
    let recovered = monitor.ingest(model_message("codex", "Recovered and continuing normally."));
    let after_recovery = monitor.ingest(model_message("codex", "context length exceeded"));

    assert_eq!(first[0].action, Action::RetrySameAgent);
    assert_eq!(second[0].action, Action::RetrySameAgent);
    assert!(recovered.is_empty());
    assert_eq!(after_recovery[0].action, Action::RetrySameAgent);
    assert_eq!(after_recovery[0].agent.as_deref(), Some("codex"));
}

#[test]
fn successful_command_result_resets_transient_failure_retry_count() {
    let mut monitor = Monitor::new(Config {
        open_work: true,
        retry_limit: 2,
        fallback_agents: vec!["claude-code".into(), "opencode".into()],
    });

    let first = monitor.ingest(model_message(
        "codex",
        "connection reset while streaming response",
    ));
    let second = monitor.ingest(model_message("codex", "upstream service unavailable"));
    let recovered = monitor.ingest(Event {
        agent: "codex".into(),
        kind: EventKind::CommandResult,
        command: Some("codex exec".into()),
        exit_code: Some(0),
        ..Event::default()
    });
    let after_recovery = monitor.ingest(model_message("codex", "context length exceeded"));

    assert_eq!(first[0].action, Action::RetrySameAgent);
    assert_eq!(second[0].action, Action::RetrySameAgent);
    assert!(recovered.is_empty());
    assert_eq!(after_recovery[0].action, Action::RetrySameAgent);
    assert_eq!(after_recovery[0].agent.as_deref(), Some("codex"));
}

#[test]
fn records_design_thoughts_and_file_change_trace() {
    let mut monitor = Monitor::new(Config::default());

    monitor.ingest(Event {
        time: Some("2026-06-22T12:03:00Z".into()),
        agent: "codex".into(),
        session: Some("s1".into()),
        kind: EventKind::DesignThought,
        content: Some("The monitor should judge agent robustness from outside the agent.".into()),
        ..Event::default()
    });
    monitor.ingest(Event {
        time: Some("2026-06-22T12:04:00Z".into()),
        agent: "codex".into(),
        session: Some("s1".into()),
        kind: EventKind::FileChange,
        file: Some("src/lib.rs".into()),
        line: Some(42),
        rationale: Some("Add premature-stop detection.".into()),
        ..Event::default()
    });

    let design = monitor.design_record();
    assert_eq!(design.len(), 1);
    assert_eq!(
        design[0].content,
        "The monitor should judge agent robustness from outside the agent."
    );

    let trace = monitor.trace();
    assert_eq!(trace.len(), 1);
    assert_eq!(trace[0].file, "src/lib.rs");
    assert_eq!(trace[0].line, Some(42));
    assert_eq!(
        trace[0].rationale,
        Some("Add premature-stop detection.".into())
    );
}

#[test]
fn forgetting_design_memory_marks_agent_degraded_and_requests_fresh_agent() {
    let mut monitor = Monitor::new(Config::default());

    let interventions = monitor.ingest(model_message(
        "codex",
        "I do not remember the design constraints or what the user wanted.",
    ));

    assert_eq!(interventions.len(), 1);
    assert_eq!(interventions[0].kind, InterventionKind::AgentDegraded);
    assert_eq!(interventions[0].action, Action::SpawnFreshAgent);
    assert_eq!(interventions[0].agent.as_deref(), Some("codex"));
    assert!(
        interventions[0].reason.contains("design memory"),
        "unexpected reason: {}",
        interventions[0].reason
    );
    assert_eq!(monitor.robustness_score("codex"), -3);
}

#[test]
fn premature_stop_reduces_robustness_score() {
    let mut monitor = Monitor::new(Config::default());

    monitor.ingest(model_message(
        "codex",
        "This is a good point to stop. Ask user about remaining jobs.",
    ));

    assert_eq!(monitor.robustness_score("codex"), -2);
}

fn model_message(agent: &str, content: &str) -> Event {
    Event {
        agent: agent.into(),
        kind: EventKind::ModelMessage,
        content: Some(content.into()),
        ..Event::default()
    }
}
