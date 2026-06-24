use coding_agent_monitor::{
    Action, Config, Event, EventKind, Intervention, InterventionKind, ProjectStore, run_jsonl,
    run_jsonl_with_store,
};

#[test]
fn run_jsonl_emits_interventions_for_input_events() {
    let input = br#"{"agent":"codex","session":"s1","kind":"model_message","content":"This is a good point to stop. Ask user about remaining jobs."}
"#;
    let mut output = Vec::new();

    run_jsonl(
        &input[..],
        &mut output,
        Config {
            open_work: true,
            retry_limit: 1,
            fallback_agents: vec!["claude-code".into()],
        },
    )
    .expect("jsonl run should succeed");

    let got: Intervention =
        serde_json::from_slice(output.as_slice()).expect("output should be one JSON object");
    assert_eq!(got.kind, InterventionKind::PrematureStop);
    assert_eq!(got.action, Action::ContinueWorking);
}

#[test]
fn run_jsonl_reports_malformed_input_line() {
    let mut output = Vec::new();
    let err = run_jsonl(b"{bad json}\n".as_slice(), &mut output, Config::default())
        .expect_err("malformed input should fail");

    assert!(
        err.to_string().contains("line 1"),
        "expected line number in error, got {err}"
    );
}

#[test]
fn run_jsonl_with_store_skips_disabled_fallback_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "adapters": {
            "claude_code": {
              "enabled": false
            }
          }
        }"#,
    )
    .expect("config");
    let input = serde_json::to_vec(&Event {
        agent: "codex".into(),
        kind: EventKind::ModelMessage,
        content: Some("upstream service unavailable".into()),
        ..Event::default()
    })
    .expect("event json");
    let mut input = input;
    input.push(b'\n');
    let mut output = Vec::new();

    run_jsonl_with_store(
        input.as_slice(),
        &mut output,
        Config {
            open_work: true,
            retry_limit: 0,
            fallback_agents: vec!["claude-code".into(), "opencode".into()],
        },
        &mut store,
    )
    .expect("jsonl run should succeed");

    let intervention: Intervention =
        serde_json::from_slice(output.as_slice()).expect("one intervention jsonl record");
    assert_eq!(intervention.kind, InterventionKind::ServiceFailure);
    assert_eq!(intervention.action, Action::SwitchAgent);
    assert_eq!(intervention.agent.as_deref(), Some("opencode"));
}

#[test]
fn run_jsonl_with_store_skips_unknown_fallback_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = serde_json::to_vec(&Event {
        agent: "codex".into(),
        kind: EventKind::ModelMessage,
        content: Some("upstream service unavailable".into()),
        ..Event::default()
    })
    .expect("event json");
    let mut input = input;
    input.push(b'\n');
    let mut output = Vec::new();

    run_jsonl_with_store(
        input.as_slice(),
        &mut output,
        Config {
            open_work: true,
            retry_limit: 0,
            fallback_agents: vec!["mystery-agent".into(), "opencode".into()],
        },
        &mut store,
    )
    .expect("jsonl run should succeed");

    let intervention: Intervention =
        serde_json::from_slice(output.as_slice()).expect("one intervention jsonl record");
    assert_eq!(intervention.kind, InterventionKind::ServiceFailure);
    assert_eq!(intervention.action, Action::SwitchAgent);
    assert_eq!(intervention.agent.as_deref(), Some("opencode"));
}
