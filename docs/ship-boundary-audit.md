# Ship Boundary Audit

Current date: 2026-06-24

This audit tracks the current implementation against the GPT Pro control-plane
boundary and the project contract in `AGENTS.md`. It is evidence for continued
work, not a completion claim.

## Boundary

The shippable monitor slice is an external, event-sourced supervisor for coding
agents. It must observe agent work, build bounded evidence, estimate operational
uncertainty, enforce deterministic safety rules, use an endpoint advisor only as
a bounded diagnostic component, compile packets for adapter surfaces, record
outcomes, and make changes traceable enough to explain why they exist.

## Status Matrix

| Requirement | Status | Current evidence | Closure gap |
| --- | --- | --- | --- |
| External control-plane thesis is documented in dense agent prompts | Proven | `AGENTS.md`, `CLAUDE.md` | Keep detailed history out of prompt files. |
| Normalized append-only store with replayable evidence | Proven | `ProjectStore` event/side-log appenders, `case-files.jsonl`, `advice.jsonl`, `packets.jsonl`, `dispatch.jsonl`, `outcomes.jsonl`; persistence and replay tests | SQLite remains future storage, not required for this slice. |
| Event provenance is stamped before persistence | Proven | `ProjectStore::append_event`, adapter JSONL ingest tests | Continue adding adapter fixture tests as schemas drift. |
| Entropy vector and bounded case file exist | Proven | `ControlCaseFile`, `EntropyVector`, case-file builder and advisor-visible bounding tests | Ongoing calibration quality depends on real outcome data. |
| Advisor is endpoint-configured, schema-bounded, and validator-owned | Proven | `advisor_client`, `advisor_boundary`, config tests, evidence-id validation tests | Live provider compatibility still depends on configured endpoint and credential profile. |
| Local Codex/Claude configs can be imported without copying runtime credentials | Proven | `config import-local`, `config runtime-auth`, runtime-auth capability tests, coding-plan credential import tests, cc-switch prior-art review | Dedicated advisor credentials remain separate from `.codex` and `.claude` auth; cc-switch-style broker profiles are metadata-only runtime-auth inputs, not project-secret imports. Live broker health probing remains future work. |
| Hard policy blocks unsafe progress | Proven | stale-verification, ask-user, worktree-lock, target-agent, sandbox, and packet-precondition tests | Add more destructive-command fixtures as adapter hooks expand. |
| Packet compiler and dispatch trail are replayable | Proven | `ControlPacket`, `dispatch_control_packet`, decision-trail replay tests | Live adapter transport beyond outbox/hook surfaces remains incremental. |
| Codex, Claude Code, OpenCode, and Pi are modeled as different surfaces | Partial | `AgentKind`, adapter capabilities, injections, ingest tests | Pi is boundary-safe but not full live adapter parity; writable Pi remains blocked unless explicitly sandboxed. |
| Runtime validation is surface-aware and not browser-only | Proven for control logic | `validation_surface` classifier tests; runtime validation docs; web/mobile/native/system/ML entropy tests; `config verifier` can register runtime verifier mappings | Real platform validation requires each project to register the relevant platform validators. |
| Runtime validation probes cannot fake success | Proven | `run_probe` now returns `Unknown` unless a verifier has `acceptance_patterns = ["runtime_validation:<surface>"]`; runtime probe tests | Add richer platform runners later through the verifier registry. |
| Worktree/write safety is enforced | Proven | lock acquire/release, wrapped-agent lock, stale-lock, handoff lock tests | Multi-worktree merge queue is still future work. |
| Repo blame and requirement proof surfaces exist | Proven as first slice | `repo_hunks`, `trace`, `requirements`, `completion-certificate` commands and tests; project-contract requirements now derive from `AGENTS.md`/`CLAUDE.md`; `agent-monitor trace` can bind rationale to exact requirement ids; stale-verification `force_verification` decisions bind to the matching contract requirement and verifier outcomes inherit that id; `config verifier` registers verifier ids without hand-editing JSON; verifier outcomes can now inherit requirement ids from case-file verifier mappings | Keep adding source-backed proof maps when new contract requirements are introduced. |
| Completion certificate refuses weak proof | Proven after fix | empty-scope completion certificate test; report-level verification freshness stays separate from requirement closure; gapless high-strength proof can close an initially unmapped requirement; current self-certificate scopes 9 project-contract requirements and closes them only when source, verifier, control, outcome, trace, and repo-hunk evidence are present | Future completion claims still need a fresh clean-repo certificate. |
| Native dashboard surfaces bounded operational state | Proven as first slice | `agent-monitor-ui`, dashboard tests for events, advisor status, locks, probes, requirements, decision trails | Visual QA of the native app should be repeated before packaging. |
| Dev-history packaging and analysis exists | Proven as first slice | `dev_history` module/tests and docs examples | Broader mining heuristics can improve after more real packages. |
| Calibration records expected vs observed outcomes | Proven as first slice | `calibration` module/tests | Needs real long-run outcome data to become predictive. |

## Current Blockers To Completion

1. The monitor's own `.agent-monitor` store now has 9 scoped requirements
   derived from `AGENTS.md`. Completion claims require a fresh
   `completion-certificate --workspace=.` result with a clean git anchor,
   fresh verifier results, and no unresolved proof gaps.
2. Pi support is safe but not feature-parity with Codex, Claude Code, and
   OpenCode live control surfaces.
3. Runtime validation execution is intentionally verifier-registry based. A
   project must configure platform validators before `runtime_validation` can
   succeed.

## Verification

Latest verified commands:

```powershell
cargo fmt --check
cargo test --quiet
cargo clippy --quiet -- -D warnings
```

All passed again on 2026-06-24 after the project-contract requirement scope
change.

The focused project-contract requirement extraction test also passed:

```powershell
cargo test --quiet --test entropy_control case_file_scopes_project_contract_requirements_from_agents_md
```

The trace-proof slice added focused coverage:

```powershell
cargo test --quiet parses_trace_command_with_requirement_binding
cargo test --quiet record_trace_entry_links_requirement_id_to_necessary_proof
cargo test --quiet requirement_id_trace_links_matching_repo_hunk_as_necessary_proof
cargo test --quiet force_verification_links_project_contract_requirement_to_control_and_outcome_proof
cargo test --quiet config_verifier_command
cargo test --quiet verifier_config_writer_tolerates_existing_invalid_advisor_profile
cargo test --quiet completion_certificate_report_preserves_global_verification_when_requirements_unmapped
cargo test --quiet completion_certificate_report_closes_unmapped_requirement_with_gapless_proof
```

The self `completion-certificate --workspace=.` check now reports 9 scoped
project-contract requirements. A completion claim is valid only when the latest
certificate reports all 9 closed with passed verification and `git_dirty:
false`.

## Project Contract Proof Map

The certificate requirement ids are derived from `AGENTS.md`. These anchors
connect the contract language to implementation and tests so the monitor can
answer why each invariant is considered proven.

| Contract requirement | Source anchors | Test anchors |
| --- | --- | --- |
| Active agent does not judge its own robustness; the monitor uses external evidence | `build_control_case_file_with_config`, deterministic entropy scoring, verifier and repo-audit evidence ingestion | `case_file_maps_project_contract_requirement_to_configured_verifier`, verifier and completion-certificate requirement tests |
| Continue obvious work without asking the user; ask only for authority decisions | control policy and user-decision entropy gates in `src/lib.rs` | `policy_validator_replaces_low_value_ask_user_with_continue_working`, `ordinary_continue_question_does_not_raise_user_decision_entropy`, `advisor_request_forbids_ask_user_when_user_decision_entropy_is_low`, `advisor_request_forbids_ask_user_when_interrupt_budget_is_exhausted`, `advise_workspace_selects_bounded_ask_user_for_user_authority_blocker`, `advise_workspace_downgrades_ask_user_when_hourly_interrupt_budget_is_exhausted` |
| Do not run two writable agents on the same worktree | worktree lock acquisition, conflict detection, and isolated worktree policy in `src/lib.rs` | worktree lock acquire/release, stale-release, conflict, and handoff lock tests |
| Wrapped launched agent is a writable owner | `agent-monitor wrap` lock acquisition and release around the child process | wrapped-agent lock lifecycle tests |
| Do not send secrets, auth files, private env values, or tainted excerpts | `src/redaction.rs`, packet validation, advisor request pruning, config credential import guards | `advisor_case_file_prunes_tainted_evidence_before_endpoint_request`, `case_file_redacts_secret_like_evidence_before_advisor_use`, `case_file_filters_secret_like_durable_memory_before_packets`, `control_packet_with_secret_like_content_is_rejected_before_outbox_or_log_write`, `local_agent_config_import_never_copies_local_cli_auth_into_project_credentials`, `coding_plan_credential_import_rejects_codex_or_claude_runtime_auth_sources`, `adapter_runtime_auth_config_rejects_secret_like_broker_metadata`, `case_file_adapter_capabilities_include_runtime_auth_metadata_without_secrets`, `case_file_adapter_capabilities_drop_invalid_runtime_auth_metadata`, `run_verifier_records_requirement_outcome_for_matching_continue_advice` |
| Every meaningful change needs trace rationale | trace CLI parsing, repo-audit hunk matching, trace/repo proof scoring in `src/main.rs`, `src/lib.rs`, and `src/requirements.rs` | `repo_audit_marks_modified_hunk_traced_when_line_trace_has_rationale`, `repo_audit_flags_matching_trace_without_rationale`, `policy_validator_replaces_progress_with_trace_and_verification_block`, `record_trace_entry_links_requirement_id_to_necessary_proof`, `requirements_query_links_proof_history_to_trace_evidence` |
| Durable memory is small, explicit, and source-backed | memory candidate extraction, durable memory promotion, conflict quarantine, and trusted-source checks in `src/lib.rs` | `promote_memory_candidate_persists_manual_review_memory_as_durable`, `promote_memory_candidate_rejects_agent_claim_as_trusted_source`, `promote_memory_candidate_rejects_secret_like_claims_before_persisting`, `promote_memory_candidate_rejects_already_governed_memory_id`, `case_file_loads_active_verified_memory_separately_from_unverified_candidates`, `case_file_does_not_promote_agent_claim_memory_as_durable`, `case_file_uses_latest_memory_record_status_for_append_only_governance`, `case_file_quarantines_conflicting_active_memory_across_ids`, `promote_memory_candidate_rejects_cross_id_conflicting_durable_claim` |
| Advisor is stateless, schema-constrained, denied direct tools, and validator-owned | `src/advisor_client.rs`, `src/advisor_boundary.rs`, advisor validation and fallback control in `src/lib.rs` | `advisor_system_prompt_is_schema_bounded_and_action_dense`, `advisor_validation_rejects_evidence_ids_outside_case_file`, `advisor_validation_rejects_unknown_top_evidence_and_packet_refs`, `advisor_validation_rejects_out_of_range_entropy_estimates`, `advisor_validation_rejects_missing_dominant_entropy_score`, `advisor_validation_rejects_out_of_range_expected_entropy_delta`, `advisor_validation_rejects_tainted_non_packet_diagnostics`, `advisor_validation_rejects_pause_even_if_advisor_attempts_to_stop_work`, `advise_workspace_falls_back_when_endpoint_advisor_cites_unknown_packet_evidence`, `advise_workspace_falls_back_when_endpoint_advisor_omits_dominant_entropy_score` |
