//! Tests for advisor credential resolution and the schema-bounded system
//! prompt, included into `advisor_client.rs` via `#[path]` so they can reach
//! the module's private helpers.

use super::*;

#[test]
fn advisor_credential_path_rejects_local_cli_auth_directory() {
    let provider = AdvisorProviderConfig {
        credential_source: AdvisorCredentialSource::CodingPlan,
        credential_file: Some(".codex/auth.json".into()),
        ..Default::default()
    };

    let error = advisor_credential_path(&provider, Path::new("F:/repo/.agent-monitor"))
        .expect_err("local CLI auth paths must be rejected at runtime");

    assert!(matches!(
        error,
        AdvisorClientError::LocalCliAuthCredentialProfile {
            cli_dir: ".codex",
            ..
        }
    ));
}

#[test]
fn advisor_bearer_token_rejects_legacy_claude_plan_source() {
    let provider = AdvisorProviderConfig {
        credential_source: AdvisorCredentialSource::ClaudePlan,
        credential_file: Some("credentials/advisor.json".into()),
        ..Default::default()
    };

    let error = resolve_advisor_bearer_token(&provider, Path::new("F:/repo/.agent-monitor"))
        .expect_err("legacy claude_plan source must fail closed");

    assert!(matches!(
        error,
        AdvisorClientError::UnsupportedCredentialSource {
            kind: AdvisorCredentialSource::ClaudePlan
        }
    ));
}

#[test]
fn advisor_system_prompt_is_schema_bounded_and_action_dense() {
    let prompt = advisor_system_prompt();

    assert!(prompt.contains("JSON only"));
    assert!(prompt.contains("cheapest allowed action"));
    assert!(prompt.contains("visible evidence ids"));
    assert!(prompt.contains("Supported run_probe specs"));
    assert!(prompt.contains("runtime_validation"));
    assert!(prompt.contains("Runtime validation gaps"));
    assert!(prompt.contains("not browser-only probes"));
    assert!(prompt.contains("packet_draft must be short"));
    assert!(prompt.contains("diagnose uncertainty, not agent quality"));
    assert!(prompt.contains("Do not summarize the case file"));
    assert!(prompt.contains("Do not include secrets or chain-of-thought"));
    assert!(
        !prompt.contains("helpful"),
        "advisor prompt should avoid generic assistant posture: {prompt}"
    );
}
