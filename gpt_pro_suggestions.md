Your architecture is directionally right, but the production version should not be “LLM advisor in the middle.” It should be an event-sourced control plane with typed uncertainty estimates, deterministic safety invariants, and an LLM used only as a bounded diagnostic/advisory component. The monitor’s job is not to decide whether the active agent is “good.” Its job is to keep the work loop in a low-entropy, verifiable, recoverable state.

Status note: this file is historical source material. The current operating contract is `AGENTS.md` / `CLAUDE.md` plus `docs/superpowers/specs/2026-06-22-entropy-control-plane-design.md`. In particular, browser validation is only one runtime surface; current policy uses `runtime_validation` for web UI, mobile app, native GUI, system component, and ML system evidence, and generic words like `e2e` or `integration` do not prove a surface without a matching surface signal or configured verifier mapping.

A useful framing:

`observe → normalize → build bounded case file → estimate entropy vector → select allowed control action → validate → compile adapter packet → execute → measure result → update calibration`

The core improvement is to make every action answer this question: “Which uncertainty is currently blocking safe autonomous progress, and what is the cheapest permitted action that reduces it?”

Codex, Claude Code, OpenCode, and Pi need to be treated as different control surfaces, not as interchangeable chat agents. Codex exposes local CLI operation, hooks, and `codex exec --json` JSONL automation; its hook docs also warn that transcript paths are convenient but not a stable hook interface, which means your monitor should normalize events rather than couple itself to one transcript format. ([developers.openai.com][1]) Claude Code has a richer hook lifecycle with session, turn, and per-tool events, and hook handlers receive JSON over stdin or HTTP and can return decisions. ([code.claude.com][2]) OpenCode has plugins, permission config, session export, session listing, ACP over newline-delimited JSON, and plugin events such as `tool.execute.before`, `tool.execute.after`, `session.idle`, and `session.error`. ([opencode.ai][3]) Pi is intentionally minimal and extensible, but its own docs state it has no built-in permission system for filesystem, process, network, or credential access, so your monitor must provide sandboxing and process boundaries externally. ([pi.dev][4])

## 1. Critique of the proposed architecture

The proposed layers are good, but the boundaries need tightening.

The deterministic hard policy layer should be split into two pieces: pre-action invariants and control-policy rules. Pre-action invariants are absolute: never allow two writable agents on the same worktree, never deliver secrets into an agent packet, never let a “continue” action run when verification is stale after code changes, never ask the user a question that can be resolved by logs, diffs, tests, or repo metadata. Control-policy rules are softer: retry once, then switch; rerun flaky tests; prefer fresh agent when context entropy is dominant.

The bounded case-file builder is essential, but it must produce an evidence object, not a prose summary. Every claim in the case file should have a source pointer: event id, file path plus git blob hash, test run id, diff hunk id, or memory id. Otherwise, the LLM advisor will invent continuity across stale logs.

The endpoint-configured LLM advisor should not “decide.” It should estimate dominant uncertainty, missing evidence, expected entropy reduction per action, and packet content. It should be schema-constrained, stateless, and denied direct tool access. The policy validator then turns its proposal into an executable action or denial.

The policy validator should also enforce user-annoyance controls. “Ask user” is not just another action. It is an expensive interrupt with a budget, cooldown, and value-of-information threshold.

Adapter-specific urgent/follow-up packets are the right idea, but they should be compiled from an intermediate `ControlPacket` representation. Do not hand-write four formats. Each adapter should declare capabilities: can inject context at session start, can block tools, can resume headless, can export JSON, can run in plan mode, can receive slash command, can start subagent, can run in sandbox, can attach to existing session.

Persistence should be event-sourced. Store evidence, diagnosis, action, compiled packet, dispatch result, resulting events, and final outcome. This is not just audit logging. It is how you calibrate the entropy model and debug bad supervisory choices.

## 2. Entropy model that is practical

Do not try to compute real Shannon entropy. Use “entropy” as an operational risk score: the estimated probability that the next autonomous step will waste time, degrade correctness, or require later human repair because of unresolved uncertainty.

Represent the state as an entropy vector:

```rust
enum EntropyKind {
    Goal,
    Context,
    RepoBlame,
    Verification,
    Plan,
    AgentHealth,
    UserDecision,
}

struct EntropyScore {
    kind: EntropyKind,
    score: u8,        // 0..100
    confidence: u8,   // 0..100: confidence in the estimate, not in success
    trend: Trend,     // Rising, Falling, Stable, Unknown
    top_causes: Vec<String>,
    evidence_ids: Vec<Uuid>,
    missing_evidence: Vec<String>,
    recommended_observations: Vec<ObservationRequest>,
}
```

Use deterministic features first, then let the LLM advisor interpret the ambiguous remainder.

Goal entropy is high when acceptance criteria are missing, conflicting, or changed mid-run. Signals: no extracted acceptance criteria, multiple incompatible goals, agent changed the task scope, unresolved product choice, or tests not mapped to requirements.

Context entropy is high when the active session likely lacks necessary context. Signals: compaction occurred, transcript is large, recent packet contradicted earlier design memory, agent repeatedly rereads the same files, or code changes rely on undocumented assumptions.

Repo/blame entropy is high when the failing behavior cannot be localized. Signals: broad diff, many unrelated files touched, failing stack trace does not intersect touched files, generated files mixed with hand-written files, ownership unclear, or recent dependency upgrade.

Verification entropy is high when correctness is unknown. Signals: code changed after last test run, no targeted test for modified behavior, test run failed but agent continued, flaky test signature, build cache contamination, or service dependency unavailable.

Plan entropy is high when the agent’s next move is unclear or contradictory. Signals: plan says “implement X” but latest tool call edits Y; agent has no current plan; repeated “I will now…” without execution; multiple abandoned plans.

Agent-health entropy is high when the agent loop itself is unhealthy. Signals: repeated identical commands, same patch reverted and reapplied, permission denial loops, tool failures, hallucinated file paths, service 5xx, context overflow, rate-limit backoff, or rising no-op diff ratio.

User-decision entropy is high when an unresolvable human preference or authority decision blocks progress. Signals: irreversible API/UX/security choice, credentials needed, production migration risk, or acceptance criteria genuinely underdetermined.

A production scoring model can start simple:

```rust
struct EntropyVector {
    goal: EntropyScore,
    context: EntropyScore,
    repo_blame: EntropyScore,
    verification: EntropyScore,
    plan: EntropyScore,
    agent_health: EntropyScore,
    user_decision: EntropyScore,
}

fn risk(score: &EntropyScore, impact_weight: f32) -> f32 {
    let s = score.score as f32 / 100.0;
    let c = score.confidence as f32 / 100.0;
    s * (0.5 + 0.5 * c) * impact_weight
}
```

Then select action by expected entropy reduction:

```text
utility(action) =
  Σ kind weight[kind] * expected_reduction(action, kind)
  - action_cost(action)
  - safety_risk(action)
  - user_annoyance(action)
  - cooldown_penalty(action)
```

Do not always choose the highest-scoring entropy. Choose the highest expected reduction per cost subject to hard policy. Example: verification entropy 90 and agent-health entropy 70 should usually trigger `ForceVerification`, not `SwitchAgent`, because verification gives objective information and may lower repo/blame and plan entropy too.

Use hysteresis. Require one of:

```text
dominant_score >= 80
dominant_score - second_score >= 15
same dominant entropy for 2 consecutive windows
hard trigger fired, e.g. code changed after tests
```

This prevents oscillation between “retry,” “switch agent,” and “ask user.”

## 3. Control actions

Define a small, explicit action enum. Everything else is implementation detail.

```rust
enum ControlAction {
    Continue {
        packet: Option<ControlPacket>,
        max_steps: Option<u32>,
    },
    Retry {
        scope: RetryScope,
        packet: ControlPacket,
        max_attempts: u8,
    },
    SwitchAgent {
        target_agent: AgentKind,
        reason: String,
        packet: ControlPacket,
    },
    SpawnFresh {
        target_agent: AgentKind,
        worktree_policy: WorktreePolicy,
        packet: ControlPacket,
    },
    ForceVerification {
        suite: VerificationSuite,
        blocking: bool,
        packet_on_failure: Option<ControlPacket>,
    },
    PreserveMemory {
        candidates: Vec<MemoryCandidate>,
        require_verification: bool,
    },
    AskUser {
        question: UserQuestion,
        default_when_timeout: Option<UserOptionId>,
    },
    Pause {
        reason: String,
    },
    Abort {
        reason: String,
        rollback_plan: Option<RollbackPlan>,
    },
}
```

Allowed action defaults:

```text
Goal entropy high:
  First: synthesize acceptance criteria packet.
  Then: ask user only if unresolved decision has high value of information.

Context entropy high:
  First: send compact case-file packet.
  Then: spawn fresh agent if current session is polluted or overlong.

Repo/blame entropy high:
  First: run deterministic repo probes.
  Then: spawn read-only explorer or plan-mode agent.

Verification entropy high:
  First: force verification.
  Then: retry only with failure-specific packet.

Plan entropy high:
  First: require explicit plan/update.
  Then: switch to plan-capable agent or spawn fresh planner.

Agent-health entropy high:
  First: retry with tight packet if failure is transient.
  Then: restart/switch/spawn fresh if loop signature persists.

User-decision entropy high:
  Ask one bounded question with options and a recommended default.
```

Production guardrails:

```text
No Continue if dirty diff exists and last verifier is older than latest write.
No SwitchAgent solely because tests failed once.
No SpawnFresh into same writable worktree.
No AskUser unless deterministic probes and memory lookup failed.
No PreserveMemory from unverified or reverted work.
No packet may include secrets, raw auth files, private env values, or unredacted logs.
No agent may receive a packet whose preconditions no longer match current HEAD.
```

## 4. Normalized event schema

Your ingestion layer should normalize every adapter into a common event stream.

```json
{
  "event_id": "uuid",
  "project_id": "uuid",
  "run_id": "uuid",
  "agent_session_id": "string",
  "adapter": "codex|claude_code|opencode|pi|custom",
  "adapter_version": "string|null",
  "seq": 1842,
  "observed_at": "2026-06-22T18:20:11.209Z",
  "occurred_at": "2026-06-22T18:20:10.991Z",
  "cwd": "/repo",
  "worktree": "/repo/.worktrees/cam-run-123",
  "git": {
    "head": "sha",
    "branch": "cam/run-123",
    "dirty": true
  },
  "kind": "tool_call.completed",
  "actor": "agent|monitor|user|verifier|system",
  "payload": {},
  "source": {
    "type": "hook|jsonl|logfile|poll|git|verifier",
    "path": "/path/to/source",
    "offset": 99123,
    "hash": "blake3"
  },
  "redaction": {
    "status": "clean|redacted|tainted",
    "rules": ["env_secret", "token_like"]
  }
}
```

Core event kinds:

```text
session.started
session.stopped
session.failed
message.user
message.agent
message.system
tool_call.started
tool_call.completed
tool_call.failed
file.read
file.written
diff.changed
verification.started
verification.completed
verification.failed
service.failed
permission.requested
permission.denied
plan.updated
memory.candidate
memory.persisted
monitor.diagnosed
monitor.action_selected
monitor.packet_sent
monitor.action_result
```

Do not assume every adapter emits all events. Codex and Claude Code give hook-level events; OpenCode gives plugin/session/ACP surfaces; Pi may require wrapper-based observation and filesystem/process monitoring.

## 5. Bounded case-file schema

The case file is the advisor’s entire world. It should be short, source-grounded, and replayable.

```json
{
  "case_file_id": "uuid",
  "run_id": "uuid",
  "built_at": "2026-06-22T18:25:00Z",
  "git_head": "sha",
  "token_budget": 18000,
  "task": {
    "user_goal": "Implement X",
    "acceptance_criteria": [
      {
        "id": "ac1",
        "text": "Tests for Y pass",
        "source_event_id": "uuid",
        "confidence": 0.82
      }
    ],
    "open_questions": []
  },
  "current_state": {
    "active_agent": "codex",
    "permission_mode": "workspace-write",
    "worktree": "/repo/.worktrees/cam-run-123",
    "latest_agent_claim": "Fixed failing parser tests",
    "latest_agent_claim_evidence_id": "uuid"
  },
  "recent_timeline": [
    {
      "event_id": "uuid",
      "summary": "Agent edited src/parser.rs",
      "salience": 0.77
    }
  ],
  "diff_summary": {
    "changed_files": [
      {
        "path": "src/parser.rs",
        "adds": 42,
        "dels": 9,
        "risk": "medium",
        "hunks": ["diff_hunk_id"]
      }
    ],
    "generated_files": [],
    "sensitive_paths_touched": []
  },
  "verification": {
    "last_run_id": "uuid",
    "status": "failed|passed|not_run|stale",
    "commands": ["cargo test"],
    "failure_signatures": [
      {
        "signature": "parser::tests::handles_nested failed",
        "first_seen_event_id": "uuid",
        "last_seen_event_id": "uuid"
      }
    ]
  },
  "agent_health": {
    "loop_signatures": [],
    "tool_failure_rate_last_20": 0.05,
    "repeated_command_count": 1,
    "no_op_patch_count": 0
  },
  "design_memory": [
    {
      "memory_id": "uuid",
      "claim": "Parser must preserve comments for formatter roundtrip.",
      "status": "active",
      "evidence_ids": ["uuid"],
      "last_confirmed_at": "2026-06-21T10:00:00Z"
    }
  ],
  "entropy_features": {
    "goal": {},
    "context": {},
    "repo_blame": {},
    "verification": {},
    "plan": {},
    "agent_health": {},
    "user_decision": {}
  },
  "allowed_actions": [
    "Continue",
    "Retry",
    "ForceVerification",
    "SpawnFresh",
    "AskUser",
    "Pause"
  ],
  "forbidden_actions": [
    {
      "action": "SwitchAgent",
      "reason": "Cooldown active until 2026-06-22T18:45:00Z"
    }
  ]
}
```

Case-file building should use salience, not recency alone. A useful salience formula:

```text
salience =
  0.30 * causal_relevance_to_failure
+ 0.20 * touches_changed_files
+ 0.15 * mentions_acceptance_criteria
+ 0.15 * recency_decay
+ 0.10 * human_or_policy_origin
+ 0.10 * contradiction_or_uncertainty_marker
```

Keep raw logs outside the case file. Store pointers and hashes. The advisor sees excerpts only.

## 6. LLM advisor schema

The advisor should return a strictly validated JSON object. It should never be allowed to invent an action outside `allowed_actions`.

```json
{
  "diagnosis_id": "uuid",
  "dominant_entropy": "verification",
  "entropy_scores": {
    "goal": { "score": 22, "confidence": 78 },
    "context": { "score": 41, "confidence": 67 },
    "repo_blame": { "score": 55, "confidence": 61 },
    "verification": { "score": 91, "confidence": 88 },
    "plan": { "score": 34, "confidence": 70 },
    "agent_health": { "score": 18, "confidence": 74 },
    "user_decision": { "score": 5, "confidence": 81 }
  },
  "top_evidence": [
    {
      "event_id": "uuid",
      "why_it_matters": "Code changed after last passing test run."
    }
  ],
  "missing_evidence": [
    "No targeted test run after parser.rs edit."
  ],
  "proposed_action": {
    "type": "ForceVerification",
    "suite": "targeted",
    "blocking": true
  },
  "expected_entropy_delta": {
    "verification": -55,
    "repo_blame": -15,
    "plan": -10
  },
  "packet_intent": "Tell active agent not to edit further; run targeted verifier and interpret failures.",
  "packet_draft": {
    "urgency": "blocking",
    "summary": "Do not continue implementation until targeted verification completes.",
    "instructions": [
      "Run cargo test parser::tests::handles_nested.",
      "If it fails, report exact failure and do not patch yet."
    ],
    "evidence_refs": ["uuid"]
  },
  "ask_user": null,
  "confidence": 0.84
}
```

The system prompt for the advisor should be restrictive:

```text
You are not the controller. You diagnose uncertainty in a bounded case file.
Return only JSON matching the schema.
You may propose only actions listed in allowed_actions.
Every claim must cite evidence_ids from the case file.
Prefer deterministic verification over agent switching.
Prefer one bounded user question only when evidence cannot resolve the decision.
Do not include hidden reasoning or free-form chain of thought.
```

The advisor should not decide policy. It estimates.

## 7. Policy validator

The validator consumes `CaseFile`, deterministic entropy features, advisor JSON, budgets, and adapter capabilities.

```rust
struct ValidationContext {
    run_id: Uuid,
    git_head: String,
    dirty: bool,
    latest_write_at: Option<DateTime<Utc>>,
    latest_verification_at: Option<DateTime<Utc>>,
    active_agent: AgentKind,
    adapter_caps: AdapterCapabilities,
    budgets: BudgetState,
    cooldowns: Cooldowns,
    user_interrupt_budget: UserInterruptBudget,
    locks: WorktreeLocks,
}

enum ValidationOutcome {
    Approved(ControlAction),
    Modified {
        original: ControlAction,
        replacement: ControlAction,
        reason: String,
    },
    Denied {
        original: ControlAction,
        reason: String,
    },
    EscalateToPause {
        reason: String,
    },
}
```

Validator rules should be explicit and testable:

```text
R1: If latest_write_at > latest_verification_at and proposed action is Continue,
    replace with ForceVerification unless change is docs-only.

R2: If action is AskUser and user_decision_entropy < 70,
    deny unless hard policy requires user consent.

R3: If action is SpawnFresh and no isolated worktree is available,
    replace with Pause.

R4: If action is SwitchAgent and previous switch occurred within cooldown,
    replace with Retry or Pause.

R5: If packet contains redacted/tainted evidence,
    deny packet dispatch and rebuild case file with sanitized excerpts.

R6: If advisor cites evidence not in case file,
    deny advisor output and fall back to deterministic policy.

R7: If action mutates repo and adapter lacks sandbox/permission controls,
    require external worktree/container wrapper.
```

This layer is where “user-annoying actions” are prevented.

## 8. Adapter capability model

Use capabilities, not agent names, as the primary abstraction.

```rust
struct AdapterCapabilities {
    ingest_transcript: bool,
    ingest_jsonl: bool,
    hook_pre_tool: bool,
    hook_post_tool: bool,
    hook_stop: bool,
    can_block_tool: bool,
    can_rewrite_tool_input: bool,
    can_inject_context: bool,
    can_run_headless: bool,
    can_resume_session: bool,
    can_export_session: bool,
    can_start_subagent: bool,
    can_switch_mode: bool,
    supports_readonly_mode: bool,
    supports_workspace_write_mode: bool,
    requires_external_sandbox: bool,
}
```

A Rust adapter trait:

```rust
#[async_trait::async_trait]
trait AgentAdapter: Send + Sync {
    fn kind(&self) -> AgentKind;
    fn capabilities(&self) -> AdapterCapabilities;

    async fn discover_sessions(&self, project: &Project) -> anyhow::Result<Vec<AgentSessionRef>>;

    async fn ingest_events(
        &self,
        session: &AgentSessionRef,
        sink: EventSink,
    ) -> anyhow::Result<()>;

    async fn send_packet(
        &self,
        session: &AgentSessionRef,
        packet: &CompiledPacket,
    ) -> anyhow::Result<DispatchResult>;

    async fn spawn(
        &self,
        request: SpawnRequest,
    ) -> anyhow::Result<AgentSessionRef>;

    async fn stop(
        &self,
        session: &AgentSessionRef,
        reason: &str,
    ) -> anyhow::Result<()>;
}
```

Adapter notes:

Codex adapter: prefer `codex exec --json` for machine-readable subprocess runs and hooks for lifecycle observation. Do not rely on transcript file format as a stable interface; use JSONL events and hook payloads where available. Codex `exec` supports explicit sandbox flags, JSONL event output, and structured final output via schema, which is useful for isolated verifier/explorer runs. ([developers.openai.com][5])

Claude Code adapter: use hooks for urgent policy decisions and headless/print mode or Agent SDK for programmatic runs. Claude hooks can fire before tool execution, after tool success/failure, on stop, and on permission events, and `PreToolUse` can allow, deny, ask, defer, or modify input. ([code.claude.com][2])

OpenCode adapter: use plugin hooks for event capture and command injection; use session export/listing for historical state; use ACP for editor/subprocess control when appropriate. OpenCode’s permission config should be mapped into your policy model rather than bypassed. ([opencode.ai][3])

Pi adapter: treat Pi as a lightweight harness behind an external supervisor. Launch it inside a container, microVM, or restricted process wrapper when mutation is allowed. Because Pi lacks built-in permissions, the monitor must own filesystem, process, network, and credential boundaries. ([GitHub][6])

## 9. Urgent and follow-up packet schemas

Use one internal schema and adapter-specific renderers.

```json
{
  "packet_id": "uuid",
  "run_id": "uuid",
  "urgency": "urgent|follow_up|context|verification|memory",
  "delivery_semantics": "blocking|advisory|next_turn|session_start",
  "preconditions": {
    "git_head": "sha",
    "worktree": "/repo/.worktrees/cam-run-123",
    "agent_session_id": "abc"
  },
  "title": "Verification required before further edits",
  "summary": "The repo changed after the last passing test run.",
  "instructions": [
    {
      "priority": "must",
      "text": "Do not make further code edits before running the targeted verifier."
    },
    {
      "priority": "must",
      "text": "Run: cargo test parser::tests::handles_nested"
    },
    {
      "priority": "should",
      "text": "If the test fails, summarize the exact failure and wait for the monitor packet."
    }
  ],
  "evidence_refs": [
    {
      "evidence_id": "uuid",
      "label": "src/parser.rs changed after last verifier"
    }
  ],
  "forbidden": [
    "Do not broaden the task scope.",
    "Do not edit unrelated files."
  ],
  "success_criteria": [
    "Targeted verifier result recorded.",
    "No additional diff unless verifier failure is understood."
  ]
}
```

Urgent packet properties:

```text
Short.
Imperative.
Single purpose.
Blocking if safety/verification/user-consent issue.
No long history.
No speculative analysis.
```

Follow-up packet properties:

```text
Includes case-file summary.
Includes accepted design memory.
Includes current plan and evidence.
Suitable for fresh agent or resumed session.
```

Example urgent packet rendered for a hook-capable agent:

```text
CAM BLOCKING NOTE

Dominant uncertainty: verification.
Do not continue implementation until verification runs.

Evidence:
- src/parser.rs changed after last passing verifier.
- Last targeted parser test is stale.

Required next step:
Run `cargo test parser::tests::handles_nested`.
If it fails, report exact failure and stop before patching.
```

Example fresh-agent packet:

```text
You are taking over an isolated worktree for a monitored coding run.

Goal:
Implement nested parser behavior while preserving comment roundtrip.

Current evidence:
- Prior agent edited src/parser.rs.
- Targeted test parser::tests::handles_nested failed after the edit.
- Design memory: parser must preserve comments for formatter roundtrip.

Constraints:
- Work only in src/parser.rs and parser tests unless evidence requires otherwise.
- Do not change public API without explaining why.
- Run targeted test first; inspect failure before editing.
- After patching, run targeted test and then cargo test.

Stop condition:
Return a concise report with changed files, verifier results, and unresolved risks.
```

## 10. Persistence model

Use SQLite first. Event-sourced, append-only, with derived tables for speed.

Core tables:

```sql
create table runs (
  run_id text primary key,
  project_id text not null,
  goal text not null,
  status text not null,
  created_at text not null,
  updated_at text not null
);

create table agent_sessions (
  agent_session_id text primary key,
  run_id text not null,
  adapter text not null,
  worktree text not null,
  started_at text not null,
  stopped_at text,
  status text not null
);

create table events (
  event_id text primary key,
  run_id text not null,
  agent_session_id text,
  seq integer not null,
  kind text not null,
  observed_at text not null,
  occurred_at text,
  git_head text,
  payload_json text not null,
  source_json text not null,
  redaction_status text not null
);

create table case_files (
  case_file_id text primary key,
  run_id text not null,
  git_head text not null,
  built_at text not null,
  case_json text not null,
  input_event_high_watermark integer not null
);

create table diagnoses (
  diagnosis_id text primary key,
  case_file_id text not null,
  deterministic_scores_json text not null,
  advisor_scores_json text,
  dominant_entropy text not null,
  created_at text not null
);

create table control_decisions (
  decision_id text primary key,
  diagnosis_id text not null,
  proposed_action_json text,
  final_action_json text not null,
  validation_outcome text not null,
  validator_reason text,
  created_at text not null
);

create table packets (
  packet_id text primary key,
  decision_id text not null,
  adapter text not null,
  internal_packet_json text not null,
  rendered_text text not null,
  dispatch_status text not null,
  dispatched_at text
);

create table verifier_runs (
  verifier_run_id text primary key,
  run_id text not null,
  command text not null,
  status text not null,
  started_at text not null,
  completed_at text,
  exit_code integer,
  output_digest text,
  full_output_ref text
);

create table memories (
  memory_id text primary key,
  project_id text not null,
  scope text not null,
  claim text not null,
  status text not null,
  evidence_json text not null,
  introduced_at text not null,
  last_confirmed_at text,
  invalidation_json text
);
```

Memory needs governance. Treat memory as claims, not truth.

```json
{
  "memory_id": "uuid",
  "scope": "repo|module|file|task",
  "claim": "The parser must preserve comments for formatter roundtrip.",
  "status": "active|deprecated|conflicted|unverified",
  "evidence_ids": ["uuid"],
  "source": "user|verified_result|agent_claim|manual_review",
  "confidence": 0.88,
  "validity_conditions": [
    "Applies to src/parser.rs and formatter tests"
  ],
  "invalidation_triggers": [
    "formatter architecture changed",
    "roundtrip tests removed or replaced"
  ],
  "last_confirmed_at": "2026-06-22T18:00:00Z"
}
```

Do not persist durable design memory from raw agent claims unless confirmed by user, tests, code review, or repeated evidence. Memory poisoning will otherwise become a long-term entropy amplifier.

## 11. Verification architecture

Verification should be first-class, not just another tool call.

Use a verifier registry:

```toml
[verifiers.cargo_test]
command = "cargo test"
scope = "full"
timeout_sec = 900
required_for = ["rust"]

[verifiers.parser_targeted]
command = "cargo test parser::tests::handles_nested"
scope = "targeted"
timeout_sec = 120
paths = ["src/parser.rs", "tests/parser.rs"]

[verifiers.fmt]
command = "cargo fmt --check"
scope = "style"
timeout_sec = 60
```

Track verifier freshness:

```rust
fn verification_is_stale(latest_write: DateTime<Utc>, verifier: &VerifierRun) -> bool {
    verifier.completed_at.map_or(true, |t| t < latest_write)
}
```

Classify verifier failures:

```text
deterministic failure: same signature on rerun
flaky failure: passes on rerun or failure migrates
environment failure: service unavailable, dependency missing, timeout
compile failure: build/typecheck failure
assertion failure: test assertion
coverage gap: no relevant verifier exists
```

Policy examples:

```text
Compile failure after edit:
  Retry same agent once with exact compiler error.
  If same error repeats, spawn fresh agent with failure packet.

Test assertion failure:
  Ask active agent to explain before patching if repo/blame entropy is high.
  Otherwise retry with targeted failure packet.

Environment failure:
  Do not switch agent.
  Run service diagnosis or ask user only if credentials/service ownership required.

No verifier exists:
  Increase verification entropy.
  Ask agent to add or identify targeted verifier before implementation continues.
```

## 12. Control loop

A practical loop:

```rust
loop {
    let event = ingest.next().await?;
    store.append_event(event).await?;

    let state = state_builder.update_from_event(event).await?;

    if !should_evaluate(&state) {
        continue;
    }

    let deterministic = entropy::score(&state).await?;

    let hard_action = hard_policy::maybe_fire(&state, &deterministic)?;
    let action = if let Some(action) = hard_action {
        action
    } else {
        let case = case_builder::build(&state, deterministic.clone()).await?;
        let advisor = advisor_client::diagnose(&case).await?;
        let proposed = advisor.proposed_action;
        validator::validate(proposed, &state, &case)?
    };

    let packet = packet_compiler::compile(&action, &state.adapter_caps)?;
    let dispatch = adapter.send_packet(&state.active_session, &packet).await?;

    store.record_decision(action, packet, dispatch).await?;

    calibration::record_expected_delta(&action).await?;
}
```

Evaluation triggers:

```text
After every tool failure.
After every file write batch.
After every verifier completion.
After stop/idle.
After permission denial.
After N repeated commands.
After context compaction.
After service failure.
After elapsed wall-clock threshold.
After user message.
```

Do not evaluate after every token or message part. That creates supervisory noise.

## 13. Deterministic hard policy layer

Start with hard rules that do not require the LLM.

```rust
enum HardPolicyTrigger {
    DirtyDiffWithStaleVerification,
    DestructiveCommand,
    SecretExposure,
    ConcurrentWriteConflict,
    AgentLoopDetected,
    ServiceUnavailable,
    UserConsentRequired,
    BudgetExceeded,
}
```

Examples:

```text
DirtyDiffWithStaleVerification:
  Trigger when changed source/test files exist and latest verifier predates latest write.
  Action: ForceVerification.

DestructiveCommand:
  Trigger on rm -rf, git reset --hard, git clean, destructive db command, cloud deletion command.
  Action: block tool or ask user, depending on configured allowlist.

SecretExposure:
  Trigger when packet/log/case file includes token-like material, .env, auth.json, private key.
  Action: redact, rebuild case file, optionally pause.

ConcurrentWriteConflict:
  Trigger when two agents attempt writable operation on same file set.
  Action: pause one agent or move to separate worktree.

AgentLoopDetected:
  Trigger when same command or patch signature repeats above threshold.
  Action: retry with loop-breaking packet, then switch/spawn.

BudgetExceeded:
  Trigger on token, wall-clock, spend, action count, or user-interrupt budget.
  Action: pause with summary.
```

Hard policy must be unit-testable with fixture event streams.

## 14. `cam.toml` sketch

```toml
[project]
id = "coding-agent-monitor"
root = "/repo"
default_branch = "main"

[advisor]
provider = "openai_compatible"
base_url = "http://localhost:8080/v1"
model = "gpt-5.5"
timeout_sec = 45
max_input_tokens = 18000
max_output_tokens = 2500
temperature = 0

[policy]
max_user_questions_per_hour = 2
switch_agent_cooldown_min = 20
spawn_fresh_cooldown_min = 10
max_parallel_writable_agents = 1
require_verification_after_source_change = true
allow_docs_only_continue_without_tests = true

[policy.entropy_thresholds]
force_verification = 75
ask_user = 80
spawn_fresh = 78
switch_agent = 82
pause = 90

[security]
redact_env = true
redact_auth_files = true
deny_paths = [".env", ".env.*", "**/auth.json", "**/id_rsa", "**/*.pem"]
protected_paths = ["migrations/**", "infra/**", ".github/workflows/**"]

[worktrees]
root = ".cam/worktrees"
isolate_fresh_agents = true
cleanup = "on_success"

[adapters.codex]
enabled = true
mode = "exec_json_and_hooks"
command = "codex"

[adapters.claude_code]
enabled = true
command = "claude"
use_hooks = true
output_format = "stream-json"

[adapters.opencode]
enabled = true
command = "opencode"
use_acp = true
plugin_dir = ".opencode/plugins"

[adapters.pi]
enabled = true
command = "pi"
requires_external_sandbox = true
sandbox = "docker"

[verifiers.rust_targeted]
command = "cargo test {{test_filter}}"
timeout_sec = 180

[verifiers.rust_full]
command = "cargo test"
timeout_sec = 900
```

## 15. Implementation slices

Build this in thin vertical slices.

Slice 1: event store and git/verifier observation. No LLM. Ingest git status, diff summary, test runs, command results, and session lifecycle. Implement stale-verification detection and basic loop detection.

Slice 2: case-file builder. Produce bounded JSON with evidence pointers. Add redaction. Add deterministic entropy scores.

Slice 3: hard policy engine. Implement `ForceVerification`, `Pause`, and “do not ask user unless required.” This alone will be useful.

Slice 4: advisor client. Use schema-constrained JSON. Add advisor result validation, evidence-id validation, and fallback to deterministic policy.

Slice 5: packet compiler and one adapter. Start with Codex or Claude Code because their hook/headless surfaces are more straightforward for lifecycle control. Then add OpenCode plugin/ACP support. Add Pi last, through an external wrapper.

Slice 6: memory governance. Extract memory candidates, but require verification/user confirmation before durable persistence.

Slice 7: calibration. Compare expected entropy deltas with observed outcomes: did verifier pass, did loop stop, did user question resolve ambiguity, did fresh agent reduce failure rate?

## 16. Production failure modes

The main failure modes are supervisory, not model-theoretic.

Supervisor-induced loops: the monitor sends too many packets, and the agent spends time responding to the monitor rather than working. Mitigation: packet cooldowns, max one urgent packet per turn, and terse imperative packets.

Bad case-file compression: the advisor receives a neat summary that omitted the causal line. Mitigation: source-ground every claim, include failure signatures verbatim, keep top raw excerpts for tests/tool errors.

Adapter drift: CLI event schemas change. Mitigation: normalize through adapter-specific parsers with contract tests and sample fixture replays. Treat transcript formats as unstable unless the adapter documents them as stable.

False verification confidence: tests pass but wrong tests ran. Mitigation: path-to-verifier mapping, acceptance-criteria coverage, and stale-verifier checks by git write timestamp.

Agent ping-pong: the monitor switches agents after ordinary failures. Mitigation: switch only on agent-health entropy, not task difficulty. Normal test failure should trigger targeted retry or verification, not agent replacement.

Memory poisoning: a wrong agent claim becomes durable design memory. Mitigation: memory statuses, provenance, invalidation triggers, and “verified/user-confirmed only” promotion.

User annoyance: the monitor asks questions that logs could answer. Mitigation: value-of-information gate and interrupt budget. Ask one question with options and a recommended default.

Split-brain repo state: multiple agents edit same files or conflicting worktrees. Mitigation: per-file/write locks, isolated worktrees, patch merge queue, and HEAD preconditions on every packet.

Secret leakage: logs and case files include auth files, tokens, or environment dumps. Mitigation: redaction before storage, taint tracking, denylisted paths, and never feeding tainted evidence to the advisor.

Over-trusting the advisor: the LLM proposes plausible but unsafe actions. Mitigation: schema validation, evidence-id validation, deterministic policy override, and no direct tools.

Cost runaway: long-running monitor spawns too many agents or verification jobs. Mitigation: budgets per run, per action, per adapter, and per entropy type.

## 17. The architecture I would ship

The production architecture should be:

```text
camd
  ├─ ingestion
  │   ├─ codex adapter
  │   ├─ claude-code adapter
  │   ├─ opencode adapter
  │   ├─ pi wrapper adapter
  │   └─ git/verifier/service watchers
  │
  ├─ event store
  │   ├─ append-only normalized events
  │   ├─ artifact store for logs/diffs
  │   └─ replay API
  │
  ├─ state builder
  │   ├─ run state
  │   ├─ repo state
  │   ├─ verification state
  │   ├─ agent health state
  │   └─ memory state
  │
  ├─ entropy engine
  │   ├─ deterministic feature scorers
  │   ├─ bounded case-file builder
  │   └─ optional LLM advisor
  │
  ├─ control engine
  │   ├─ hard policies
  │   ├─ action utility scorer
  │   ├─ validator
  │   └─ budgets/cooldowns
  │
  ├─ packet compiler
  │   ├─ internal ControlPacket
  │   ├─ codex renderer
  │   ├─ claude renderer
  │   ├─ opencode renderer
  │   └─ pi renderer
  │
  └─ outcome tracker
      ├─ result attribution
      ├─ entropy delta measurement
      ├─ memory promotion
      └─ calibration
```

The most valuable MVP is not multi-agent switching. It is stale-verification detection plus bounded evidence packets plus loop detection. Once that is solid, add advisor-based entropy diagnosis. Agent switching and fresh spawning should come later, because they are high-cost controls and easy to misuse.

[1]: https://developers.openai.com/codex/cli "CLI – Codex | OpenAI Developers"
[2]: https://code.claude.com/docs/en/hooks "Hooks reference - Claude Code Docs"
[3]: https://opencode.ai/docs/cli/ "CLI | OpenCode"
[4]: https://pi.dev/ "Pi Coding Agent"
[5]: https://developers.openai.com/codex/noninteractive "Non-interactive mode – Codex | OpenAI Developers"
[6]: https://github.com/earendil-works/pi "GitHub - earendil-works/pi: AI agent toolkit: unified LLM API, agent loop, TUI, coding agent CLI · GitHub"
