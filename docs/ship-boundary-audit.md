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
| Local Codex/Claude configs can be imported without copying runtime credentials | Proven | `config import-local`, coding-plan credential import tests | Dedicated advisor credentials must remain separate from `.codex` and `.claude` auth. |
| Hard policy blocks unsafe progress | Proven | stale-verification, ask-user, worktree-lock, target-agent, sandbox, and packet-precondition tests | Add more destructive-command fixtures as adapter hooks expand. |
| Packet compiler and dispatch trail are replayable | Proven | `ControlPacket`, `dispatch_control_packet`, decision-trail replay tests | Live adapter transport beyond outbox/hook surfaces remains incremental. |
| Codex, Claude Code, OpenCode, and Pi are modeled as different surfaces | Partial | `AgentKind`, adapter capabilities, injections, ingest tests | Pi is boundary-safe but not full live adapter parity; writable Pi remains blocked unless explicitly sandboxed. |
| Runtime validation is surface-aware and not browser-only | Proven for control logic | `validation_surface` classifier tests; runtime validation docs; web/mobile/native/system/ML entropy tests; `config verifier` can register runtime verifier mappings | Real platform validation requires each project to register the relevant platform validators. |
| Runtime validation probes cannot fake success | Proven | `run_probe` now returns `Unknown` unless a verifier has `acceptance_patterns = ["runtime_validation:<surface>"]`; runtime probe tests | Add richer platform runners later through the verifier registry. |
| Worktree/write safety is enforced | Proven | lock acquire/release, wrapped-agent lock, stale-lock, handoff lock tests | Multi-worktree merge queue is still future work. |
| Repo blame and requirement proof surfaces exist | Proven as first slice | `repo_hunks`, `trace`, `requirements`, `completion-certificate` commands and tests; project-contract requirements now derive from `AGENTS.md`/`CLAUDE.md`; `agent-monitor trace` can bind rationale to exact requirement ids; stale-verification `force_verification` decisions bind to the matching contract requirement and verifier outcomes inherit that id; `config verifier` registers verifier ids without hand-editing JSON | Requirement closure still needs mapped proof evidence for the remaining project-contract requirements. |
| Completion certificate refuses weak proof | Proven after fix | empty-scope completion certificate test; current self-certificate reports 9 scoped project-contract requirements and `blocked` without a `requirement_scope` incident | Map requirements to trace/control/outcome/verifier evidence before claiming this project itself is complete. |
| Native dashboard surfaces bounded operational state | Proven as first slice | `agent-monitor-ui`, dashboard tests for events, advisor status, locks, probes, requirements, decision trails | Visual QA of the native app should be repeated before packaging. |
| Dev-history packaging and analysis exists | Proven as first slice | `dev_history` module/tests and docs examples | Broader mining heuristics can improve after more real packages. |
| Calibration records expected vs observed outcomes | Proven as first slice | `calibration` module/tests | Needs real long-run outcome data to become predictive. |

## Current Blockers To Completion

1. The monitor's own `.agent-monitor` store now has 9 scoped requirements
   derived from `AGENTS.md`, and `completion-certificate --workspace=.`
   correctly reports `blocked`. The trace-rationale requirement now has
   necessary trace and repo-hunk proof. The stale-verification requirement has
   a deterministic control/outcome proof path once a configured verifier runs.
   Verifier ids can now be registered through `agent-monitor config verifier`
   without hand-editing `.agent-monitor/config.json`.
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
```

The self `completion-certificate --workspace=.` check now reports 9 scoped
project-contract requirements and remains blocked on unmapped proof plus stale
verification, not on empty requirement scope.
