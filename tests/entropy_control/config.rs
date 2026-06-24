use super::*;

#[test]
fn project_config_loads_endpoint_provider_without_secret_value() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "advisor": {
            "enabled": true,
            "provider": {
              "kind": "openai_compatible",
              "endpoint": "https://api.example.test/v1/chat/completions",
              "model": "gpt-5",
              "api_key_env": "CAM_TEST_KEY",
              "timeout_secs": 10,
              "max_input_tokens": 4000,
              "max_output_tokens": 1000
            }
          }
        }"#,
    )
    .expect("config");

    let config = ProjectConfig::load(store.root()).expect("load config");

    assert!(config.advisor.enabled);
    assert_eq!(
        config.advisor.provider.kind,
        AdvisorProviderKind::OpenAiCompatible
    );
    assert_eq!(config.advisor.provider.api_key_env, "CAM_TEST_KEY");
}

#[test]
fn project_config_rejects_codex_plan_credential_source_alias() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "advisor": {
            "enabled": true,
            "provider": {
              "kind": "openai_compatible",
              "endpoint": "https://api.openai.com/v1/chat/completions",
              "model": "gpt-5.5",
              "credential_source": "codex_plan",
              "credential_file": "credentials/coding-plan/auth.json"
            }
          }
        }"#,
    )
    .expect("config");

    let error = ProjectConfig::load(store.root())
        .expect_err("codex_plan must not be accepted as coding-plan credentials");

    assert!(error.to_string().contains("codex_plan"));
}

#[test]
fn advisor_endpoint_config_writer_persists_env_name_without_secret_value() {
    let temp = tempfile::tempdir().expect("temp dir");
    let updated = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "https://api.example.test/v1/chat/completions".into(),
            model: "gpt-5.5".into(),
            api_key_env: "CAM_PROD_KEY".into(),
            credential_source: None,
            credential_file: None,
            enabled: true,
        },
    )
    .expect("write advisor config");

    assert!(updated.advisor.enabled);
    assert_eq!(
        updated.advisor.provider.endpoint,
        "https://api.example.test/v1/chat/completions"
    );
    assert_eq!(updated.advisor.provider.model, "gpt-5.5");
    assert_eq!(updated.advisor.provider.api_key_env, "CAM_PROD_KEY");

    let config_text =
        std::fs::read_to_string(temp.path().join(".agent-monitor").join("config.json"))
            .expect("config text");
    assert!(config_text.contains("\"api_key_env\": \"CAM_PROD_KEY\""));
    assert!(!config_text.contains("sk-"));
    assert!(!config_text.contains("CAM_PROD_KEY="));

    let reloaded = ProjectConfig::load(temp.path().join(".agent-monitor")).expect("reload config");
    assert_eq!(reloaded.advisor.provider.api_key_env, "CAM_PROD_KEY");
}

#[test]
fn advisor_endpoint_config_writer_preserves_existing_policy_and_verifiers() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "switch_agent_cooldown_min": 45
          },
          "verifiers": [
            {
              "id": "smoke",
              "command": "cargo test smoke",
              "scope": "targeted",
              "timeout_secs": 30,
              "paths": ["src/lib.rs"]
            }
          ]
        }"#,
    )
    .expect("seed config");

    let updated = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "http://127.0.0.1:8080/v1/chat/completions".into(),
            model: "local-advisor".into(),
            api_key_env: "LOCAL_ADVISOR_KEY".into(),
            credential_source: None,
            credential_file: None,
            enabled: true,
        },
    )
    .expect("write advisor config");

    assert_eq!(updated.policy.switch_agent_cooldown_min, 45);
    assert_eq!(updated.verifiers.len(), 1);
    assert_eq!(updated.verifiers[0].id, "smoke");
    assert_eq!(
        updated.advisor.provider.endpoint,
        "http://127.0.0.1:8080/v1/chat/completions"
    );
}

#[test]
fn verifier_config_writer_upserts_verifier_without_touching_advisor_or_policy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "switch_agent_cooldown_min": 45
          },
          "advisor": {
            "enabled": true,
            "provider": {
              "endpoint": "http://127.0.0.1:8080/v1/chat/completions",
              "model": "local-advisor",
              "api_key_env": "LOCAL_ADVISOR_KEY"
            }
          },
          "verifiers": [
            {
              "id": "smoke",
              "command": "cargo test old",
              "scope": "targeted",
              "timeout_secs": 30,
              "paths": ["src/old.rs"]
            }
          ]
        }"#,
    )
    .expect("seed config");

    let updated = coding_agent_monitor::write_verifier_config(
        temp.path(),
        coding_agent_monitor::VerifierConfig {
            id: "smoke".into(),
            command: "cargo test --quiet".into(),
            scope: VerificationScope::Full,
            timeout_secs: 900,
            paths: vec!["src/lib.rs".into(), "tests/entropy_control.rs".into()],
            acceptance_patterns: vec!["runtime_validation:native_gui".into()],
        },
    )
    .expect("write verifier config");

    assert_eq!(updated.policy.switch_agent_cooldown_min, 45);
    assert!(updated.advisor.enabled);
    assert_eq!(updated.advisor.provider.model, "local-advisor");
    assert_eq!(updated.verifiers.len(), 1);
    let verifier = &updated.verifiers[0];
    assert_eq!(verifier.id, "smoke");
    assert_eq!(verifier.command, "cargo test --quiet");
    assert_eq!(verifier.scope, VerificationScope::Full);
    assert_eq!(verifier.timeout_secs, 900);
    assert_eq!(
        verifier.paths,
        vec![
            "src/lib.rs".to_string(),
            "tests/entropy_control.rs".to_string()
        ]
    );
    assert_eq!(
        verifier.acceptance_patterns,
        vec!["runtime_validation:native_gui".to_string()]
    );
}

#[test]
fn verifier_config_writer_rejects_empty_id_or_command() {
    let temp = tempfile::tempdir().expect("temp dir");

    let empty_id = coding_agent_monitor::write_verifier_config(
        temp.path(),
        coding_agent_monitor::VerifierConfig {
            id: " ".into(),
            command: "cargo test".into(),
            scope: VerificationScope::Full,
            timeout_secs: 300,
            paths: vec![],
            acceptance_patterns: vec![],
        },
    )
    .expect_err("empty verifier id should fail");
    assert!(empty_id.to_string().contains("verifier id"));

    let empty_command = coding_agent_monitor::write_verifier_config(
        temp.path(),
        coding_agent_monitor::VerifierConfig {
            id: "smoke".into(),
            command: " ".into(),
            scope: VerificationScope::Full,
            timeout_secs: 300,
            paths: vec![],
            acceptance_patterns: vec![],
        },
    )
    .expect_err("empty verifier command should fail");
    assert!(empty_command.to_string().contains("verifier command"));
}

#[test]
fn verifier_config_writer_tolerates_existing_invalid_advisor_profile() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::create_dir_all(store.root().join("credentials").join("coding-plan"))
        .expect("credential dir");
    std::fs::write(
        store
            .root()
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
        r#"{"OPENAI_API_KEY":"eyJheader.payload.signature"}"#,
    )
    .expect("credential file");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "advisor": {
            "enabled": true,
            "provider": {
              "endpoint": "https://api.openai.com/v1/chat/completions",
              "model": "gpt-5.5",
              "credential_source": "coding_plan",
              "credential_file": "credentials/coding-plan/auth.json"
            }
          }
        }"#,
    )
    .expect("seed config");

    let updated = coding_agent_monitor::write_verifier_config(
        temp.path(),
        coding_agent_monitor::VerifierConfig {
            id: "self_test".into(),
            command: "cargo test --quiet".into(),
            scope: VerificationScope::Full,
            timeout_secs: 900,
            paths: vec!["src".into(), "tests".into()],
            acceptance_patterns: vec![],
        },
    )
    .expect("verifier registration should not validate unrelated advisor credentials");

    assert_eq!(updated.verifiers.len(), 1);
    assert_eq!(updated.verifiers[0].id, "self_test");
    assert!(updated.advisor.enabled);
    assert_eq!(
        updated.advisor.provider.endpoint,
        "https://api.openai.com/v1/chat/completions"
    );
}

#[test]
fn advisor_endpoint_config_writer_can_select_coding_plan_credentials() {
    let temp = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(
        temp.path()
            .join(".agent-monitor")
            .join("credentials")
            .join("coding-plan"),
    )
    .expect("credential dir");
    std::fs::write(
        temp.path()
            .join(".agent-monitor")
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
        r#"{"OPENAI_API_KEY":"dedicated-plan-key"}"#,
    )
    .expect("credential file");
    let updated = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            model: "gpt-5.5".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            credential_source: Some(coding_agent_monitor::AdvisorCredentialSource::CodingPlan),
            credential_file: Some("credentials/coding-plan/auth.json".into()),
            enabled: true,
        },
    )
    .expect("write advisor config");

    assert_eq!(
        updated.advisor.provider.credential_source,
        coding_agent_monitor::AdvisorCredentialSource::CodingPlan
    );
    assert_eq!(
        updated.advisor.provider.credential_file.as_deref(),
        Some("credentials/coding-plan/auth.json")
    );

    let config_text =
        std::fs::read_to_string(temp.path().join(".agent-monitor").join("config.json"))
            .expect("config text");
    assert!(config_text.contains("\"credential_source\": \"coding_plan\""));
    assert!(config_text.contains("\"credential_file\": \"credentials/coding-plan/auth.json\""));
    assert!(!config_text.contains("access_token"));
    assert!(!config_text.contains("refresh_token"));
    assert!(!config_text.contains("dedicated-plan-key"));
}

#[test]
fn advisor_endpoint_config_writer_rejects_claude_plan_credential_source() {
    let temp = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(temp.path().join(".agent-monitor").join("credentials"))
        .expect("credential dir");
    std::fs::write(
        temp.path()
            .join(".agent-monitor")
            .join("credentials")
            .join("advisor.json"),
        r#"{"api_key":"dedicated-provider-key"}"#,
    )
    .expect("credential file");

    let error = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "https://api.anthropic.com/v1/messages".into(),
            model: "claude-opus-4".into(),
            api_key_env: "ANTHROPIC_API_KEY".into(),
            credential_source: Some(coding_agent_monitor::AdvisorCredentialSource::ClaudePlan),
            credential_file: Some("credentials/advisor.json".into()),
            enabled: true,
        },
    )
    .expect_err("advisor credential_source must use coding_plan for dedicated profiles");

    let message = error.to_string();
    assert!(message.contains("coding_plan"));
    assert!(message.contains("claude_plan"));
    assert!(
        !temp
            .path()
            .join(".agent-monitor")
            .join("config.json")
            .exists()
    );
}

#[test]
fn advisor_endpoint_config_writer_rejects_coding_plan_source_without_credential_file() {
    let temp = tempfile::tempdir().expect("temp dir");

    let error = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            model: "gpt-5.5".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            credential_source: Some(coding_agent_monitor::AdvisorCredentialSource::CodingPlan),
            credential_file: None,
            enabled: true,
        },
    )
    .expect_err("coding-plan credential source requires a credential file");

    let message = error.to_string();
    assert!(message.contains("credential_file"));
    assert!(message.contains("coding_plan"));
    assert!(!temp.path().join(".agent-monitor").exists());
}

#[test]
fn advisor_endpoint_config_writer_rejects_missing_coding_plan_credential_file() {
    let temp = tempfile::tempdir().expect("temp dir");

    let error = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            model: "gpt-5.5".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            credential_source: Some(coding_agent_monitor::AdvisorCredentialSource::CodingPlan),
            credential_file: Some("credentials/coding-plan/missing.json".into()),
            enabled: true,
        },
    )
    .expect_err("missing dedicated coding-plan credentials should be rejected");

    let message = error.to_string();
    assert!(message.contains("credentials/coding-plan/missing.json"));
    assert!(
        !temp
            .path()
            .join(".agent-monitor")
            .join("config.json")
            .exists()
    );
}

#[test]
fn advisor_endpoint_config_writer_rejects_coding_plan_profile_without_supported_token() {
    let temp = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(
        temp.path()
            .join(".agent-monitor")
            .join("credentials")
            .join("coding-plan"),
    )
    .expect("credential dir");
    std::fs::write(
        temp.path()
            .join(".agent-monitor")
            .join("credentials")
            .join("coding-plan")
            .join("profile.json"),
        r#"{"refresh_token":"not-usable-by-advisor"}"#,
    )
    .expect("credential profile");

    let error = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            model: "gpt-5.5".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            credential_source: Some(coding_agent_monitor::AdvisorCredentialSource::CodingPlan),
            credential_file: Some("credentials/coding-plan/profile.json".into()),
            enabled: true,
        },
    )
    .expect_err("coding-plan credential profile needs a supported token field");

    let message = error.to_string();
    assert!(message.contains("coding_plan"));
    assert!(message.contains("supported advisor token"));
    assert!(
        !temp
            .path()
            .join(".agent-monitor")
            .join("config.json")
            .exists()
    );
}

#[test]
fn advisor_endpoint_config_writer_rejects_local_cli_auth_as_coding_plan_profile() {
    let workspace = tempfile::tempdir().expect("workspace");
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".codex")).expect("codex dir");
    let codex_auth = home.path().join(".codex").join("auth.json");
    std::fs::write(
        &codex_auth,
        r#"{"tokens":{"access_token":"cli-auth-token"}}"#,
    )
    .expect("codex auth");

    let error = coding_agent_monitor::write_advisor_endpoint_config(
        workspace.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            model: "gpt-5.5".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            credential_source: Some(coding_agent_monitor::AdvisorCredentialSource::CodingPlan),
            credential_file: Some(codex_auth.display().to_string()),
            enabled: true,
        },
    )
    .expect_err("local Codex CLI auth must not be accepted as coding-plan credentials");

    let message = error.to_string();
    assert!(message.contains("local CLI auth"));
    assert!(message.contains(".codex"));
    assert!(
        !workspace
            .path()
            .join(".agent-monitor")
            .join("config.json")
            .exists()
    );
}

#[test]
fn advisor_endpoint_config_writer_rejects_jwt_coding_plan_profile_with_public_openai_endpoint() {
    let temp = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(
        temp.path()
            .join(".agent-monitor")
            .join("credentials")
            .join("coding-plan"),
    )
    .expect("credential dir");
    std::fs::write(
        temp.path()
            .join(".agent-monitor")
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
        r#"{"OPENAI_API_KEY":"eyJheader.payload.signature"}"#,
    )
    .expect("credential profile");

    let error = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            model: "gpt-5.5".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            credential_source: Some(coding_agent_monitor::AdvisorCredentialSource::CodingPlan),
            credential_file: Some("credentials/coding-plan/auth.json".into()),
            enabled: true,
        },
    )
    .expect_err("JWT-style coding-plan credentials need a dedicated provider endpoint");

    let message = error.to_string();
    assert!(message.contains("coding-plan"));
    assert!(message.contains("api.openai.com"));
    assert!(
        !temp
            .path()
            .join(".agent-monitor")
            .join("config.json")
            .exists()
    );
}

#[test]
fn advisor_endpoint_config_writer_preserves_existing_credential_source_when_unspecified() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::create_dir_all(store.root().join("credentials").join("coding-plan"))
        .expect("credential dir");
    std::fs::write(
        store
            .root()
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
        r#"{"OPENAI_API_KEY":"dedicated-token"}"#,
    )
    .expect("credential profile");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "advisor": {
            "enabled": true,
            "provider": {
              "endpoint": "https://api.openai.com/v1/chat/completions",
              "model": "gpt-5.5",
              "credential_source": "coding_plan",
              "credential_file": "credentials/coding-plan/auth.json"
            }
          }
        }"#,
    )
    .expect("seed config");

    let updated = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "http://127.0.0.1:8080/v1/chat/completions".into(),
            model: "local-advisor".into(),
            api_key_env: "LOCAL_ADVISOR_KEY".into(),
            credential_source: None,
            credential_file: None,
            enabled: true,
        },
    )
    .expect("write advisor config");

    assert_eq!(
        updated.advisor.provider.credential_source,
        coding_agent_monitor::AdvisorCredentialSource::CodingPlan
    );
    assert_eq!(
        updated.advisor.provider.credential_file.as_deref(),
        Some("credentials/coding-plan/auth.json")
    );
}

#[test]
fn advisor_endpoint_config_writer_rejects_preserved_cli_auth_reference() {
    let temp = tempfile::tempdir().expect("temp dir");
    let home = tempfile::tempdir().expect("home");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::create_dir_all(home.path().join(".codex")).expect("codex dir");
    let codex_auth = home.path().join(".codex").join("auth.json");
    std::fs::write(&codex_auth, r#"{"OPENAI_API_KEY":"codex-cli-token"}"#).expect("codex auth");
    std::fs::write(
        store.root().join("config.json"),
        format!(
            r#"{{
          "advisor": {{
            "enabled": true,
            "provider": {{
              "endpoint": "https://api.openai.com/v1/chat/completions",
              "model": "gpt-5.5",
              "credential_source": "coding_plan",
              "credential_file": "{}"
            }}
          }}
        }}"#,
            codex_auth.display().to_string().replace('\\', "\\\\")
        ),
    )
    .expect("seed config");

    let error = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "http://127.0.0.1:8080/v1/chat/completions".into(),
            model: "local-advisor".into(),
            api_key_env: "LOCAL_ADVISOR_KEY".into(),
            credential_source: None,
            credential_file: None,
            enabled: true,
        },
    )
    .expect_err("preserved local CLI auth reference must be rejected");

    let message = error.to_string();
    assert!(message.contains("local CLI auth"));
    assert!(message.contains(".codex"));
}

#[test]
fn advisor_endpoint_config_writer_rejects_preserved_claude_plan_source() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "advisor": {
            "enabled": true,
            "provider": {
              "endpoint": "https://api.openai.com/v1/chat/completions",
              "model": "gpt-5.5",
              "credential_source": "claude_plan",
              "credential_file": "credentials/claude-plan/auth.json"
            }
          }
        }"#,
    )
    .expect("seed config");

    let error = coding_agent_monitor::write_advisor_endpoint_config(
        temp.path(),
        coding_agent_monitor::AdvisorEndpointConfigUpdate {
            endpoint: "http://127.0.0.1:8080/v1/chat/completions".into(),
            model: "local-advisor".into(),
            api_key_env: "LOCAL_ADVISOR_KEY".into(),
            credential_source: None,
            credential_file: None,
            enabled: true,
        },
    )
    .expect_err("preserved claude_plan source must be rejected");

    let message = error.to_string();
    assert!(message.contains("claude_plan"));
    assert!(message.contains("coding_plan"));
}

#[test]
fn coding_plan_credential_import_materializes_project_advisor_profile_without_runtime_auth() {
    let workspace = tempfile::tempdir().expect("workspace");
    let source_dir = tempfile::tempdir().expect("source");
    let source_file = source_dir.path().join("auth.json");
    std::fs::write(
        &source_file,
        r#"{
          "OPENAI_API_KEY": "dedicated-plan-key",
          "tokens": {
            "access_token": "plan-access-token",
            "refresh_token": "refresh-must-not-copy",
            "id_token": "id-must-not-copy"
          }
        }"#,
    )
    .expect("source credential");

    let config = coding_agent_monitor::import_coding_plan_advisor_credentials(
        workspace.path(),
        &source_file,
        Some("https://api.openai.com/v1/chat/completions"),
        Some("gpt-5.5"),
    )
    .expect("import coding-plan credentials");

    assert!(config.advisor.enabled);
    assert_eq!(
        config.advisor.provider.credential_source,
        coding_agent_monitor::AdvisorCredentialSource::CodingPlan
    );
    assert_eq!(
        config.advisor.provider.credential_file.as_deref(),
        Some("credentials/coding-plan/auth.json")
    );

    let config_text =
        std::fs::read_to_string(workspace.path().join(".agent-monitor").join("config.json"))
            .expect("config");
    assert!(!config_text.contains("dedicated-plan-key"));
    assert!(!config_text.contains("plan-access-token"));
    assert!(!config_text.contains("refresh-must-not-copy"));

    let profile_text = std::fs::read_to_string(
        workspace
            .path()
            .join(".agent-monitor")
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
    )
    .expect("materialized profile");
    assert!(profile_text.contains("dedicated-plan-key"));
    assert!(!profile_text.contains("plan-access-token"));
    assert!(!profile_text.contains("refresh-must-not-copy"));
    assert!(!profile_text.contains("id-must-not-copy"));
}

#[test]
fn coding_plan_credential_import_rewrites_existing_local_cli_auth_reference() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store = ProjectStore::open(workspace.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "advisor": {
            "enabled": true,
            "provider": {
              "endpoint": "https://api.openai.com/v1/chat/completions",
              "model": "gpt-5.5",
              "credential_source": "coding_plan",
              "credential_file": "C:\\Users\\yys\\.codex\\auth.json"
            }
          }
        }"#,
    )
    .expect("seed config");
    let source_dir = tempfile::tempdir().expect("source");
    let source_file = source_dir.path().join("codex-auth.json");
    std::fs::write(&source_file, r#"{"OPENAI_API_KEY":"dedicated-plan-key"}"#)
        .expect("source credential");

    let config = coding_agent_monitor::import_coding_plan_advisor_credentials(
        workspace.path(),
        &source_file,
        None,
        None,
    )
    .expect("import coding-plan credentials");

    assert_eq!(
        config.advisor.provider.credential_file.as_deref(),
        Some("credentials/coding-plan/auth.json")
    );
    let config_text =
        std::fs::read_to_string(workspace.path().join(".agent-monitor").join("config.json"))
            .expect("config");
    assert!(!config_text.contains(".codex"));
    assert!(!config_text.contains("dedicated-plan-key"));
}

#[test]
fn coding_plan_credential_import_rejects_profile_without_supported_token() {
    let workspace = tempfile::tempdir().expect("workspace");
    let source_dir = tempfile::tempdir().expect("source");
    let source_file = source_dir.path().join("auth.json");
    std::fs::write(&source_file, r#"{"refresh_token":"not-usable"}"#).expect("source credential");

    let error = coding_agent_monitor::import_coding_plan_advisor_credentials(
        workspace.path(),
        &source_file,
        None,
        None,
    )
    .expect_err("missing advisor token should fail");

    let message = error.to_string();
    assert!(message.contains("supported advisor token"));
    assert!(
        !workspace
            .path()
            .join(".agent-monitor")
            .join("credentials")
            .exists()
    );
}

#[test]
fn coding_plan_credential_import_rejects_codex_or_claude_runtime_auth_sources() {
    for (cli_dir, auth_file) in [(".codex", "auth.json"), (".claude", ".credentials.json")] {
        let workspace = tempfile::tempdir().expect("workspace");
        let home = tempfile::tempdir().expect("home");
        let source_dir = home.path().join(cli_dir);
        std::fs::create_dir_all(&source_dir).expect("source dir");
        let source_file = source_dir.join(auth_file);
        std::fs::write(
            &source_file,
            r#"{"tokens":{"access_token":"runtime-cli-token"}}"#,
        )
        .expect("runtime auth");

        let error = coding_agent_monitor::import_coding_plan_advisor_credentials(
            workspace.path(),
            &source_file,
            None,
            None,
        )
        .expect_err("runtime CLI auth must not become coding-plan credentials");

        let message = error.to_string();
        assert!(message.contains("local CLI auth"));
        assert!(message.contains(cli_dir));
        assert!(
            !workspace
                .path()
                .join(".agent-monitor")
                .join("credentials")
                .exists()
        );
    }
}

#[test]
fn coding_plan_credential_import_uses_profile_endpoint_for_jwt_plan_token() {
    let workspace = tempfile::tempdir().expect("workspace");
    let source_dir = tempfile::tempdir().expect("source");
    let source_file = source_dir.path().join("auth.json");
    std::fs::write(
        &source_file,
        r#"{
          "tokens": {
            "access_token": "eyJheader.payload.signature"
          },
          "endpoint": "https://coding-plan.example.test/v1/chat/completions",
          "model": "coding-plan-advisor"
        }"#,
    )
    .expect("source credential");

    let config = coding_agent_monitor::import_coding_plan_advisor_credentials(
        workspace.path(),
        &source_file,
        None,
        None,
    )
    .expect("import coding-plan credentials");

    assert_eq!(
        config.advisor.provider.endpoint,
        "https://coding-plan.example.test/v1/chat/completions"
    );
    assert_eq!(config.advisor.provider.model, "coding-plan-advisor");
    assert_eq!(
        config.advisor.provider.credential_source,
        coding_agent_monitor::AdvisorCredentialSource::CodingPlan
    );
}

#[test]
fn coding_plan_credential_import_rejects_jwt_plan_token_without_dedicated_endpoint() {
    let workspace = tempfile::tempdir().expect("workspace");
    let source_dir = tempfile::tempdir().expect("source");
    let source_file = source_dir.path().join("auth.json");
    std::fs::write(
        &source_file,
        r#"{"tokens":{"access_token":"eyJheader.payload.signature"}}"#,
    )
    .expect("source credential");

    let error = coding_agent_monitor::import_coding_plan_advisor_credentials(
        workspace.path(),
        &source_file,
        None,
        None,
    )
    .expect_err("JWT-style coding-plan credentials need a dedicated endpoint");

    let message = error.to_string();
    assert!(message.contains("coding-plan"));
    assert!(message.contains("endpoint"));
    assert!(
        !workspace
            .path()
            .join(".agent-monitor")
            .join("credentials")
            .exists()
    );
}

#[test]
fn local_agent_config_import_copies_codex_and_claude_settings_without_secret_values() {
    let workspace = tempfile::tempdir().expect("workspace");
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".codex")).expect("codex dir");
    std::fs::create_dir_all(home.path().join(".claude")).expect("claude dir");
    std::fs::write(
        home.path().join(".codex").join("config.toml"),
        r#"
model = "gpt-5.5"
model_reasoning_effort = "xhigh"
sandbox_mode = "danger-full-access"
approval_policy = "never"
api_key = "sk-should-not-be-imported"
"#,
    )
    .expect("codex config");
    std::fs::write(
        home.path().join(".claude").join("settings.json"),
        r#"{
          "env": {
            "ANTHROPIC_API_KEY": "should-not-be-imported",
            "SAFE_FLAG": "1"
          },
          "enabledPlugins": {
            "frontend-design@claude-plugins-official": true,
            "disabled-plugin": false
          },
          "effortLevel": "medium",
          "tui": "default",
          "skipDangerousModePermissionPrompt": true
        }"#,
    )
    .expect("claude settings");

    let config = coding_agent_monitor::import_local_agent_configs(
        workspace.path(),
        home.path(),
        coding_agent_monitor::LocalAgentConfigImportOptions {
            codex: true,
            claude_code: true,
            copy_credentials: false,
            advisor_credential_source: None,
            advisor_credential_file: None,
        },
    )
    .expect("import configs");

    let codex = config.local_agents.codex.expect("codex import");
    assert_eq!(codex.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(codex.model_reasoning_effort.as_deref(), Some("xhigh"));
    assert_eq!(codex.sandbox_mode.as_deref(), Some("danger-full-access"));
    assert_eq!(codex.approval_policy.as_deref(), Some("never"));
    assert_eq!(codex.command, vec!["codex", "exec", "--json"]);
    assert_eq!(config.adapters.codex.can_run_headless, Some(true));

    let claude = config.local_agents.claude_code.expect("claude import");
    assert_eq!(claude.effort_level.as_deref(), Some("medium"));
    assert_eq!(claude.tui.as_deref(), Some("default"));
    assert_eq!(
        claude.enabled_plugins,
        vec!["frontend-design@claude-plugins-official"]
    );
    assert_eq!(
        claude.env_keys,
        vec!["ANTHROPIC_API_KEY".to_string(), "SAFE_FLAG".to_string()]
    );
    assert_eq!(claude.command, vec!["claude"]);
    assert_eq!(config.adapters.claude_code.can_inject_context, Some(true));

    let config_text =
        std::fs::read_to_string(workspace.path().join(".agent-monitor").join("config.json"))
            .expect("project config");
    assert!(config_text.contains("ANTHROPIC_API_KEY"));
    assert!(!config_text.contains("should-not-be-imported"));
    assert!(!config_text.contains("sk-should-not-be-imported"));
}

#[test]
fn local_agent_config_import_never_copies_local_cli_auth_into_project_credentials() {
    let workspace = tempfile::tempdir().expect("workspace");
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".codex")).expect("codex dir");
    std::fs::create_dir_all(home.path().join(".claude")).expect("claude dir");
    std::fs::write(
        home.path().join(".codex").join("config.toml"),
        "model = \"gpt-5.5\"\n",
    )
    .expect("codex config");
    std::fs::write(
        home.path().join(".codex").join("auth.json"),
        "{\"token\":\"codex-secret\"}\n",
    )
    .expect("codex auth");
    std::fs::write(
        home.path().join(".claude").join("settings.json"),
        "{\"effortLevel\":\"medium\"}\n",
    )
    .expect("claude settings");
    std::fs::write(
        home.path().join(".claude").join(".credentials.json"),
        "{\"token\":\"claude-secret\"}\n",
    )
    .expect("claude credentials");

    let config = coding_agent_monitor::import_local_agent_configs(
        workspace.path(),
        home.path(),
        coding_agent_monitor::LocalAgentConfigImportOptions {
            codex: true,
            claude_code: true,
            copy_credentials: false,
            advisor_credential_source: None,
            advisor_credential_file: None,
        },
    )
    .expect("import configs");

    let codex = config.local_agents.codex.expect("codex import");
    let claude = config.local_agents.claude_code.expect("claude import");
    assert!(codex.uses_native_auth);
    assert!(claude.uses_native_auth);
    assert_eq!(codex.advisor_credential_file, None);
    assert_eq!(claude.advisor_credential_file, None);
    assert!(
        !workspace
            .path()
            .join(".agent-monitor")
            .join("credentials")
            .exists()
    );

    let config_text =
        std::fs::read_to_string(workspace.path().join(".agent-monitor").join("config.json"))
            .expect("project config");
    let config_json: serde_json::Value =
        serde_json::from_str(&config_text).expect("project config json");
    assert!(
        config_json
            .pointer("/local_agents/codex/credential_file")
            .is_none()
    );
    assert!(
        config_json
            .pointer("/local_agents/claude_code/credential_file")
            .is_none()
    );
    assert!(!config_text.contains("codex-secret"));
    assert!(!config_text.contains("claude-secret"));
    assert!(!config.advisor.enabled);
    assert_eq!(config.advisor.provider.model, "");
    assert_eq!(
        config.advisor.provider.credential_source,
        coding_agent_monitor::AdvisorCredentialSource::Env
    );
    assert_eq!(config.advisor.provider.credential_file, None);
}

#[test]
fn local_agent_config_import_rejects_local_cli_auth_copy_request() {
    let workspace = tempfile::tempdir().expect("workspace");
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".codex")).expect("codex dir");
    std::fs::write(
        home.path().join(".codex").join("config.toml"),
        "model = \"gpt-5.5\"\n",
    )
    .expect("codex config");
    std::fs::write(
        home.path().join(".codex").join("auth.json"),
        "{\"token\":\"codex-secret\"}\n",
    )
    .expect("codex auth");

    let error = coding_agent_monitor::import_local_agent_configs(
        workspace.path(),
        home.path(),
        coding_agent_monitor::LocalAgentConfigImportOptions {
            codex: true,
            claude_code: false,
            copy_credentials: true,
            advisor_credential_source: None,
            advisor_credential_file: None,
        },
    )
    .expect_err("local cli credential copy should be rejected");

    let message = error.to_string();
    assert!(message.contains("dedicated advisor credentials"));
    assert!(
        !workspace
            .path()
            .join(".agent-monitor")
            .join("credentials")
            .exists()
    );
}

#[test]
fn local_agent_config_import_rejects_advisor_credential_source_without_file() {
    let workspace = tempfile::tempdir().expect("workspace");
    let home = tempfile::tempdir().expect("home");

    let error = coding_agent_monitor::import_local_agent_configs(
        workspace.path(),
        home.path(),
        coding_agent_monitor::LocalAgentConfigImportOptions {
            codex: true,
            claude_code: true,
            copy_credentials: false,
            advisor_credential_source: Some(
                coding_agent_monitor::AdvisorCredentialSource::CodingPlan,
            ),
            advisor_credential_file: None,
        },
    )
    .expect_err("advisor credential source without file should be rejected");

    let message = error.to_string();
    assert!(message.contains("credential_file"));
    assert!(message.contains("coding_plan"));
    assert!(!workspace.path().join(".agent-monitor").exists());
}

#[test]
fn local_agent_config_import_links_dedicated_coding_plan_credentials_without_copying_agent_auth() {
    let workspace = tempfile::tempdir().expect("workspace");
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".codex")).expect("codex dir");
    std::fs::write(
        home.path().join(".codex").join("config.toml"),
        "model = \"gpt-5.5\"\n",
    )
    .expect("codex config");
    let coding_plan_dir = home.path().join("coding-plan");
    std::fs::create_dir_all(&coding_plan_dir).expect("coding plan dir");
    let coding_plan_file = coding_plan_dir.join("auth.json");
    std::fs::write(
        &coding_plan_file,
        "{\"OPENAI_API_KEY\":\"dedicated-plan-key\"}\n",
    )
    .expect("coding plan credential");
    let coding_plan_file = coding_plan_file.display().to_string();

    let config = coding_agent_monitor::import_local_agent_configs(
        workspace.path(),
        home.path(),
        coding_agent_monitor::LocalAgentConfigImportOptions {
            codex: true,
            claude_code: false,
            copy_credentials: false,
            advisor_credential_source: Some(
                coding_agent_monitor::AdvisorCredentialSource::CodingPlan,
            ),
            advisor_credential_file: Some(coding_plan_file.clone()),
        },
    )
    .expect("import configs");

    let codex = config.local_agents.codex.expect("codex import");
    assert!(codex.uses_native_auth);
    assert_eq!(codex.advisor_credential_file, None);
    assert!(config.advisor.enabled);
    assert_eq!(config.advisor.provider.model, "gpt-5.5");
    assert_eq!(
        config.advisor.provider.credential_source,
        coding_agent_monitor::AdvisorCredentialSource::CodingPlan
    );
    assert_eq!(
        config.advisor.provider.credential_file.as_deref(),
        Some(coding_plan_file.as_str())
    );
    assert!(
        !workspace
            .path()
            .join(".agent-monitor")
            .join("credentials")
            .exists()
    );

    let config_text =
        std::fs::read_to_string(workspace.path().join(".agent-monitor").join("config.json"))
            .expect("project config");
    assert!(config_text.contains("coding_plan"));
    assert!(config_text.contains("uses_native_auth"));
    assert!(!config_text.contains("dedicated-plan-key"));
    assert!(!config_text.contains("advisor_credential_file"));
}

#[test]
fn local_agent_config_import_rejects_cli_auth_path_as_dedicated_coding_plan_credentials() {
    let workspace = tempfile::tempdir().expect("workspace");
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".claude")).expect("claude dir");
    let claude_auth = home.path().join(".claude").join(".credentials.json");
    std::fs::write(
        &claude_auth,
        r#"{"tokens":{"access_token":"cli-auth-token"}}"#,
    )
    .expect("claude auth");

    let error = coding_agent_monitor::import_local_agent_configs(
        workspace.path(),
        home.path(),
        coding_agent_monitor::LocalAgentConfigImportOptions {
            codex: false,
            claude_code: true,
            copy_credentials: false,
            advisor_credential_source: Some(
                coding_agent_monitor::AdvisorCredentialSource::CodingPlan,
            ),
            advisor_credential_file: Some(claude_auth.display().to_string()),
        },
    )
    .expect_err("local Claude CLI auth must not be accepted as coding-plan credentials");

    let message = error.to_string();
    assert!(message.contains("local CLI auth"));
    assert!(message.contains(".claude"));
    assert!(
        !workspace
            .path()
            .join(".agent-monitor")
            .join("config.json")
            .exists()
    );
}

#[test]
fn local_agent_config_import_rejects_coding_plan_profile_without_supported_token() {
    let workspace = tempfile::tempdir().expect("workspace");
    let home = tempfile::tempdir().expect("home");
    let coding_plan_dir = home.path().join("coding-plan");
    std::fs::create_dir_all(&coding_plan_dir).expect("coding plan dir");
    let coding_plan_file = coding_plan_dir.join("auth.json");
    std::fs::write(&coding_plan_file, "{\"refresh_token\":\"not-usable\"}\n")
        .expect("coding plan credential");

    let error = coding_agent_monitor::import_local_agent_configs(
        workspace.path(),
        home.path(),
        coding_agent_monitor::LocalAgentConfigImportOptions {
            codex: false,
            claude_code: false,
            copy_credentials: false,
            advisor_credential_source: Some(
                coding_agent_monitor::AdvisorCredentialSource::CodingPlan,
            ),
            advisor_credential_file: Some(coding_plan_file.display().to_string()),
        },
    )
    .expect_err("invalid coding-plan credential profile should be rejected");

    let message = error.to_string();
    assert!(message.contains("coding_plan"));
    assert!(message.contains("supported advisor token"));
    assert!(
        !workspace
            .path()
            .join(".agent-monitor")
            .join("config.json")
            .exists()
    );
}

#[test]
fn project_config_loads_verifiers_policy_and_adapter_overrides() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "max_user_questions_per_hour": 1,
            "switch_agent_cooldown_min": 30,
            "max_parallel_writable_agents": 1,
            "worktree_lock_stale_after_secs": 86400,
            "handoff_outcome_timeout_secs": 600
          },
          "security": {
            "redact_env": false,
            "redact_auth_files": true,
            "deny_paths": [".env", ".env.*", "**/auth.json", "**/*.pem"],
            "protected_paths": ["migrations/**", "infra/**"]
          },
          "verifiers": [
            {
              "id": "parser_targeted",
              "command": "cargo test parser::tests::handles_nested",
              "scope": "targeted",
              "timeout_secs": 120,
              "paths": ["src/parser.rs", "tests/parser.rs"],
              "acceptance_patterns": ["nested parser", "comment roundtrip"]
            }
          ],
          "adapters": {
            "pi": { "enabled": true, "requires_external_sandbox": true },
            "codex": { "enabled": true, "can_run_headless": true }
          }
        }"#,
    )
    .expect("config");

    let config = ProjectConfig::load(store.root()).expect("load config");

    assert_eq!(config.policy.max_user_questions_per_hour, 1);
    assert_eq!(config.policy.switch_agent_cooldown_min, 30);
    assert_eq!(config.policy.worktree_lock_stale_after_secs, Some(86400));
    assert_eq!(config.policy.handoff_outcome_timeout_secs, Some(600));
    assert!(!config.security.redact_env);
    assert!(config.security.redact_auth_files);
    assert!(config.security.deny_paths.contains(&"**/*.pem".to_string()));
    assert!(
        config
            .security
            .protected_paths
            .contains(&"infra/**".to_string())
    );
    assert_eq!(config.verifiers.len(), 1);
    assert_eq!(config.verifiers[0].id, "parser_targeted");
    assert_eq!(config.verifiers[0].scope, VerificationScope::Targeted);
    assert_eq!(
        config.verifiers[0].acceptance_patterns,
        vec!["nested parser", "comment roundtrip"]
    );
    assert_eq!(config.adapters.pi.requires_external_sandbox, Some(true));
    assert_eq!(config.adapters.codex.can_run_headless, Some(true));
}
