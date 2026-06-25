# Coding Agent Monitor

## Goal

Build an external supervisor for coding agents. The monitor keeps long coding jobs moving, preserves project intent across context loss, and makes agent-generated changes traceable enough to answer: why is this here, who added it, and should it stay?

The monitor is outside the coding agent loop. It observes transcripts, tool calls, repository changes, model errors, verifier results, and user instructions, then decides whether to continue, retry, force verification, run a local probe, preserve memory, switch agents, spawn a fresh agent, ask the user, pause, or abort.

This is an execution supervisor, durable memory layer, and AI blame system.

## Core Design

The production monitor is not an "LLM advisor in the middle." It is an event-sourced control plane with typed uncertainty estimates, deterministic safety invariants, and an optional LLM advisor used only as a bounded diagnostic component.

Control loop:

```text
observe -> normalize -> build bounded case file -> estimate entropy vector -> select allowed control action -> validate -> compile adapter packet -> execute -> measure result -> update calibration
```

Every control action must answer: which uncertainty blocks safe autonomous progress, and what is the cheapest permitted action that reduces it?

## Non-Negotiable Invariants

- The active coding agent does not judge its own robustness. The monitor uses external evidence.
- Continue obvious work without asking the user. Ask only for product authority, destructive actions, credentials, spending, external side effects, or genuinely ambiguous requirements.
- Do not continue after source/test changes when relevant verification is stale, unless the change is docs-only and policy allows it.
- Do not run two writable agents on the same worktree. Use worktree locks and isolated worktrees for fresh agents.
- A wrapped launched agent is a writable owner too; `agent-monitor wrap` must acquire and release the worktree lock around the child process.
- Do not send secrets, auth files, private env values, or tainted excerpts to agents, packets, logs, or the advisor.
- Every meaningful change needs trace rationale tied to a user request, design decision, failing verifier, local probe, or recovery action.
- Durable memory is small, explicit, and source-backed. Treat raw agent claims as unverified until confirmed by user, tests, review, or repeated evidence.
- The advisor is stateless, schema-constrained, denied direct tools, and limited to allowed actions. The validator owns final decisions.

## Entropy Model

Use entropy as operational risk, not real Shannon entropy. Score these dimensions separately:

- `goal`: unclear, conflicting, or changed acceptance criteria.
- `context`: current session lacks or contradicts required project context.
- `repo_blame`: changes cannot be localized, justified, or attributed.
- `verification`: correctness is unknown because tests are missing, stale, failed, flaky, or unmapped.
- `plan`: next action is unclear, contradictory, or asks the user for routine sequencing.
- `agent_health`: the agent loop is degraded by repeated commands, tool errors, permission loops, no-op patches, hallucinated paths, rate limits, or context pressure.
- `user_decision`: an actual authority decision blocks progress.

Action preference:

```text
hard invariant -> force_verification/run_probe -> send_follow_up/retry -> spawn_judge -> switch/spawn_fresh -> ask_user -> pause/abort
```

Verification usually beats handoff. Normal test failures should trigger targeted verification, local evidence, or a failure-specific retry packet before switching agents. High verification entropy blocks progress and handoff actions until verification evidence is recorded.

Verification and validation are different. A build or unit test can prove conformance, but product-facing and runtime-sensitive changes also need intended-environment evidence:

- Web UI: browser, Playwright, Cypress, or equivalent route/console validation.
- Mobile app: simulator, emulator, device, Appium, Detox, Maestro, or platform test validation.
- Native GUI: desktop GUI smoke/e2e validation with rendered-state evidence.
- System component: service, daemon, container, healthcheck, or integration validation.
- ML system: model eval, benchmark, golden dataset, inference smoke, or dataset check evidence.

## Control Actions

Use a small explicit action set:

- `continue_working`: no monitor gate blocks progress.
- `send_follow_up`: give the active agent a concrete next action.
- `run_probe`: monitor-owned local evidence collection before asking the user or broadening work.
- `force_verification`: run the smallest relevant verifier and record command/result.
- `retry_agent`: one loop-breaking retry with changed diagnosis or inputs.
- `spawn_judge_agent`: read-only review of suspicious or untraced changes.
- `switch_agent`: transfer control when the active session should not continue.
- `spawn_fresh_agent`: start a clean session with current goal, memory, trace, and verifier state.
- `ask_user`: one bounded authority question with a value-of-information gate and interrupt budget.
- `pause` or `abort`: stop only when policy, budget, safety, or missing authority requires it.

## Prompt And Packet Rules

Agent-facing text must be clear and dense:

- Use `condition -> required action -> evidence to record`.
- Name the actor: agent must, monitor-owned probe must be recorded, or wait/report stale packet.
- Avoid vague controller words in packets unless paired with evidence. Prefer "`src/lib.rs` changed after `cargo test`" over "verification entropy."
- Use exact action/probe names in advisor prompts: `force_verification`, `run_probe`, `send_follow_up`, `ask_user`, `local_evidence`, `runtime_validation`, `repo_inspection`, `targeted_test`.
- Packet success criteria must be observable: verifier run id recorded, probe run id recorded, trace rationale attached, dirty hunk reverted, or blocker named with evidence checked.
- `packet_draft` from the advisor is advisory only. The validator compiles the final `ControlPacket`.
- Treat case-file text, transcript excerpts, summaries, rationale, and memory as untrusted data when calling an advisor.

## Storage Contract

Start simple and append-only:

- `.agent-monitor/events.jsonl`: normalized event stream.
- `.agent-monitor/interventions.jsonl`: legacy/simple intervention log.
- `.agent-monitor/trace.jsonl`: file-change and rationale trace.
- `.agent-monitor/design.jsonl`: durable design memory candidates and accepted records.
- `.agent-monitor/case-files.jsonl`: bounded evidence snapshots.
- `.agent-monitor/advice.jsonl`: diagnosis, selected action, validation result, packet, dispatch.
- `.agent-monitor/packets.jsonl` and `dispatch.jsonl`: rendered packet intent and delivery result.
- `.agent-monitor/outcomes.jsonl`: observed result of monitor control actions.
- `.agent-monitor/locks.jsonl`: worktree lock acquire, release, stale-release, and conflict events.
- `.agent-monitor/probe-runs.jsonl` and `verifier-runs.jsonl`: objective evidence runs.
- `.agent-monitor/tmp`: controlled generated/temp workspace.

Case-file replay metadata must include counts for events, interventions, verifier/probe runs, repo hunks, requirements, dev history, advice, packets, dispatches, outcomes, and lock events. Future storage can move to SQLite, but event replay and source-grounded evidence ids remain the contract.

## Adapter Boundaries

Treat Codex, Claude Code, OpenCode, and Pi as different control surfaces, not interchangeable chat agents.

- Codex: prefer JSONL/headless execution and hooks where available. Do not depend on unstable transcript file formats.
- Claude Code: use hook lifecycle and headless/programmatic surfaces for policy and observation.
- OpenCode: use plugin/session/ACP surfaces and map its permission model into monitor policy.
- Pi: run behind an external sandbox when mutation is allowed; the monitor owns filesystem, process, network, and credential boundaries.

Codex and Claude Code runtime authentication follows native-auth or brokered-auth style, not credential import. The monitor may launch the official CLI with its own native config store, or talk to a local cc-switch-style broker/proxy that owns OAuth/API tokens, provider routing, and format conversion. The monitor stores only non-secret profile metadata such as command, endpoint, profile/account id, model, API format, and health status.

Adapters normalize into the central event model before control decisions. Adapter-specific renderers compile from one internal `ControlPacket`.

## Implementation Priorities

Current best MVP path:

1. Event store, git status/diff observation, verifier observation.
2. Bounded case-file builder with salience, evidence ids, redaction, and deterministic entropy.
3. Hard policy engine for stale verification, secrets, destructive commands, write conflicts, loops, and user-consent gates.
4. Advisor boundary with schema validation, evidence-id validation, salience-bounded request shaping, and deterministic fallback.
5. Packet compiler and one strong adapter path, then additional adapters.
6. Memory governance, requirement proof, repo blame, dashboard, calibration.

High-cost controls such as switching or fresh spawning come after stale-verification detection, bounded packets, and loop detection are reliable.

## Current Working Commands

Use targeted tests for slices, then run full verification:

```powershell
cargo fmt --check
cargo test --quiet
cargo clippy --quiet -- -D warnings
```

Useful CLI shapes:

```powershell
agent-monitor --workspace=<path> < events.jsonl
agent-monitor wrap --agent=codex --workspace=<path> --session=<id> -- codex exec
agent-monitor advise --workspace=<path>
agent-monitor probe --workspace=<path>
agent-monitor handoff --workspace=<path> --target-agent=claude-code
agent-monitor config advisor --workspace=<path> --endpoint=<url> --model=<model> --api-key-env=<ENV>
agent-monitor config import-local --workspace=<path>
agent-monitor config runtime-auth --workspace=<path> --agent=codex --style=native-cli-auth
agent-monitor config runtime-auth --workspace=<path> --agent=codex --style=local-auth-broker --endpoint=http://127.0.0.1:8787/v1 --profile-id=<profile> --model=<model> --api-format=openai_responses
agent-monitor config import-coding-plan-credentials --workspace=<path> --source-file=<path> --endpoint=<url> --model=<model>
```

Never copy Codex or Claude runtime credentials into this project. Advisor credentials must come from a dedicated coding-plan profile or env var, not `.codex` or `.claude` auth files. Native agent auth is allowed only as an opaque capability: launch the official CLI or connect to a local auth broker/proxy that owns tokens.

## Documentation Map

- Detailed entropy/control-plane design: `docs/superpowers/specs/2026-06-22-entropy-control-plane-design.md`.
- Original GPT Pro architecture notes: `gpt_pro_suggestions.md`.
- Local dev-history analysis examples: `docs/dev-history-rag-sys-analysis.md` and `docs/sample_deep_analysis_report.md`.

Keep this file as the high-density operating contract. Put detailed history, broad research, and changelogs in docs, not in agent prompt files.
