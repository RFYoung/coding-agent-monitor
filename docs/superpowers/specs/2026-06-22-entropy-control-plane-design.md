# Entropy Control Plane Design

## Goal

Turn Coding Agent Monitor from a passive detector into an event-sourced control
plane for long AI coding runs. The monitor should keep work verifiable,
recoverable, and low-entropy without trusting the active coding agent to judge
its own state.

The first implementation slice focuses on auditable control decisions and
agent-facing packets. It does not attempt live control of Codex, Claude Code,
OpenCode, or Pi sessions yet.

## Core Loop

```text
observe
-> normalize and persist events
-> build bounded case file
-> compute deterministic entropy vector
-> optionally call endpoint advisor
-> validate proposed action against hard policy
-> compile adapter packet
-> persist decision, dispatch result, and packet
-> measure later result
```

The endpoint LLM is not the controller. It estimates uncertainty and drafts a
packet from bounded evidence. Deterministic policy validates or replaces the
proposal.

## Entropy Types

Use entropy as an operational risk score: the chance that the next autonomous
step will waste time, degrade correctness, or require later human repair because
an important uncertainty is unresolved.

- `goal`: done state or acceptance criteria are unclear.
- `context`: the active agent may have lost durable constraints.
- `repo_blame`: changed code lacks clear rationale or ownership.
- `verification`: correctness is unknown or stale after edits.
- `plan`: next action is unclear, contradictory, or circular.
- `agent_health`: the current agent loop is degraded.
- `user_decision`: progress requires user authority or preference.

Each score is 0 to 100 with confidence, causes, evidence ids, missing evidence,
and recommended observations.

## Deterministic MVP Signals

The first slice scores entropy from data already available in the project:

- stale verification: source/test file changes with no later passing command
  result.
- unverified completion: agent claims completion while admitting tests did not
  run.
- premature stop: agent asks whether to continue obvious work.
- lost design memory: agent says it lost or forgot project context.
- telemetry gap: running agent exists but no monitor events exist.
- stale or degraded agent sessions from dashboard snapshots.
- trace gap: file-change event has no rationale.
- user-authority blocker: agent evidence says credentials, destructive or
  external side-effect consent, spending/billing authorization, or an
  irreversible product/preference decision is required.
- destructive command intent: normalized command events or adapter
  `tool.execute.before` records include commands such as `git reset --hard`,
  force-clean/delete operations, database drops, infrastructure destroys, or
  cloud/Kubernetes deletes.

## Case File

The case file is the advisor's whole world. It must be bounded, source-grounded,
and safe to persist.

Required fields:

- case file id and build time.
- workspace, active agents, and replay metadata for git head, branch, dirty
  state, input counts, and max normalized event sequence.
- task summary with latest user goal, source event id, source-grounded
  acceptance criteria, and ambiguity markers.
- dashboard status and recent counts.
- recent evidence items with ids, kind, agent, summary, and source pointer.
- evidence redaction status so advisor-visible case files do not carry obvious
  secrets.
- entropy vector with deterministic scores.
- belief state with bounded failure hypotheses, estimated probability,
  confidence, evidence ids, and missing evidence. This is a diagnostic prior
  for the advisor and control policy, not a permission to bypass deterministic
  validators.
- verification summary with status, changed source files, latest verifier
  commands, and recommended verifier commands.
- unverified memory candidates extracted from design-thought events and
  durable-marker user instructions.
- allowed actions.
- forbidden actions with reasons.
- latest verification status inferred from command events.

The case file should include excerpts only when they are short and sanitized.
Raw logs stay in project storage.

Secret-like evidence summaries are redacted before the case file is built.
Bearer tokens, `sk-` token-looking values, and common key/value secrets are
replaced with `[REDACTED]` and marked `redacted`.
Clean text that contains no token-like material must preserve structured
whitespace so user-instruction blocks, acceptance criteria, and packet excerpts
remain parseable after storage.

## Advisor

Endpoint configuration lives in `.agent-monitor/config.json`.

Environment-variable example:

```json
{
  "advisor": {
    "enabled": true,
    "provider": {
      "kind": "openai_compatible",
      "endpoint": "https://api.example.com/v1/chat/completions",
      "model": "gpt-5",
      "api_key_env": "OPENAI_API_KEY",
      "timeout_secs": 45,
      "max_input_tokens": 18000,
      "max_output_tokens": 2500
    }
  }
}
```

Dedicated coding-plan profile example:

```json
{
  "advisor": {
    "enabled": true,
    "provider": {
      "kind": "openai_compatible",
      "endpoint": "https://api.example.com/v1/chat/completions",
      "model": "gpt-5",
      "credential_source": "coding_plan",
      "credential_file": "credentials/coding-plan/auth.json",
      "timeout_secs": 45,
      "max_input_tokens": 18000,
      "max_output_tokens": 2500
    }
  }
}
```

The monitor stores only the environment variable name or dedicated profile path
in config, never the secret value. `credential_source: "coding_plan"` is the
only non-env advisor credential source. Local Codex and Claude Code configs are
imported as adapter metadata and native-auth capability records; their `.codex`
and `.claude` credential files must not be referenced, read, or copied by
advisor configuration. `agent-monitor config import-coding-plan-credentials`
defaults to `~/.coding-plan/auth.json`, rejects `.codex` and `.claude` source
paths, and materializes the explicit project profile by extracting only a
supported advisor bearer token into
`.agent-monitor/credentials/coding-plan/auth.json`.
When the source profile includes endpoint or model metadata, import uses those
values unless explicit CLI flags override them.
Enabled advisor configs are final-validated before write, so endpoint-only
updates cannot preserve stale `claude_plan` or local CLI auth references.
Provider-specific dedicated plan profiles can still use generic fields such as
`api_key` or `tokens.access_token`.
JWT/OAuth-shaped coding-plan bearer tokens require a configured
provider/proxy endpoint that accepts them. The public `api.openai.com`
OpenAI-compatible endpoint expects API-key-style credentials, so config
write/import rejects that credential/endpoint pairing before persistence. The
advisor client repeats the typed compatibility check before transport and falls
back to deterministic control with a typed advisor error.

OpenAI-compatible endpoints may use `https://`; Windows builds use the OS
WinHTTP stack so production provider configuration does not require shelling
out to a separate command.

Before calling the endpoint, the monitor builds an advisor-visible clone of the
case file bounded by `max_input_tokens`. The stored case file remains complete,
but low-priority evidence is removed from the endpoint request when the prompt
would exceed the configured budget. Advisor output is validated against the
bounded evidence set it actually received. The advisor-visible clone also
redacts memory and verification text, removes raw repo-audit trace records, and
prunes evidence references that no longer point at visible evidence after
budgeting.

Case-file task summaries and acceptance coverage recover user-instruction
intent from the full persisted event log when the originating goal falls out of
the bounded dashboard snapshot, preserving source event ids and avoiding false
missing-goal entropy after long tool-heavy runs.

The advisor response must be strict JSON:

- diagnosis id.
- dominant entropy kind.
- entropy scores with `score` and `confidence` in `0..100`.
- top evidence refs.
- cited evidence ids.
- missing evidence.
- proposed action.
- expected entropy deltas.
- packet intent.
- packet draft.
- optional user question.
- confidence.

If the endpoint is disabled, unavailable, malformed, cites evidence outside the
advisor-visible case file, omits a score for its declared dominant entropy,
gives out-of-range scores or expected entropy deltas, targets an unsupported
adapter, proposes a forbidden action, proposes an unsupported `run_probe` spec,
or emits secret-like text in any persisted diagnostic, proposed action,
ask-user payload, raw response, or packet draft, the monitor falls back to
deterministic policy without persisting the tainted advisor decision.

`Pause` is not advertised as an advisor action. A low-value `ask_user` proposal
is replaced with deterministic control selection instead of pausing the run.
The validator approves `ask_user` only when deterministic user-decision entropy
is high, currently score `>= 80`. In endpoint-disabled deterministic mode, that
same threshold can select one bounded urgent user question after higher-priority
verification, context-recovery, and agent-health controls have been considered.
When an advisor proposes `ask_user`, the validator may accept the action type
only after this same entropy gate, but it rewrites the question to the
monitor-derived bounded authorization question. The endpoint advisor does not
author the final user interrupt text.
Even then, the persisted `policy.max_user_questions_per_hour` budget is checked
against recent `ask_user` advice. If the budget is exhausted, the monitor emits
an internal pause packet instead of interrupting the user again.

## Actions

The first slice supports a small control action set:

- `continue_working`
- `retry_agent`
- `force_verification`
- `run_probe`
- `send_follow_up`
- `spawn_judge_agent`
- `spawn_fresh_agent`
- `switch_agent`
- `ask_user`
- `pause`

`run_probe` is a cheap deterministic observation action for routine questions,
missing reproduction/localization evidence, repeated inspection loops, or other
probe-worthy plan uncertainty. It is denied when probe-worthy entropy is low,
and high verification entropy still forces `force_verification`. The first
monitor-owned executor supports `local_evidence`, `repo_inspection`, and
configured `targeted_test` probes, plus `runtime_validation` for
surface-specific intended-environment evidence. `advise_workspace` executes
monitor-owned probes immediately after persisting the still-current `run_probe`
advice: `local_evidence` records recent monitor-owned events plus repo-audit
observations, `repo_inspection` records read-only repo-audit observations,
`runtime_validation` runs a verifier explicitly mapped to the named runtime
surface or records `unknown` when no mapping exists, and `targeted_test` runs
through the verifier registry only when the probe command exactly matches a
configured verifier. `agent-monitor probe` remains a manual replay path for the
latest probe advice. `local_evidence` never executes arbitrary command text;
`runtime_validation` only executes verifier-registry commands whose
`acceptance_patterns` include the exact marker
`runtime_validation:<surface>`, such as `runtime_validation:mobile_app`.

Runtime-validation classification is surface-specific:

| Surface | Positive command/path signals | Not enough by itself |
| --- | --- | --- |
| `web_ui` | Playwright, Cypress, Puppeteer, browser, route/console/web validation, web/frontend/UI paths | bare `e2e`, `smoke`, or `integration` |
| `mobile_app` | Appium, Detox, Maestro, emulator/simulator/device tests, Android/iOS/Flutter/React Native paths | generic UI smoke without mobile signal |
| `native_gui` | GUI/desktop/native/Tauri/Wails plus smoke/e2e/screenshot evidence | bare desktop build or bare `e2e` |
| `system_component` | healthcheck, systemd, docker compose, service/container/daemon plus smoke/integration evidence | bare `integration` or `smoke` |
| `ml_system` | eval, benchmark, golden data, MLflow, inference smoke, dataset check, model/training/evals paths | generic test pass without model/data signal |

Generic runtime words such as `e2e`, `smoke`, `end-to-end`, and `integration`
raise confidence only when paired with a runtime surface signal or a configured
verifier mapping. They do not prove browser, mobile, native GUI, system, or ML
validation by themselves.
Probe execution writes `probe-runs.jsonl` and attaches a `run_probe` outcome
only when the latest advice is still the matching probe. Dashboard snapshots
surface recent probe runs as bounded `kind:probe-run` capture rows, and control
case files include them as `ProbeRun` evidence with `source_type: probe`. The
native dashboard renders a compact probe card above the raw row JSON.

`spawn_judge_agent` is read-only and only available when repo/blame entropy is
high after verification is current.

`pause` is an internal validator fallback and is not advisor-proposable. Future
versions can add richer retry scopes, memory preservation, worktree policies,
and abort/rollback actions.

## Policy Validator

The validator is deterministic and unit-tested.

Initial rules:

- No progress action such as `continue_working`, `run_probe`, or
  `send_follow_up` when
  source/test changes appear newer than passing verification, unless
  `policy.require_verification_after_source_change` is disabled. The
  stale-verification scorer, verification status, and recommended verifier list
  all use the same policy.
- No `run_probe` when probe-worthy entropy is low; the action is for recording
  monitor-owned local evidence that answers a specific blocker, not for generic
  continuation.
- Git repo-audit dirty source/test changes are also treated as
  verification-relevant writes when adapter `file_change` events are missing.
  They feed verification status, targeted verifier recommendations, and stale
  verification entropy. The repo-audit change timestamp is compared against
  the latest passing verifier; a later passing verifier clears verification
  entropy, but repo/blame entropy can still require trace rationale for dirty
  hunks.
- Documentation-only changes are exempt from stale-verification entropy when
  `policy.allow_docs_only_continue_without_tests` is true. If that exemption is
  disabled, documentation changes are treated as verification-relevant.
- Completion claims are verification-relevant. If an agent says the
  implementation, task, or work is complete, the monitor requires an objective
  passing verifier event at or after that claim. Agent prose that says tests
  passed is evidence of the claim, not proof of verification.
- No `ask_user` unless user-decision entropy is high or hard consent is needed.
- No advisor- or agent-authored free-form `ask_user` text is preserved; approved
  user interrupts are normalized to the monitor-derived bounded question.
- No repeated `ask_user` beyond `policy.max_user_questions_per_hour`; the
  budget uses persisted advice and case-file timestamps for the last hour, and
  over-budget asks become internal pause packets.
- User-decision entropy is raised only for evidence of credentials, destructive
  or external side effects, spending/billing authority, or irreversible
  product/preference choices. Ordinary "should I continue?" language stays in
  plan entropy and does not authorize a user interrupt.
- Command fields are classified separately from prose. Destructive command
  intent raises user-decision entropy even when the event content is only a
  tool-call wrapper, while ordinary read-only commands do not.
- Security path fields are also classified separately from prose.
  `.agent-monitor/config.json` supports `security.redact_env`,
  `security.redact_auth_files`, `security.deny_paths`, and
  `security.protected_paths`. When enabled, `redact_env` and
  `redact_auth_files` classify env/auth filenames implicitly; `deny_paths`
  adds project-specific denied patterns. File changes matching implicit,
  deny, or protected path patterns raise user-decision entropy with the
  file-change event as evidence, so credentials, auth files, infra, migrations,
  and CI workflow edits require explicit authorization even if the active agent
  does not describe the risk.
- No advisor evidence id may be absent from the case file.
- No advisor `dominant_entropy` may be absent from `entropy_scores`.
- No advisor expected entropy delta may fall outside the operational `-100..100`
  entropy scale.
- No advisor response may include more than one expected entropy delta for the
  same entropy kind.
- No explicit advisor target may name an unknown, disabled, or packet-injection
  incapable adapter.
- No packet may include tainted or secret-like content.
- Advisor taint screening runs before validation errors that echo
  advisor-supplied identifiers, so unknown evidence ids or target names cannot
  leak secret-like strings into persisted `advisor_error` fields.
- Repeated non-verification command failures raise agent-health entropy and
  select a loop-breaking `retry_agent` packet targeted at the unhealthy agent.
  A later successful command clears that agent's command-loop signature.
- `retry_agent` is available only when agent-health entropy reaches the retry
  threshold. Low-entropy retry proposals are rewritten to deterministic
  control selection, and advisor-visible case files remove `retry_agent` from
  `allowed_actions` until the threshold is met.
- `send_follow_up` is available only when plan, repo/blame, or context entropy
  is high enough to justify another monitor packet. Low-entropy follow-up
  proposals are rewritten to deterministic control selection, and
  advisor-visible case files remove `send_follow_up` until one of those
  uncertainty types reaches its threshold.
- Repeated service failures raise severe agent-health entropy and select a
  fallback `switch_agent` action that avoids switching to the same failed
  agent. A later healthy message or successful command result clears the
  service-failure streak and resolves prior service-failure intervention
  penalties for snapshot health.
- Retry attempts are clamped to a small safe range and rendered into the packet.
- Advisor-proposed retry/switch actions are normalized against agent-health
  evidence, overriding wrong explicit retry targets and preventing switches to
  the failed agent.
- `switch_agent` and `spawn_fresh_agent` honor
  `policy.switch_agent_cooldown_min` and `policy.spawn_fresh_cooldown_min`.
  The cooldown check uses persisted advice and case-file timestamps, runs
  before worktree-lock acquisition, and converts active-cooldown handoffs into
  internal pause packets so the monitor does not ping-pong agents.
- Writable handoff targets are validated against effective adapter
  capabilities. Effective capabilities start from the built-in Codex, Claude
  Code, OpenCode, and Pi profiles, then apply `.agent-monitor/config.json`
  adapter overrides, including the adapter `enabled` flag. A disabled target,
  a target that requires an external sandbox, or a target that lacks
  workspace-write support is replaced by a safe fallback or paused if no safe
  fallback exists. Explicit CLI handoff to such a target fails before writing a
  packet or acquiring a worktree lock. This means Pi needs a project config
  override that represents an enabled monitor-owned wrapper before it can
  receive a writable fresh-agent handoff.
- If no enabled workspace-write adapter can receive a writable handoff, the
  case file removes `spawn_fresh_agent` and `switch_agent` from
  `allowed_actions` and records matching `forbidden_actions` reasons. This
  keeps endpoint-advisor proposals inside the same configured availability
  boundary the validator later enforces.
- Before an endpoint-advisor request is sent, persisted policy history is also
  applied to the case file: low user-decision entropy or exhausted
  user-interrupt budget removes `ask_user`, high verification entropy removes
  progress and handoff actions such as `continue_working`, `run_probe`,
  `send_follow_up`, `spawn_judge_agent`, `switch_agent`, and
  `spawn_fresh_agent`, and active `switch_agent` / `spawn_fresh_agent`
  cooldowns remove those handoff actions from `allowed_actions` with explicit
  forbidden reasons. The validator still enforces the same rules after advisor
  output.
- High-cost handoffs are also entropy-gated. `switch_agent` remains advisor-
  visible only when agent-health entropy is severe enough to justify replacing
  the active session. `spawn_fresh_agent` remains advisor-visible only when
  context entropy requires a fresh case-file handoff or agent-health entropy is
  severe enough to justify restart. The validator applies the same gate after
  any unsafe-target rewrite, so converting a bad target into a safe target does
  not bypass the high-cost control boundary. High verification entropy takes
  precedence over both handoff gates and rewrites proposed writable handoffs to
  `force_verification` before any worktree lock can be acquired.
- `policy.max_parallel_writable_agents` is enforced before writable handoffs.
  When capacity is exhausted, advisor-visible case files remove
  `spawn_fresh_agent` and `switch_agent`, advisory handoffs become pause
  packets, and explicit CLI handoffs fail without writing the target packet.
- Recent normalized `file_change` evidence marks an already-running active
  writer for the worktree. When the active writer is a different agent than the
  proposed handoff target, advisor-visible writable handoffs are removed and
  proposed handoffs become pause packets before the monitor acquires a lock.
- Adapter JSONL ingestion also loads workspace adapter config and rejects a
  disabled adapter before normalizing or persisting any incoming events.
- Workspace-backed normalized JSONL and adapter JSONL loops filter unknown,
  disabled, sandbox-required, and non-workspace-write adapters out of
  configured fallback switch targets before service-failure interventions are
  emitted.
- If filtering or config leaves no concrete fallback target, legacy
  service-failure handling emits a same-agent retry intervention instead of a
  targetless `switch_agent`.
- Writable handoffs acquire a project worktree lock before dispatch. If a
  `spawn_fresh_agent` or `switch_agent` advice conflicts with an existing
  writable owner, the monitor records the conflict and emits a pause packet
  instead of giving a second agent the same worktree. Explicit CLI handoffs fail
  without writing the target packet when the worktree is already locked.
- Verification packets also carry trace-rationale instructions when repo/blame
  entropy is high.
- If advisor output is invalid, fall back to deterministic action selection.
- Persist the validation outcome so later blame can see whether a proposed
  action was approved, modified, or denied.

## Verifiers

Project config supports a verifier registry:

```json
{
  "verifiers": [
    {
      "id": "parser_targeted",
      "command": "cargo test parser::tests::handles_nested",
      "scope": "targeted",
      "timeout_secs": 120,
      "paths": ["src/parser.rs", "tests/parser.rs"]
    }
  ]
}
```

When changed source/test files match verifier paths, the case file recommends
those verifier commands and verification packets render them explicitly.

The CLI can also execute a configured verifier directly:

```powershell
agent-monitor verify --workspace=<path> --verifier=<id>
```

Verifier execution enforces the configured `timeout_secs` value and records
command, status, exit code, timestamps, and output digest in
`.agent-monitor/verifier-runs.jsonl`. Timed-out verifier process trees are
killed and recorded with `status: "timed_out"` and no exit code. This makes
verification a first-class monitor event source instead of only inferring it
from generic command output.

Dashboard snapshots replay `verifier-runs.jsonl` alongside normalized events and
interventions. Verifier runs appear as capture rows, are included as case-file
evidence, and participate in verification freshness scoring. A passing verifier
run can clear stale-verification entropy when it is newer than the latest source
write; a failed or timed-out verifier run raises verification entropy until a
later passing verifier result supersedes it.

On Windows, verifier processes are created with an atomic job-list startup
attribute so descendants are in the monitor-owned job from process creation. On
Unix, verifier shells run in their own process group. Output collection is also
bounded by the verifier timeout so a background descendant cannot keep inherited
pipes open indefinitely.

## Adapter Capabilities

Adapters are described by capabilities rather than agent names. Codex, Claude
Code, OpenCode, and Pi expose different control surfaces for JSONL ingestion,
hooks, blocking, context injection, session export, headless runs, and sandbox
requirements. Pi is marked as requiring an external sandbox. Case files include
the effective capability map visible to policy and the endpoint advisor. The
map includes whether each adapter is enabled, so advisor proposals and
validator fallback selection share the same configured availability boundary.
When that map contains no safe writable handoff target, the case file prunes
handoff action kinds from `allowed_actions` before advisor validation.

## Memory Governance

Design-thought events become agent-claim memory candidates with provenance,
evidence ids, source, confidence, and `unverified` status. User instructions
become memory candidates only when they use durable markers such as
`remember:`, `constraint:`, `preference:`, `do not`, or `never`; generic user
requests are not copied into design memory. Agent claims are not promoted to
durable active memory by default.

Durable memory records are stored in `.agent-monitor/memories.jsonl`. Case files
load only active memories sourced from the user, a verified result, or manual
review; active agent-claim memories remain excluded until promoted by a trusted
source. Secret-like memory claims are filtered before case files and handoff
packets are built. Handoff packets prefer active durable memory and label
design-thought memories as unverified candidates rather than facts.
The operator promotion path is `agent-monitor memory promote --memory-id=<id>`.
It can promote only a current case-file memory candidate, writes the promoted
record as active durable memory, and accepts only trusted sources:
`manual_review`, `user`, or `verified_result`. `agent_claim`, missing
candidates, already governed memory ids, and secret/token-like claims or
evidence ids are rejected before any memory record is written. A deprecated or conflicted
memory id cannot be silently resurrected by re-running promotion; that requires
a separate future governance operation.
The memory log is append-only: the latest record for a `memory_id` supersedes
earlier records, so deprecated or conflicted records retract prior active
claims. Malformed memory records are skipped with warning evidence in the case
file instead of erasing the whole durable-memory set. Bounded packets keep the
newest active trusted memories first.

## Trace Blame

The first repository-aware blame slice stays on JSONL storage. File-change
trace entries preserve event id, provider, model, session, related event ids,
file, line, and rationale when those fields are present on the normalized
event. `agent-monitor blame --workspace=<path> --file=<path> [--line=<n>]`
reads `.agent-monitor/trace.jsonl`, normalizes relative or workspace-absolute
paths, collapses dot segments, ranks exact line matches before file-level
matches, and returns newest matching trace evidence first.
When `--workspace` is relative, blame path matching resolves it against the
current directory for prefix stripping while still reading from the requested
workspace path.
This does not replace future git-hunk indexing, but it gives the monitor an
immediate answer for "who touched this file or line, and what rationale was
recorded?"

## Repo Diff Audit

`agent-monitor repo-audit --workspace=<path>` observes the current git working
tree and compares changed file hunks against `.agent-monitor/trace.jsonl`.
Tracked modified, added, and deleted files use `git diff --unified=0` to
capture compact hunk ranges. Untracked files are reported as changes but do not
have hunk ranges yet. Monitor-owned storage such as `.agent-monitor/` and build
output such as `target/` are ignored.

Each changed file is classified from its dirty hunks as:

- `traced`: every hunk has matching trace evidence with a rationale.
- `missing_rationale`: every hunk has trace evidence, but at least one hunk has
  no matching rationale.
- `untraced`: at least one hunk has no matching trace evidence.

For tracked dirty files, timestamped trace evidence must be fresh relative to
the changed file's modified time. For deleted files, it must be fresh relative
to the deleted path's nearest existing ancestor directory modified time when
available, falling back to the current HEAD commit time. Untimestamped or
unparsable trace entries are not allowed to justify a dirty hunk when a
freshness threshold is available. Stored matching trace excerpts are capped per
changed file so case files remain bounded even when the trace log is long.

The report returns `clean` only when every audited change is traced with
rationale. This gives the control loop an immediate repo/blame entropy signal:
dirty code without monitor trace evidence should not be treated as fully
recoverable work.

When a git audit succeeds, control case files embed the bounded repo-audit
report and add one evidence item per untraced or unexplained changed file. The
deterministic entropy scorer raises `repo_blame` when dirty hunks lack trace
evidence or rationale. If verification and context entropy do not dominate, the
deterministic action becomes a follow-up packet requiring trace rationale or a
revert of unjustified hunks before new code edits continue.

## Packet Compiler

Use one internal `ControlPacket` schema and render it per target agent. The
first slice writes Markdown packets to:

```text
.agent-monitor/outbox/<agent>/<timestamp>-<urgency>.md
```

Packets are short, imperative, evidence-grounded, and single-purpose. They have
preconditions, instructions, evidence refs, forbidden actions, and success
criteria.

Packet preconditions include the target adapter, latest target-agent run id,
latest target-agent session id, workspace/worktree path, and current Git HEAD
when those values are available. Dispatch validates those preconditions before
writing the outbox artifact so stale packets cannot silently cross repo, run,
or session states.

Renderer output is action-first for every adapter: heading, target agent,
action, urgency, objective, instructions, evidence, forbidden actions, success
criteria, and stale-precondition handling. Adapter transport details stay in
adapter docs and hooks rather than consuming the live packet prompt budget.

Adapter JSONL ingestion is exposed through `agent-monitor ingest`. It reads
bounded adapter records from stdin, normalizes them into the common `Event`
schema, appends them to `.agent-monitor/events.jsonl`, persists derived design
and trace records when applicable, and runs the same intervention loop used by
normalized event input. In store-backed runs, events that trigger bounded
case-file advice suppress legacy stdout/intervention-log control output for
that same event; live control is the validated outbox packet, not a competing
legacy `continue_working` or retry instruction. Non-store JSONL streams keep
legacy stdout interventions for simple pipeline use. The first parser supports
normalized pass-through events, Codex-style agent messages, Claude Code stream
messages, OpenCode `tool.execute.*` records, and generic command/tool result
records. Claude
`PreToolUse` records with destructive shell commands are normalized as
permission-request intervention evidence even when the hook payload has not
already decided to ask or deny, so the control loop sees the authorization
blocker before ordinary tool-call handling. The adapter layer also exposes a
typed `adapter_hook_response` helper for live pre-tool hook handlers: it returns
`allow` for ordinary tools, `ask` for explicit permission-request hook
decisions, and `block` for explicit denials, destructive commands, mutating
writes to configured security deny/protected paths, or a latest matching
read-only judge outbox packet that forbids worktree mutation for the same
adapter/session. `agent-monitor hook-response
--adapter=<agent>` reads one hook JSON object from stdin and emits that typed
response, so Claude Code or OpenCode hook scripts can call the deterministic
policy boundary directly. The command checks effective adapter capabilities
from project config before rendering and refuses disabled adapters or adapters
without pre-tool blocking support. The optional native renderers include
`--format=codex`, `--format=claude-code`, and `--format=opencode`.
Claude Code renders native `PreToolUse` ask or denial JSON, Codex renders
native denial JSON for non-allow decisions, and OpenCode renders
plugin-oriented block JSON with `action`, `decision`, `message`, and `reason`
fields. Native renderers stay silent on allow, matching hook contracts where
silence leaves normal tool handling untouched.
CLI/headless streams still use normalized events, and future adapter-specific
renderers should stay behind the same typed response boundary.
Malformed adapter JSONL lines are skipped with a warning event so one bad tool
output cannot halt a long stream. Unsupported typed records are recorded as
safe warning events that name simple adapter event types and hash anything
outside the constrained event-name alphabet, so they are observable without
copying raw agent text or triggering control-policy heuristics.
The ingest command accepts the same retry and fallback policy knobs as
normalized JSONL monitoring, rejects empty fallback lists, and uses the
configured recovery policy for adapter streams.

Explicit handoff packets are generated by `agent-monitor handoff`. They force a
fresh-agent context packet for a selected adapter and include durable memory
candidates, recent traced changes, current verification status, blame-query
guidance, evidence refs, and workspace/Git preconditions. This gives an operator
or supervisor a deterministic way to rehydrate Codex, Claude Code, OpenCode, or
Pi without waiting for entropy policy to choose a spawn action.

## Dispatch And Outcomes

The first delivery mechanism is outbox dispatch. Writing a packet records a
separate dispatch result so the monitor can distinguish packet compilation from
delivery. Action outcomes are stored separately with expected and observed
entropy deltas for later calibration.

## Worktree Locks

The monitor uses exclusive lock files under
`.agent-monitor/locks/worktrees/` to prevent multiple writable owners from
controlling the same worktree. Lock acquire, conflict, and release events are
also appended to `locks.jsonl` for replay and blame.

By default, locks are conservative and remain until explicitly released by the
monitor. Projects that want bounded stale-lock recovery can set
`policy.worktree_lock_stale_after_secs` in `.agent-monitor/config.json`.
When configured, `advise` and `handoff` expire locks older than that threshold
before attempting a writable handoff, append an `expired` lock event, and then
try the normal exclusive acquire path. This avoids silent double ownership while
still giving long-running supervised work a configured stale-lock escape hatch.

## CLI

Add:

```powershell
agent-monitor advise --workspace=<path>
agent-monitor trail --workspace=<path>
agent-monitor verify --workspace=<path> --verifier=<id>
agent-monitor handoff --workspace=<path> --agent=<agent>
agent-monitor repo-audit --workspace=<path>
agent-monitor ingest --workspace=<path> --adapter=<agent> [--session=<id>] [--retry-limit=<n>] [--fallbacks=a,b]
```

Behavior:

1. Load project store and config.
2. Load dashboard snapshot.
3. Build deterministic entropy vector and bounded case file.
4. Call endpoint advisor only if enabled and configured.
5. Validate advisor result or fall back to deterministic selection.
6. Compile packet.
7. Persist advice record and packet.
8. Print final decision JSON.

`trail` loads the event-sourced control records and prints joined decision
trails: case file, advice, packet, dispatch result, and any recorded outcomes.
This is the first replay API for answering why a packet was sent and what
result evidence later attached to it.
Dashboard snapshots expose the same joined trail as bounded
`kind:decision-trail` capture rows with action, target agent, packet id,
dispatch status, outcome count, and full detail payloads for inspection. The
native dashboard renders a compact control-chain card above the raw row JSON.

`verify` loads the project verifier registry, runs the selected verifier in the
workspace with its configured timeout, appends a verifier-run record, and prints
the persisted run JSON.

`handoff` builds the same bounded case file used by advice, compiles a
fresh-agent packet for the requested adapter, validates and writes it to the
outbox, appends packet and dispatch records, then appends the case file only
after dispatch succeeds. It prints the handoff JSON. This ordering prevents a
tainted or stale packet failure from leaving a misleading case-file record.

`repo-audit` reads live git status and diff hunks, matches them against trace
entries, and prints a bounded JSON report with warning counts for untraced and
unexplained changes.

`ingest` reads adapter JSONL from stdin, normalizes each recognized record into
the shared event schema, persists the event stream, and emits JSONL
interventions on stdout. Malformed lines and unsupported typed records become
warning events and do not stop later records from being processed.

## Persistence

Keep the current JSONL approach for this slice:

- `events.jsonl`
- `interventions.jsonl`
- `design.jsonl`
- `trace.jsonl`
- new `advice.jsonl`
- new `case-files.jsonl`
- new `packets.jsonl`
- new `dispatch.jsonl`
- new `outcomes.jsonl`
- new `locks.jsonl`
- new `verifier-runs.jsonl`
- outbox Markdown packet files

Advice records include the final action and the validation outcome so the
decision trail remains inspectable. Dispatch records and outcome records extend
that trail through delivery and result measurement.

Replay should tolerate older JSONL records when a field was added after the
record was written. Safe defaults are allowed only for derived metadata such as
redaction status, verification summary, memory candidates, case-file replay
metadata, validation outcome,
or inline dispatch copies. Raw evidence, ids, actions, packets, and dispatch
logs should still be required.

Replay readers should also tolerate an incomplete trailing JSONL line from a
concurrent append. Only EOF-style parse errors on the final non-empty line may
be skipped, and only when that final line is not newline-terminated;
newline-terminated malformed records are completed history and must still fail
loudly. Metadata counters that feed dashboard summaries follow the same
completed-record boundary, so partial trailing design or trace records do not
inflate counts during concurrent appends.
Case-file replay metadata records the same completed-input boundary for
normalized events, interventions, verifier/probe runs, repo hunk history,
requirements, dev-history reports, advice records, packets, dispatches, action
outcomes, and worktree lock events. This makes a bad supervisory choice
replayable from the persisted side logs instead of inferred from a summary.

SQLite remains the later repository-aware and replay-friendly store.

## Tests

Add focused tests before implementation:

- config loads endpoint provider without reading the secret.
- entropy vector flags stale verification after file changes.
- case file includes evidence ids and allowed actions.
- advisor validation rejects unknown evidence ids.
- policy validator replaces unsafe continue with force verification.
- detailed validation outcome records modified action decisions.
- verifier registry recommends targeted verification for changed paths.
- design thoughts and durable-marker user instructions become unverified memory
  candidates.
- governed `.agent-monitor/memories.jsonl` records provide active durable
  memory only from trusted sources; agent claims remain candidates.
- memory promotion tests prove a current candidate can be promoted through a
  trusted operator source, while `agent_claim`, missing candidates, and
  secret/token-like claims or evidence ids are rejected before persistence.
  Regression tests also prevent re-promotion of already governed memory ids.
- durable memory loading uses latest-record-per-id governance, warning evidence
  for malformed lines, and newest-first packet truncation.
- durable memory loading quarantines clear cross-id polarity conflicts before
  the packet cap, emits `memory_conflict` evidence that cites both memory ids,
  and excludes both conflicting claims from case files, requirement nodes, and
  handoff packets; memory promotion rejects a new trusted candidate when it
  conflicts with an existing active trusted durable memory under another id.
- active trusted durable memory is mirrored into requirement graph nodes and
  proof steps with `source: durable_memory`, so requirement queries distinguish
  design constraints from acceptance criteria.
- requirement graph nodes carry typed evidence refs for requirement source,
  verification result, durable-memory source, and supporting evidence, with
  necessary/correlated attribution. Proof refs matched only through shared
  source events are marked correlated and do not earn direct trace, repo hunk,
  control-decision, or successful-outcome strength signals.
- trace records and normalized events may cite `requirement_ids`; matching trace
  proof refs are treated as necessary proof for those requirements even when
  the same trace also carries only correlated source-event evidence.
- control rationale and action outcomes may also cite `requirement_ids`; matching
  control and outcome proof refs are treated as necessary proof and expose the
  matched requirement ids in requirement graph reports.
- native dashboard requirement-row details render a structured proof trail above
  the raw payload, with bounded latest proof steps, proof score, verifier status,
  evidence-reference counts, gaps, and hidden older-step count.
- adapter capabilities reflect Codex, Claude Code, OpenCode, and Pi control
  surfaces.
- dispatch records prove packet delivery to the outbox.
- action outcome records store expected and observed entropy deltas.
- spawn-judge action outcomes require meaningful judge/review disposition
  content from the dispatched read-only judge target; lifecycle-only events do
  not satisfy the outcome, and judge-side file-change or repo-diff events fail
  the read-only judge outcome.
- verifier runs automatically record action outcomes only for the current
  matching `force_verification` advice whose packet is still the latest
  dispatch, including pass/fail status and measured before/after verification
  entropy delta.
- JSONL, adapter, and wrapped-command event flows record retry-agent action
  outcomes when the target agent later succeeds with the same failing command
  signature or repeats it, including expected and observed entropy deltas for
  every expected kind plus agent health.
- worktree lock and active-writer tests prevent two writable owners from
  silently sharing one worktree.
- evidence redaction tests prevent secret-like strings from reaching case-file
  summaries.
- advisor endpoint tests accept a valid OpenAI-compatible response, persist the
  typed diagnosis fields, and prove the request includes the schema contract.
- advisor request tests prove `max_input_tokens` bounds low-priority case-file
  evidence before the endpoint call.
- advisor request tests prove memory candidates are redacted, raw repo-audit
  trace records are removed, and budget pruning does not leave dangling
  entropy evidence references.
- advisor validation rejects unknown top-evidence refs, unknown packet evidence
  refs, missing dominant-entropy scores, out-of-range entropy estimates or
  expected deltas, duplicate expected delta kinds, and unsupported explicit
  adapter targets before policy sees the proposal.
- advisor validation rejects secret-like endpoint output in non-packet
  diagnostics, ask-user payloads, action text, or raw response content, and the
  endpoint fallback path proves those tainted fields are not persisted.
- advisor validation rejects secret-like evidence ids and target-agent strings
  before emitting id-bearing validation errors, and fallback tests prove those
  identifiers are not persisted.
- advisor validation rejects `pause`, and policy validation replaces low-value
  `ask_user` proposals with deterministic action selection instead of stopping
  the run.
- policy validation rewrites high-entropy but non-canonical `ask_user`
  proposals to the monitor-authored bounded authorization question.
- deterministic user-decision entropy approves bounded `ask_user` packets only
  for credentials, destructive/external side effects, spending/billing, or
  irreversible product/preference blockers.
- advisor request tests prove `ask_user` is removed from advisor-visible
  `allowed_actions` when user-decision entropy is below the interrupt threshold,
  while high user-decision entropy still leaves it available.
- destructive command-intent tests raise user-decision entropy from command
  fields, convert destructive Claude pre-tool shell records into permission
  requests, and keep ordinary read-only commands below the ask-user gate.
- security-path tests load deny/protected path config, raise user-decision
  entropy from file-change paths, cover nested env files and normalized
  dot-segment paths, verify `redact_env=false` disables implicit env
  classification, and select a bounded ask-user action for protected
  infrastructure changes.
- advisor fallback tests prove invalid endpoint output records an advisor error
  and falls back to deterministic action selection.
- decision trail replay can use inline advice packet and dispatch data when
  sidecar packet/dispatch logs are missing.
- packet compiler writes an outbox file under `.agent-monitor/outbox`.
- packet compiler renders action-first packets: adapter heading, target agent,
  action, urgency, objective, instructions, evidence, forbidden actions,
  success criteria, and a stale-precondition warning. It does not spend prompt
  budget on delivery-plumbing narration or internal controller labels when a
  concrete condition, required action, and evidence checkpoint can be named.
- adapter JSONL ingestion normalizes Codex, Claude Code, OpenCode, Pi/generic
  records into monitor events and emits interventions from the same control
  loop.
- normalized event kinds include model messages, design thoughts, file
  changes, command output/results, tool calls/results, test results, repo
  diffs, user instructions, handoff summaries, agent-health records,
  verification claims, and intervention results; `test_result` participates in
  verification freshness and entropy.
- `repo_diff` participates in trace persistence, source-change freshness,
  packet trace summaries, and repo/blame entropy when rationale is missing.
- adapter ingestion skips malformed lines with warning evidence, records
  unsupported typed records as safe warning events, rejects empty fallback
  lists, and honors configured retry/fallback policy.
- packet compiler rejects secret-like packet content before writing an outbox
  file or packet log entry.
- generated advice packets include adapter, run, session, workspace, and Git
  HEAD preconditions when available, and dispatch rejects stale preconditions.
- `agent-monitor wrap` treats the launched agent as a writable worktree owner:
  it acquires the same worktree lock used by handoff before spawning the child,
  rejects launch on lock conflict, records acquire/conflict/release events, and
  releases the lock after the child exits or the wrapper returns an error.
- advisor and judge prompts are bounded contracts: advisor output is JSON only,
  evidence-cited, case-file text is treated as untrusted data, and proposals
  are constrained to exact allowed action names; external judge output is one
  line with decision, concrete evidence, and a short risk reason.
- verification policy tests cover disabling stale verification after source
  changes and toggling the docs-only exemption.
- intended-environment validation distinguishes product/runtime surfaces from
  generic build verification: web UI uses browser/Playwright/Cypress-style
  checks, mobile uses simulator/device/Appium/Detox/Maestro-style checks,
  native GUI uses desktop smoke/e2e checks, system components use
  service/container/healthcheck/integration checks, and ML systems use
  eval/benchmark/golden/inference-smoke evidence.
- verification status and entropy both treat untimestamped
  verification-relevant writes conservatively as stale, so legacy adapter
  events cannot report `Passed` while entropy demands verification.
- repo-audit verification tests prove dirty source hunks discovered only from
  Git raise stale-verification entropy and force verification, feed targeted
  verifier recommendations, and make verification status stale. A later passing
  verifier run clears verification entropy and leaves the trace follow-up.
  Same-second repo-audit write/verifier ties fail closed and require another
  verifier.
- policy validation tests prove high verification entropy replaces
  `continue_working`, `run_probe`, `send_follow_up`, `spawn_judge_agent`,
  `switch_agent`, and `spawn_fresh_agent` actions with `force_verification`.
- advisor request tests prove high verification entropy removes progress
  and handoff actions from advisor-visible `allowed_actions`, and endpoint
  responses that ignore the pruned action set fall back to deterministic
  `force_verification`.
- policy validation and advisor request tests prove low-entropy high-cost
  handoffs are rewritten or pruned, while context-loss and severe agent-health
  fixtures still allow the corresponding fresh-agent or switch controls.
- writable handoff capacity tests cover advisor-visible pruning, advisory pause
  fallback, and explicit handoff failure when
  `policy.max_parallel_writable_agents` is exhausted.
- decision trail replay ignores incomplete trailing JSONL records caused by
  concurrent appends.
- dashboard snapshots replay recent decision trails as `kind:decision-trail`
  capture rows with action, target agent, packet id, dispatch status, outcome
  count, full detail payload, and display-filter support.
- CLI parses `advise --workspace=<path>`.
- CLI parses `verify --workspace=<path> --verifier=<id>`.
- CLI parses `handoff --workspace=<path> --agent=<agent>`.
- CLI parses `memory promote --workspace=<path> --memory-id=<id>
  [--source=<manual_review|user|verified_result>]` and rejects `agent_claim`
  as a promotion source.
- CLI parses `repo-audit --workspace=<path>`.
- CLI parses `ingest --workspace=<path> --adapter=<agent> [--session=<id>]
  [--retry-limit=<n>] [--fallbacks=a,b]`.
- verifier runner executes configured commands and persists
  `verifier-runs.jsonl`.
- verifier runner kills timed-out process trees and persists a `timed_out`
  verifier-run record.
- verifier runner does not block when a shell exits but a background descendant
  keeps verifier output pipes open.
- dashboard snapshots replay verifier runs as capture rows.
- dashboard snapshots replay recent probe runs as `kind:probe-run` capture rows
  with probe type, outcome status, summary, full detail payload, and
  `kind:probe` display-filter support; case files include the same runs as
  bounded `ProbeRun` evidence.
- case files use persisted verifier runs when computing verification status,
  latest verifier commands, and stale-verification entropy.
- case files include a task summary built from normalized user instructions:
  latest goal, source event id, acceptance criteria with source ids/confidence,
  and ambiguity markers. Missing task evidence raises bounded goal entropy;
  explicit ambiguity raises high goal entropy without bypassing verifier or
  validator gates.
- advisor-visible case files sanitize task text and keep task source refs only
  while the referenced evidence remains visible after tainted-evidence pruning
  and budget trimming.
- clean storage sanitization preserves structured whitespace in user
  instructions, so acceptance-criteria blocks remain parseable after JSONL
  persistence.
- active trusted durable memory contributes `source: durable_memory`
  requirement graph nodes and proof steps, while acceptance-derived
  requirements keep `source: acceptance_criterion`.
- requirement proof steps include bounded direct trace refs, repo hunk refs,
  monitor control decision refs, and outcome refs when evidence cites the
  requirement source, evidence, latest verification ids, or linked advice ids,
  giving requirement queries and dashboard detail a cross-source bridge from
  case-file requirements into trace, repo/blame, control, and outcome history.
- requirement proof steps include deterministic `proof_strength` with a bounded
  score plus named signals and gaps, so claim-only requirements stay visibly
  weak while requirements backed by trace rationale, hunk history, monitor
  control, and successful outcomes are easy to distinguish without an advisor.
- dashboard requirement rows use the latest proof strength for severity and
  summary text, so a `covered` requirement with only weak claim evidence remains
  visible as a warning instead of blending into fully supported requirements.
- dashboard workspace and fleet severity roll up warning and critical capture
  rows, so untraced repo hunks, uncovered requirements, and weak covered
  requirements cannot hide behind otherwise healthy agent-health scores.
- repo hunk history queries now include file-level summaries that count traced,
  missing-rationale, and untraced hunks per path, and dashboard capture rows
  expose those summaries with `kind:repo-hunk-file` for fast frontend scanning
  before drilling into raw `kind:repo-hunk` rows. The native details pane
  renders repo-hunk-file rows as a structured count/status card above the raw
  JSON payload.
- later passing verifier runs clear earlier verifier failure entropy, including
  same-second runs resolved by append order.
- `test_result` events from normalized JSONL or adapter ingestion feed the same
  verification status, latest-command, and entropy paths as command-based
  verifier results.
- `repo_diff` events from normalized JSONL or adapter ingestion are traceable
  and can raise repo/blame entropy when they lack rationale.
- OpenCode wrapped plugin `diff.changed` payloads preserve nested
  `event.properties` file, line range, and rationale fields, so repo-diff
  events from plugin hooks can become traceable blame evidence.
- OpenCode wrapped plugin events preserve nested `event.properties` replay
  provenance, including ids, sequence, git state, source pointers, and
  redaction metadata. Wrapped failed tool results read nested exit/status/error
  fields so failed edits cannot become false successful `file_change` events.
- Claude command-bearing `PostToolUse` records normalize nested `result`,
  `tool_result`, `tool_response`, and `response` output/status/error/exit-code
  fields into `command_result` events, so shell failures in hook-shaped payloads
  feed the same command-loop and service-failure policy as top-level results.
- Claude `SubagentStop` hook records normalize into `agent_health` lifecycle
  events so subagent completion is visible in replay and liveness views.
- agent-health entropy rises when the same non-verification command fails
  repeatedly, and advice emits a loop-breaking `retry_agent` packet targeted at
  the unhealthy agent.
- policy validation and advisor request tests prove low-entropy `retry_agent`
  proposals are rewritten or pruned, while retry-level agent-health fixtures
  still allow retry target normalization and attempt clamping.
- policy validation and advisor request tests prove low-entropy
  `send_follow_up` proposals are rewritten or pruned, while plan, repo/blame,
  and context entropy can still justify bounded follow-up packets.
- later command success, successful command results, and healthy model messages
  clear command-loop and service-failure streaks.
- streaming JSONL monitor service-failure retry counts reset after a healthy
  model/output event or successful command result.
- streaming JSONL and wrapped-command supervision run the bounded
  advice/validator/packet-dispatch loop for completion claims and
  premature-stop language, letting policy force verification or send a bounded
  follow-up packet without waiting for a separate manual `advise` command.
- adapter message/content deltas are persisted as low-signal `command_output`
  evidence and skipped by legacy intervention heuristics and streaming endpoint
  advice triggers, so partial tokens cannot cause premature-stop or service
  recovery decisions.
- persisted service-failure interventions do not keep agent-health degraded
  after a later healthy agent event or successful command result.
- service-failure switch interventions do not degrade the fallback target.
- degraded-memory interventions attribute the `agent` field to the degraded
  session, not the replacement candidate.
- session-derived degraded/stale agent-health causes still target the affected
  session agent even when there is no direct evidence id.
- repeated service-failure events select a fallback `switch_agent` action that
  does not point at the failed agent.
- advisor-proposed retry/switch actions are normalized so retry targets the
  unhealthy agent, even if the advisor supplied a different target, and switch
  chooses a different fallback agent.
- retry attempts are clamped to a safe range and shown in the packet.
- verification packets include trace-rationale remediation when repo/blame
  entropy is also high.
- verifier runs automatically record pass/fail action outcomes for the current
  matching `force_verification` advice, require explicit packet verifier
  commands to match even for full-suite verification, use the validated final
  action for expected deltas, measure observed verification entropy
  before/after the run, and do not attach outcomes to stale older advice after a
  newer control decision has been recorded.
- failed writable handoff outcomes from target-agent failure events or handoff
  timeouts release the matching active worktree lock when the lock owner matches
  the dispatched target, preventing crashed or never-started handoffs from
  blocking the next recovery action until stale-lock expiry.
- retry-agent advice automatically records succeeded/failed action outcomes
  from later normalized events for the same target agent: the same failing
  command signature succeeding marks recovery, while repeating it marks retry
  failure; outcomes store observed deltas for every expected entropy kind plus
  agent health.
- dispatched control packets write create-new immutable packet files and a
  stable `.agent-monitor/outbox/<agent>/latest.md` pointer so a live or fresh
  adapter can read the newest urgent/follow-up packet without knowing its
  generated id.
- run and agent-session packet preconditions are evaluated against the latest
  event for the packet target agent, so unrelated newer activity from another
  adapter cannot incorrectly satisfy or invalidate a packet.
- packet artifacts are keyed by packet id, reject packet-id reuse across
  urgency changes, and log dispatch only after the latest pointer publishes.
- urgent non-handoff packet dispatch is de-duplicated by action kind, target
  agent, evidence refs, and packet preconditions; repeated force-verification,
  trace-block, or retry packets with no new evidence or HEAD change record a
  `suppressed_duplicate` dispatch that reuses the prior packet instead of
  rewriting `.agent-monitor/outbox/<agent>/latest.md`.
- latest outbox publication uses replace-existing semantics on Windows, and
  generated ids include process id plus an in-process sequence to reduce
  cross-process collision risk.
- advice case files are persisted only after packet validation and dispatch
  succeed, preventing rejected packets from leaving misleading case-file
  records.
- injection plans for Codex, Claude Code, Pi, and OpenCode tell each agent to
  read its own latest outbox packet at turn start and follow the packet
  according to its urgency before continuing an older plan, treating
  blocking/urgent packets as immediate gates.
- injected instruction blocks are compressed into operational rules: packet
  precedence, no early stop while verification/probes/trace remain open,
  bounded user questions only, controlled temp files, trace rationale for
  meaningful changes, exact blocker reporting, explicit packet closure rules,
  and a fixed dense status format.
- Explicit `inject` and automatic `inject-running` load workspace adapter
  config and reject disabled adapters before writing monitor instructions, so
  instruction-file installation cannot bypass the handoff policy boundary.
- Markdown instruction injection uses agent-scoped managed blocks, so shared
  files such as `AGENTS.md` can carry both Codex and OpenCode monitor rules.
  Legacy generic monitor blocks are migrated to the installing agent's scoped
  block on update.
- Codex injection installs `AGENTS.md`, PowerShell pre-tool and event bridges,
  and project `.codex/hooks.json` `PreToolUse`, `PostToolUse`, `Stop`, and
  `PreCompact` hook config. The hooks file is merged as JSON so existing
  project Codex hooks survive while deterministic policy blocks route through
  `agent-monitor hook-response --adapter=codex --format=codex` and
  post/lifecycle telemetry streams through `agent-monitor ingest
  --adapter=codex`.
- Codex `apply_patch` hook payloads are parsed for
  `*** Add/Update/Delete File:` and `*** Move to:` headers. Move destinations
  are preferred for trace labels, protected-path policy can block unsafe
  patches before execution, and successful patch hooks produce bounded
  per-file `file_change` evidence without storing the full patch as the command
  label.
- Claude Code injection installs `CLAUDE.md`, PowerShell pre-tool and event
  bridges, and project `.claude/settings.json` `SessionStart`, `PreToolUse`,
  `UserPromptSubmit`, `PostToolUse`, `Stop`, `SubagentStop`, `PreCompact`, and
  `Notification` hook entries. The settings file is merged as JSON so existing
  project Claude settings and hooks survive while pre-tool policy routes through
  `agent-monitor hook-response --adapter=claude-code --format=claude-code` and
  prompt, post, and lifecycle telemetry streams through `agent-monitor ingest
  --adapter=claude-code`.
- OpenCode injection targets project `AGENTS.md`, matching OpenCode project
  rules loading, and installs `.opencode/plugins/agent-monitor.js`. The plugin
  handles `tool.execute.before`, calls `agent-monitor hook-response
  --adapter=opencode`, and throws on deterministic blocks so OpenCode's plugin
  permission surface maps back to monitor policy. It also streams
  `tool.execute.after`, `session.idle`, and `session.error` telemetry through
  `agent-monitor ingest --adapter=opencode`. Pi remains a sidecar because Pi
  needs an external supervisor wrapper for context delivery, sandboxing, and
  process launch; generic `wrap --agent=pi` is refused until that boundary is
  explicit.
- blame reports query trace JSONL by file and optional line, preserving event
  metadata and ranking exact line evidence first.
- repo-audit flags live git hunks that lack matching trace evidence or
  rationale.
- case files embed repo-audit reports and raise repo/blame entropy for dirty
  hunks without complete trace rationale.
- deterministic advice sends a follow-up packet for repo/blame entropy when no
  higher-priority verification or context action dominates.
- monitor-owned `local_evidence`, `repo_inspection`, and configured
  `targeted_test` probes execute automatically from `advise_workspace` after
  the latest still-current `run_probe` advice is persisted, and can still be
  replayed manually through `agent-monitor probe`. Probe execution persists
  `probe-runs.jsonl`, reuses monitor-event, repo-audit, or verifier-run
  evidence, and records a `run_probe` outcome without asking the active agent
  to perform the observation. `local_evidence` never executes arbitrary command
  text, and targeted-test probes reject commands absent from the configured
  verifier registry. Recent probe runs now feed dashboard capture rows and
  bounded case-file evidence so later advice can cite monitor-owned
  observations without reading raw side logs.
- handoff writes adapter-specific fresh-agent packets with memory, trace,
  verification, and blame guidance.
- current-run subagent lifecycle scoring detects overlapping unresolved worker
  ownership for the same source/test/UI path and sends a follow-up packet that
  requires joining, cancelling, or reassigning workers to disjoint paths before
  more fan-out.
- HTTPS advisor transport accepts provider endpoints instead of rejecting the
  scheme before network I/O.

## Non-Goals For This Slice

- live hook integration with Pi beyond the current Codex, Claude Code, and
  OpenCode pre-tool policy surfaces.
- SQLite migration.
- full multi-worktree orchestration beyond exclusive lock records.
- real-time UI control actions.
- raw transcript parsing beyond existing normalized events.
- autonomous memory promotion without a trusted operator, user, or verified-result source.
