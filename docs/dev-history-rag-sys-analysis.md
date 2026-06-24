# Local Dev History Analysis: F:\rag_sys

Date: 2026-06-24

This note records a bounded local analysis of Codex and Claude Code history for `F:\rag_sys`. The analysis scans local JSONL histories and emits aggregate metadata only: event type counts, tool counts, command heads, signal categories, and project file references. It does not import raw transcript text or credentials into the monitor.

## Corpus

- Codex: 36 workspace-matched session files under `C:\Users\yys\.codex\sessions`, 82,563,658 bytes, 21,319 parsed lines, 36 sessions, from 2026-06-17 to 2026-06-24.
- Claude Code: 432 project history files under `C:\Users\yys\.claude\projects\F--rag-sys`, 369,301,667 bytes, 77,842 parsed lines, 4 sessions, from 2026-06-13 to 2026-06-22.
- `F:\rag_sys\.agent-monitor` contained only `tmp`, so the useful supervision evidence lived outside monitor storage.

## Findings

- `external_history_present`: 468 local history files matched the workspace. The monitor needs a safe local-history import/analyze surface so handoff, blame, and recovery can start from evidence that already exists.
- `verification_entropy`: 14,136 verification or unverified-stop signals. This supports making verifier freshness a hard control invariant: no continue after source changes when the relevant verifier is stale.
- `user_interrupt_entropy`: 249 agent-question signals. This supports gating `AskUser` behind deterministic probes, logs, diffs, and verifier results instead of letting agents push obvious next steps back to the user.
- `agent_health_entropy`: 7,237 service/context-instability signals. The monitor should classify provider/tool instability separately from task difficulty, retry transient failures first, and switch or spawn fresh only when loop signatures persist.
- `blame_hotspots`: repeated references clustered around `e2e/word-regression.spec.js`, `frontend/src/views/portal/SearchView.vue`, `frontend/src/views/rag/provider/index.vue`, `frontend/src/views/rag/knowledgeBase/index.vue`, `backend/src/main/java/com/nercita/rag/controller/DocumentController.java`, and `backend/src/main/java/com/nercita/rag/kb/KnowledgeBaseService.java`.

## Product Implication

Local agent history should become a bounded evidence source for entropy and blame. It should not become raw durable memory. The safe path is:

1. Match Codex by workspace metadata, not text mentions.
2. Match Claude Code by encoded project directory.
3. Extract aggregate signal counts, tool categories, command heads, file refs, and timestamps.
4. Use those aggregates to create evidence ids for case files, verifier freshness checks, and repo hunk blame.
5. Feed only selected healing findings into deterministic entropy as conservative historical priors below hard-action thresholds: verification and agent-health can raise low priors, blame hotspots require current changed-file overlap, and user-interrupt/external-history findings stay evidence-only until fresh current-run evidence proves a real decision or context risk.
6. Promote memory only when user confirmation, verification, or repeated durable evidence supports the claim.

This example shows why the monitor needs historical healing: the project already had enough local evidence to detect verification risk, avoid unnecessary user interrupts, and identify blame hotspots, but that evidence was invisible to `.agent-monitor` until analyzed.













The uploaded package does **not** contain raw chat transcripts. It contains a safe aggregate of Codex and Claude Code histories: counts, tool usage, command heads, signal categories, timestamps, and file references. So I cannot say “at turn 183 the agent said X and that was wrong.” But the aggregate is still enough to identify recurring bad behaviors.

The main thing these agents are doing badly is not “they cannot code.” It is more specific:

> They keep losing proof, context, and causal ownership of the repo state.

That shows up in several concrete patterns.

## 1. They talk about verification, but do not preserve verification freshness

The package shows very high verification-related signal volume:

| Agent       | Verification-language signals | Premature/unverified-stop signals | Parsed lines | Approx. signal density      |
| ----------- | ----------------------------- | --------------------------------- | ------------ | --------------------------- |
| Codex       | 4,322                         | 43                                | 21,319       | ~20.3% verification signals |
| Claude Code | 9,612                         | 159                               | 77,843       | ~12.3% verification signals |

These counts are noisy, because “verification-language” can include commands, outputs, summaries, and failed-test text. But the pattern is clear: verification is constantly discussed, yet the package has no durable proof that verification happened **after the latest relevant code change**.

That is the real bad behavior:

> Agents often treat “I ran tests recently” as equivalent to “the current diff is verified.”

Those are not the same.

The monitor should detect this mechanically:

```text
if source_changed_after(last_relevant_test_run):
    block "done"
    force verification
```

The important state is not “tests were mentioned.” It is:

```text
current write epoch
last verifier epoch
files covered by verifier
test result
whether verifier failed, passed, or was partial
```

Without that, agents can easily end a task with stale confidence.

## 2. They repeatedly rediscover the same code instead of building stable working context

Codex tool use:

```text
shell_command: 4,935
apply_patch:     480
```

That is roughly **10 shell commands per patch**.

Top Codex commands:

```text
get-content: 1,567
rg:          1,123
get-childitem: 205
git:         122
```

Claude tool use:

```text
Read:       12,179
Grep:        2,048
Glob:        1,125
Edit:        2,057
Write:         640
```

Claude has about **15,352 read/search/navigation actions** versus **2,697 edit/write actions**, or roughly **5.7 navigation actions per modification action**.

Some exploration is normal. But at this scale, the likely failure pattern is:

> The agents keep rebuilding local context by rereading files instead of carrying forward a durable, structured model of the task.

This is especially visible in repeated references to the same hotspots:

```text
frontend/src/views/rag/provider/index.vue
frontend/src/views/rag/knowledgeBase/index.vue
backend/src/main/java/com/nercita/rag/kb/KnowledgeBaseService.java
backend/src/main/java/com/nercita/rag/controller/DocumentController.java
frontend/src/views/portal/SearchView.vue
e2e/word-regression.spec.js
```

The bad behavior is not merely “they read a lot.” The bad behavior is:

> They fail to convert repeated reading into stable design memory, blame memory, or test-memory.

A monitor should detect a rediscovery loop:

```text
same file read/searched repeatedly
+ no new edit
+ no new verifier result
+ same failure category persists
= context loop
```

Then the monitor should force a packet like:

```text
Stop rereading broadly.
Work only on these 2 files.
State the exact invariant.
Run this verifier.
Do not ask the user unless verifier output is ambiguous.
```

## 3. They thrash around repo hotspots

The file references are concentrated in a small number of files. That usually means one of two things:

1. The task genuinely centers on those files.
2. The agents are repeatedly touching symptoms without resolving the root cause.

The package cannot prove which one, but the density is high enough to treat these as suspicious hotspots.

Claude Code references:

```text
provider/index.vue:            1,033
knowledgeBase/index.vue:         872
KnowledgeBaseService.java:       686
document/index.vue:              532
DocumentController.java:         312
```

Codex references:

```text
e2e/word-regression.spec.js:     234
SearchView.vue:                 210
knowledgeBase/index.vue:         173
DocumentController.java:         170
KnowledgeBaseService.java:       160
```

The bad behavior is:

> Agents probably keep moving between frontend symptom files, backend service files, controller files, and e2e tests without preserving a causal graph.

For this project, the monitor should build a blame graph:

```text
UI file changed
    ↓
API endpoint involved
    ↓
backend controller/service touched
    ↓
test affected
    ↓
latest verifier result
```

Then, when an agent wants to keep editing, the monitor can ask:

```text
Have we identified whether the bug is UI state, API contract, service behavior, or test expectation?
```

If not, force a probe instead of allowing another broad edit.

## 4. They confuse operational instability with task difficulty

The package reports many service/context-instability signals:

| Agent       | Service/context-instability signals | Parsed lines | Approx. density |
| ----------- | ----------------------------------- | ------------ | --------------- |
| Codex       | 1,939                               | 21,319       | ~9.1%           |
| Claude Code | 5,298                               | 77,843       | ~6.8%           |

Again, this is lexical and noisy. But the likely bad behavior is real:

> Agents tend to keep reasoning as if the task is hard when the actual problem may be provider failure, timeout, shell failure, context loss, permission failure, or broken environment state.

Those need different responses.

Bad agent behavior:

```text
timeout happens
agent retries vaguely
context gets compacted
agent rereads repo
same failure repeats
agent changes more code
```

Better monitor behavior:

```text
classify failure layer first
```

For example:

```rust
enum FailureLayer {
    Provider,
    Transport,
    RateLimit,
    Auth,
    ShellRuntime,
    RepoState,
    TestRuntime,
    TaskLogic,
    Unknown,
}
```

A provider or transport failure should not count as evidence that the code change is bad. A test assertion failure should.

## 5. They ask the user too early

The package shows:

```text
Codex agent-question signals: 58
Claude Code agent-question signals: 191
Total: 249
```

This is not enormous relative to the corpus. But for a coding monitor, even a smaller number matters because user interruption is expensive.

The bad behavior is probably:

> Agents ask the user when the answer could be obtained from the repo, tests, logs, git history, or design memory.

The monitor should not ban questions. It should classify them.

Good user questions:

```text
Which product behavior do you prefer?
Do we preserve backward compatibility here?
Can I use this credential or external service?
Should this breaking API change be allowed?
```

Bad user questions:

```text
Should I run tests?
Which file should I inspect?
Should I check the backend?
Should I continue debugging?
```

Those should be resolved by deterministic probes.

Policy:

```text
AskUser allowed only if:
    local probes exhausted
    and decision is preference/business/authority-sensitive
    and expected user information cannot be inferred safely
```

## 6. Codex appears to have weak subagent lifecycle management

Codex aggregate:

```text
spawn_agent: 69
close_agent: 29
wait_agent: 4
```

This is a red flag.

It may not mean every spawned agent was abandoned, because the safe package may not capture every lifecycle edge correctly. But from the aggregate, the bad pattern to look for is:

> Agents spawn helpers but do not reliably join, summarize, close, or integrate their results.

That creates hidden entropy:

```text
subagent A found something
main agent forgets
subagent B edits overlapping file
main agent continues with stale state
verification result applies to only one branch of work
```

The monitor should enforce:

```text
spawned agent must end in one of:
    joined_with_summary
    cancelled_with_reason
    timed_out
    superseded
    failed
```

And no fresh subagent should be spawned if unresolved subagents already touch the same files.

## 7. Claude Code appears to fan out into many subagent files

Claude Code aggregate:

```text
4 sessions
432 files
428 subagent files
```

This is not automatically bad. Claude Code may store subagent traces separately by design. But operationally it creates a supervision problem:

> A single user task becomes hundreds of trace fragments.

The bad behavior is fragmentation:

```text
many local histories
weak parent-child topology
hard to know which subagent caused which repo state
hard to know which result supersedes which result
```

A monitor should reconstruct:

```text
session
  → parent agent
      → subagent
          → touched files
          → claims made
          → tests run
          → result integrated? yes/no
```

Without that, history analysis turns into noisy counts.

## 8. They waste effort on shell/workdir mechanics

Claude command heads:

```text
cd:           2,152
set-location:   337
git:            311
grep:           285
find:           251
python:         246
ls:             233
```

Codex command heads:

```text
get-content:    1,567
rg:             1,123
$i=0;             439
get-childitem:    205
```

The bad behavior here is:

> Agents spend too much attention on navigating Windows/PowerShell/path mechanics instead of using stable repo-relative operations.

This creates failure modes like:

```text
wrong working directory
mixed absolute and relative paths
Windows path vs Unix path confusion
case-sensitivity mismatch
same file counted as multiple files
commands that work in one shell but not another
```

For a Rust monitor, normalize this aggressively:

```text
F:\rag_sys\frontend\...
F:/rag_sys/frontend/...
frontend/...
```

All should become one canonical repo-relative path.

## 9. They produce evidence volume, not evidence quality

The package itself exposes a meta-problem: the histories contain tons of signals, but not enough structured causality.

For example, “failure-language” is high:

| Agent       | Failure-language signals |
| ----------- | ------------------------ |
| Codex       | 3,428                    |
| Claude Code | 8,711                    |

But this does not tell us:

```text
Was the failure before or after the edit?
Was it from code, test setup, provider, network, or shell?
Was it fixed?
Did the same failure signature recur?
Which file mutation caused it?
```

That is exactly what agents are bad at too:

> They produce lots of local evidence but do not maintain a clean causal ledger.

The monitor should fix that by turning raw events into:

```text
mutation → verifier → failure signature → diagnosis → action → outcome
```

## 10. Their biggest practical failure: they do not know when to stop editing

The combined pattern is:

```text
many reads
many shell commands
many failure signals
many verification mentions
repeated hotspots
some premature/unverified completion signals
lots of service/context instability
```

This usually means agents are poor at the “stop condition.”

They continue editing when they should instead do one of these:

```text
rerun verifier
isolate failure
restart with clean context
ask user for real product decision
switch agent
stop because provider/tooling is broken
```

A production monitor should have hard stop rules.

Example:

```text
If same file edited ≥ 3 times
and same verifier failure persists
and no new hypothesis was recorded:
    block further edits
    require failure isolation packet
```

Another:

```text
If agent says "done"
and current diff is nonempty
and verifier freshness < threshold:
    block completion
    force verification
```

Another:

```text
If same command class fails ≥ 3 times
and error signature is unchanged:
    block retry
    classify failure layer
```

## Ranking the bad behaviors

Based on this package, I would rank the agent weaknesses like this:

| Rank | Bad behavior                          | Confidence  | Why                                                          |
| ---- | ------------------------------------- | ----------- | ------------------------------------------------------------ |
| 1    | Weak verification freshness           | High        | Huge verification signal volume plus unverified-stop signals; no epoch-based proof |
| 2    | Repeated context rediscovery          | High        | Heavy Read/Grep/Get-Content/RG usage and repeated hotspots   |
| 3    | Hotspot thrashing                     | Medium-high | Same frontend/backend/test files dominate history            |
| 4    | Poor failure attribution              | Medium-high | Large failure/service/context signal volume without structured failure layer |
| 5    | User interruption before local probes | Medium      | 249 agent-question signals; needs raw text to judge quality  |
| 6    | Subagent lifecycle fragmentation      | Medium      | Codex spawn/close/wait imbalance; Claude many subagent files |
| 7    | Shell/path/workdir inefficiency       | Medium      | High `cd`, `set-location`, `get-content`, absolute path references |

## What the monitor should learn from this specific history

The monitor should not merely say:

```text
There were many failures.
```

It should say:

```text
This project has recurring uncertainty around:
- frontend RAG provider and knowledge-base views
- backend document controller and knowledge-base service
- e2e word regression behavior
- stale verification after edits
- repeated repo rediscovery
- possible provider/context instability
```

Then, for the next live agent run, it should enforce:

```text
1. Before editing, identify whether the current issue is UI, API contract, backend service, or test expectation.
2. Limit active files to the smallest causal slice.
3. After every source mutation, invalidate affected verifier results.
4. Do not allow "done" unless verifier freshness covers changed files.
5. If the same failure signature repeats, stop editing and classify the failure.
6. Do not ask the user until local probes are exhausted.
7. If a subagent is spawned, require join/summary/outcome before completion.
```

The useful conclusion is not “Codex is worse” or “Claude is worse.” The aggregate does not support that, because the corpus sizes are different. The useful conclusion is:

> Both agents generate a lot of activity, but they do not preserve enough structured state to know whether they are making progress, repeating work, or merely accumulating transcript noise.

That is the exact gap your Coding Agent Monitor should attack.
