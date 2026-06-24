# Coding-agent history: deep empirical failure analysis

**Project:** `rag_sys`  
**Study date:** 2026-06-24  
**Scope:** Complete supplied Codex and Claude Code history archive, including main sessions, tool calls, subagent traces, workflow journals, repository mutations, verification actions, and user follow-ups.

## Executive diagnosis

The dominant failure is not lack of coding skill. It is **premature epistemic closure under uncontrolled work in progress**.

The agents repeatedly convert activity into confidence:

```text
files inspected + code changed + build green + plausible explanation
                         ↓
                    “completed”
```

But the actual acceptance state often remains partially unknown:

```text
unparsed requirement artifacts
+ stale or incomplete verification
+ unresolved subagents
+ resurrected rejected designs
+ untested runtime/visual behavior
+ ambiguous scope changes
                         ↓
                 corrective user turn
```

The monitor should therefore optimize **verified requirement closure**, not token throughput, tool volume, number of agents, number of tests mentioned, or a single synthetic “entropy score.”

The most important design shift is:

> Represent uncertainty as a calibrated probability distribution over concrete failure hypotheses, and represent success as traceable closure of authoritative requirements.

“Entropy” remains useful, but only as a property of a belief distribution. A count of error words, retries, or modified files is not entropy.

## 1. Dataset and method

The archive contains 468 history files: 432 Claude Code files and 36 Codex files. The analysis reconstructed four Claude main sessions, 413 Claude subagent traces, 70 Codex turns, 12,248 normalized top-level tool calls, and 4,684 file events. Claude supplied 77,843 parseable JSONL records plus one malformed record; Codex supplied 21,319 parseable records.

The study used five analytical layers:

1. **Session reconstruction:** parent turns, subagent topology, tool calls, interruptions, context compactions, and terminal states.
2. **Process mining:** phase transitions among inspection, editing, verification, coordination, health checks, and other shell activity.
3. **Requirements/outcome proxies:** the next user response after a completed turn, with conservative detectors for immediate corrections and broader detectors for negative or missing-work feedback.
4. **Repository topology:** canonicalized paths, edit concentration, architectural layer crossing, and co-change networks.
5. **Decision analysis:** Bayesian credible intervals, posterior comparisons, multiobjective dominance, queueing/WIP interpretation, and proposed causal models.

### Methodological limits

The transcript does not provide perfect ground truth. A follow-up may introduce new scope rather than prove failure. A “missing terminal result” subagent may still have produced useful partial work. Some browser tests or shell-side edits may be hidden from the normalized top-level event class. Multiple agents modified the same repository, weakening attribution. Token fields include cache traffic and are not reliable cost estimates. Consequently, all numerical rates below are **observational proxies**, not model leaderboards or causal treatment effects.

That limitation is itself a design finding: a production monitor must create stronger outcome and attribution data than current agent runtimes provide.

## 2. Primary empirical findings

### 2.1 Closure claims outrun requirement closure

Among 138 completed Claude segments followed by another real user turn:

| Follow-up proxy | Count | Rate | Jeffreys 95% interval |
|---|---:|---:|---:|
| Broad negative/corrective next prompt | 30 | 21.7% | 15.5%–29.2% |
| Negative/corrective within 30 minutes | 18 | 13.0% | 8.2%–19.4% |
| Narrow, clear same-task correction within 30 minutes | 10 | 7.2% | 3.8%–12.5% |

The narrow detector is a conservative lower bound. The broad detector is overinclusive: some user messages contain new requirements, injected skill text, or dissatisfaction not directly caused by the preceding turn.

Broad completion language is nevertheless a useful warning signal. Clear immediate corrections occurred after 8 of 83 broad claims, versus 2 of 55 non-broad claims. A simple Beta-Binomial comparison gives approximately a 91% posterior probability that the broad-claim correction rate is higher; the posterior median relative risk is about 2.5, but its 95% interval is wide. This is suggestive, not conclusive.

The transcript contains several high-confidence examples:

- An agent reported analysis artifacts as complete; the next user response was essentially “write the code.”
- An agent called a UI change complete after a successful build while explicitly declining browser validation; the next response said it did not work.
- An agent reported all verification complete and zero errors; the next response exposed a frontend compile/runtime error.
- Codex reported that all 20 Word test items and all screenshots had been handled; the next response identified hand-drawn numbered annotations and unresolved workflow semantics.

This is best described as an **evidence-to-claim gap**.

A monitor must treat a completion statement as a transaction that needs a certificate, not as natural-language opinion.

### 2.2 Requirements are not maintained as an authoritative graph

The history contains durable constraints that recur across sessions:

- do not create a separate public-dataset concept;
- do not preserve compatibility paths merely for compatibility;
- do not put SQL inside Java;
- support Windows and Linux;
- use browser/Playwright validation for UI work;
- preserve reusable ingestion/workflow semantics;
- do not ask the user to choose routine implementation sequencing;
- read recent history and do not redo settled work;
- follow the project’s design system rather than inventing isolated UI patterns.

These are not ordinary chat details. They are architectural invariants, rejected alternatives, validation obligations, and user-governance preferences. Yet they repeatedly disappear from working context.

A particularly clear episode occurred when the agent derived a “public dataset directory” concept and a compatibility design even though neither had authority. The user rejected both. Later, compatibility language returned again. This is **rejected-design resurrection**.

The test document exposed another failure. The agents could parse text and inspect screenshots, but they did not reliably map hand-drawn numeric annotations to the corresponding acceptance issues. This is not merely an OCR problem. It requires a multimodal requirement model:

```text
paragraph / screenshot / drawn marker / callout / affected UI region
                         ↓
                  acceptance criterion
                         ↓
               implementation evidence
                         ↓
                 validation evidence
```

The correct durable object is a requirement graph, not a prose summary.

### 2.3 Verification and validation are conflated

For 122 completed Claude segments that edited code and had a later user turn:

| Evidence state | Count | Share |
|---|---:|---:|
| Recognized verification after the last explicit edit | 59 | 48.4% |
| Marked stale or missing by the parser | 43 | 35.2% |
| Claimed verification without recognized fresh evidence | 49 | 40.2% |
| Claimed completion without recognized fresh evidence | 30 | 24.6% |
| Broad claim without recognized fresh evidence | 36 | 29.5% |

The remaining cases are ambiguous because some writes or verification may occur in shell commands or subagents.

Verification modality is also skewed:

| Modality among 122 edited turns | Count |
|---|---:|
| Any recognized test action | 47 |
| Any build action | 94 |
| Any browser action | 11 |
| Build only, no test or browser | 47 |
| No recognized test, build, or browser | 21 |

This explains several failures. A build verifies compilation or packaging; it does not validate that a user workflow behaves correctly, that a route loads, that a visual hierarchy matches the requirement, or that a video preview is visible. NASA’s systems-engineering distinction is directly applicable: verification asks whether the product conforms to specified requirements; validation asks whether it fulfills intended use in the intended environment.[1]

A counterintuitive statistical result reinforces the need for causal discipline: in the raw data, turns with recognized fresh verification received more negative follow-ups than turns without it. That does not imply testing is harmful. Difficult tasks are more likely to trigger both extensive verification and later complaints. This is classic confounding by task difficulty and scope. A monitor that rewards raw test counts would optimize the wrong proxy.

The correct object is a **verification certificate** tied to:

- repository tree/commit and mutation epoch;
- requirement or risk being checked;
- affected paths and services;
- verifier type and environment;
- pass/fail/partial result;
- test-oracle authority;
- freshness relative to subsequent edits.

### 2.4 Coordination degrades sharply with fan-out

The archive contains 413 Claude subagent files. Direct subagents had no terminal structured result in 22 of 103 cases, or 21.4% (Jeffreys 95% interval 14.3%–30.0%). Workflow subagents had no terminal structured result in 128 of 310 cases, or 41.3% (35.9%–46.8%). A posterior comparison gives greater than 99.99% probability that the workflow missing-result rate is higher; the median difference is about 19.8 percentage points.

Across 15 workflow groups, fan-out and recorded result rate have Spearman correlation `ρ = -0.644`, `p = 0.0095`. Aggregating the observed groups:

| Workflow fan-out | Terminal results |
|---|---:|
| At most 10 agents | 35 / 37 = 94.6% |
| More than 10 agents | 142 / 273 = 52.0% |

This split is observational and workflows differ in difficulty, but the operational pattern is strong. Examples range from a 12-agent workflow with no recorded results to a 71-agent workflow with only 23 recorded results.

Codex shows a related lifecycle gap: 69 spawn operations, 29 close operations, and only four waits. Missing explicit closure does not prove abandonment, but it means the supervisor cannot establish whether work was integrated, cancelled, timed out, superseded, or still active.

This is queueing and distributed-systems behavior. At roughly fixed integration throughput, increasing active work raises cycle time and unfinished inventory; Little’s Law formalizes the relationship between work in system, throughput, and time in system under stable assumptions.[2] The practical conclusion is not “never parallelize.” It is:

> Parallelism is valuable only while decomposition quality, ownership isolation, and integration capacity keep pace.

The monitor needs WIP limits, ownership leases, join barriers, and adaptive fan-out—not a fixed “spawn many agents” policy.

### 2.5 Context continuity is a material reliability problem

The raw histories contain:

- 17 Claude context compactions;
- 28 Codex context compactions;
- 40 Claude user-interruption markers;
- 16 interrupted Codex turns out of 70, or 22.9% (Jeffreys 95% interval 14.2%–33.7%).

The user repeatedly asks agents to read recent history, continue unfinished work, and avoid redoing previous analysis. These requests are evidence that context compaction and session changes lose more than token-level detail. They lose:

- which requirements were authoritative;
- which alternatives were rejected;
- which files and services remain causally relevant;
- which verification evidence became stale;
- which subagent results were integrated;
- what “done” was supposed to mean.

A summary that preserves only narrative is insufficient. Before compaction, the runtime should persist a typed continuation state.

### 2.6 Tool activity is not a reliable progress measure

Claude generated 6,559 normalized top-level tool calls with a 3.6% observed failure rate; Codex generated 5,689 with a 6.7% observed failure rate. Codex used far more inspection and generic shell activity relative to edits. Claude used more direct editing, mutation, coordination, and delegation.

Process mining shows long same-phase runs:

| Transition/shape | Claude | Codex |
|---|---:|---:|
| Inspect → inspect | 53.0% | 78.8% |
| Edit → edit | 55.7% | 48.1% |
| Verify → edit | 22.7% | 13.5% |
| Median collapsed phase count | 15.0 | 30.5 |
| Median same-phase repeat share | 43.5% | 62.1% |

These are not automatically defects. An audit legitimately inspects without editing, and a refactor may need many edits. The important result is that tool volume, duration, files changed, and verification count were not stable predictors of the next user response. In several coarse splits, larger turns even had fewer negative follow-ups, presumably because task type and difficulty dominate.

Therefore the monitor must not use “busy” metrics as reward. It should detect **state progress**:

```text
requirements closed
belief uncertainty reduced
failure cause isolated
verification coverage increased
architecture debt reduced
user decisions avoided safely
```

Process mining is still valuable. The IEEE Process Mining Manifesto explicitly treats event data as first-class and uses logs to discover, check, and improve operational processes.[3] Here it should be used for conformance and loop detection, not to infer quality from activity volume alone.

### 2.7 Repository history reveals unstable domain boundaries

After canonicalizing worktree, absolute, and case variants, the analysis found approximately 3,153 explicit edit events across 802 repository paths. Edit concentration is substantial:

- Gini coefficient: approximately 0.651;
- top 1% of paths: 22.8% of edits;
- top 10%: 60.9%;
- top 20%: 73.4%;
- 243 paths, or 30.3%, are needed to account for 80% of edits.

This is **not** a clean 80/20 distribution. There are strong hotspots, but the churn is also architecturally broad.

The largest hotspots include:

| Path | Edit events | Distinct segments | Active days |
|---|---:|---:|---:|
| `knowledgeBase/index.vue` | 169 | 37 | 7 |
| `KnowledgeBaseService.java` | 99 | 24 | 9 |
| `SearchView.vue` | 83 | 21 | 7 |
| `document/index.vue` | 71 | 24 | 7 |
| `market/dataset/index.vue` | 64 | 21 | 8 |
| `portal/index.vue` | 79 | 18 | 4 |
| `knowledgeBase/detail.vue` | 59 | 13 | 4 |
| `DocumentController.java` | 44 | 12 | 6 |
| `IngestCore.java` | 38 | 12 | 4 |
| `pipeline/index.vue` | 40 | 9 | 3 |

Of 261 segments with explicit edit events, 113 (43.3%) crossed architectural layers and 58 (22.2%) co-edited frontend and backend. Only nine segments (3.45%) explicitly co-edited test paths; this is a co-evolution metric, not a test-execution metric.

The hotspots align with the recurrent conceptual disputes in the chat:

- knowledge body vs knowledge base vs dataset;
- reusable parsing flow vs per-KB/per-document configuration;
- portal/search/browse semantics;
- preview and source-trace behavior;
- provider/model capability configuration;
- ingestion-path unification.

This is **change amplification around unstable bounded contexts**. Repeated edits are not merely sloppy implementation; many are downstream consequences of an unresolved domain model.

The monitor should maintain a co-change graph and compute an impact prior. A requirement touching high-centrality nodes should trigger architecture review and broader verification before implementation begins.

### 2.8 Test success can be manufactured by changing the oracle

One transcript episode updated several failing expectations on the basis that product behavior had evolved, after which dozens of tests were green. That may be legitimate. It may also be test laundering: changing the oracle to agree with the implementation.

A monitor cannot classify this by pass/fail alone. It needs a test-authority rule:

```text
assertion / snapshot / expected-value / fixture changed
                   ↓
require authoritative requirement link
+ independent behavior evidence
+ explanation of why the old oracle is invalid
```

Mutation testing is relevant because it evaluates whether a suite detects intentionally introduced faults, rather than merely whether the current implementation passes.[9] Metamorphic testing is useful when a precise oracle is unavailable: define relations that must remain true across transformed inputs, such as retrieval invariance under harmless formatting changes or consistent preview behavior across equivalent document conversions.[10]

### 2.9 Operational failures are mixed with coding failures

One Claude main session recorded 169 API errors:

| Category | Count |
|---|---:|
| Certificate-related | 94 |
| Connection/reset | 59 |
| Rate limit/overload | 11 |
| Other | 4 |
| Authentication | 1 |

If these events enter one generic “agent health entropy” score, the controller will make bad decisions. A provider certificate error, a shell permission error, a deterministic test failure, and a flawed architecture hypothesis require different actions and different retry budgets.

Operational risk and epistemic uncertainty must be separate dimensions. Repeated identical certificate failures can imply high operational impact but low diagnostic uncertainty.

### 2.10 The system violates the user’s decision budget

Claude issued 25 AskUser calls. After a clear instruction on 2026-06-17 not to ask the user to decide routine implementation sequencing, 12 AskUser calls still occurred. Many asked which migration area or batch to do next, despite an explicit preference for autonomous completion. A few questions were legitimately blocking, such as browser connectivity or credentials.

The controller needs an economic rule based on value of information:

```text
ask_user only if
    expected loss without the answer
  - expected loss after the answer
  > interruption cost
and no cheaper local probe can resolve the decision
```

Howard’s value-of-information formulation is directly applicable: uncertainty reduction matters only through its effect on decisions and outcomes.[6]

### 2.11 Scope volatility is real; not every follow-up is agent failure

The user’s requested product expanded substantially across RAGFlow compatibility, agricultural taxonomy, provider management, multimodal parsing, Label Studio, ASR, DeepDoc, deployment, UI design, browser testing, data migration, and presentation work. The user also explicitly requested very large subagent batches at times.

A fair analysis must not label every subsequent request as rework. The agents’ error was often different: they accepted an evolving, very large scope without freezing the current acceptance baseline, then made broad completion claims.

The monitor needs to distinguish:

- correction of an accepted requirement;
- discovery of an omitted requirement already present in evidence;
- authorized scope change;
- proposed but unapproved enhancement;
- preference reversal;
- environment-induced regression.

Without that classification, both reward modeling and user-friction metrics become misleading.

## 3. Probability model: from scalar entropy to belief state

### 3.1 Model concrete hypotheses

For a current run, maintain posterior probabilities over hypotheses such as:

```rust
pub enum FailureHypothesis {
    RequirementMissing,
    AcceptanceArtifactUnparsed,
    ImplementationDefect,
    ArchitectureConflict,
    StaleVerification,
    WeakTestOracle,
    RuntimeEnvironmentMismatch,
    ProviderTransientFailure,
    DeterministicToolFailure,
    ContextLoss,
    SubagentIntegrationFailure,
    UnauthorizedScopeExpansion,
}

pub struct HypothesisBelief {
    pub hypothesis: FailureHypothesis,
    pub probability: f64,
    pub expected_impact: f64,
    pub evidence_ids: Vec<EvidenceId>,
}
```

The Shannon entropy of this distribution is meaningful: it measures diagnostic uncertainty. A count of failures or touched files is merely evidence used to update the distribution.

### 3.2 Use Bayesian updates and hierarchical outcome models

For repeatable binary outcomes, maintain Beta-Bernoulli posteriors by context:

```text
P(action succeeds | agent, task class, failure class, repo region, environment)
```

For broader analysis, fit a hierarchical logistic model:

```text
logit P(corrective follow-up) =
    intercept
  + task class
  + scope volatility
  + requirement-artifact completeness
  + architecture-hotspot exposure
  + verification modality/freshness
  + subagent fan-out and join yield
  + context age/compaction
  + provider health
  + agent/model effects
  + session/project random effects
```

This is necessary because the raw history already demonstrates confounding: harder tasks receive more tools and verification and also receive more corrective feedback.

### 3.3 Calibrate advisor probabilities

Every advisor prediction should be recorded and scored after the outcome. Use reliability diagrams and the Brier score, originally formulated for probability forecasts.[7]

Useful predictions include:

- probability the next retry succeeds;
- probability a completion claim will survive the next user review;
- probability current verification is adequate;
- probability a question is truly blocking;
- probability a fresh agent outperforms continuing the current one.

An advisor that says “0.8” should be correct approximately 80% of the time in the relevant stratum. Otherwise its confidence must be recalibrated or ignored.

### 3.4 Use sequential testing for retry/stop decisions

Retries arrive sequentially. Do not hardcode “three retries.” Maintain likelihood under two hypotheses:

```text
H_transient: another retry has meaningful success probability
H_deterministic: the same cause will recur until state changes
```

Update the likelihood ratio after each attempt. A Sequential Probability Ratio Test can stop as soon as evidence crosses a decision threshold; Wald and Wolfowitz established its expected-sample efficiency under the classical assumptions.[8]

In practice:

- repeated identical certificate error: rapidly favor deterministic/configuration failure;
- sporadic connection reset with healthy later calls: favor transient failure;
- same test assertion after code changes: favor unresolved logic/oracle failure;
- different failures after each retry: raise context or architecture uncertainty.

### 3.5 Detect regime changes, not only thresholds

Use CUSUM or Bayesian change-point detection on:

- user correction rate;
- verifier failure signatures;
- edit-without-closure cycles;
- subagent join yield;
- provider latency/error rate;
- architecture hotspot churn.

The question is not merely “is the current rate high?” but “did the process move into a worse regime?” Page’s original CUSUM work provides the statistical-control foundation.[11]

### 3.6 Treat the supervisor as a POMDP approximation

Long-running coding work is partially observed. The true requirement state, root cause, and future action outcome are not directly visible. Actions can both advance work and acquire information. This is the structure of a partially observable Markov decision process: maintain a belief state and choose actions for both progress and information gain.[4]

Do not attempt an exact POMDP solver. Use the abstraction operationally:

```text
belief state
  → hard safety/policy constraints
  → generate candidate actions
  → estimate outcome and information gain
  → remove dominated actions
  → choose a receding-horizon action
  → observe result and update belief
```

This yields a concrete reason to add `RunProbe`: sometimes the optimal next action is not another code edit but a cheap discriminating experiment.

## 4. Pareto analysis and multiobjective control

### 4.1 Pareto principle: the edit distribution is concentrated, but not 80/20

The top 20% of paths account for roughly 73% of canonical edit events, while 30% of paths are needed to account for 80%. Therefore, “focus on the top 20%” is directionally useful but mathematically inaccurate for this corpus.

More important than raw edit count is **centrality × rework × requirement ambiguity**. A file edited often because it is a generated manifest is different from a domain service repeatedly changed because the model is unstable.

### 4.2 Pareto optimality: do not collapse decisions into one score too early

An exploratory turn-level proxy analysis compared 122 implementation turns on three quality proxies and four cost dimensions. Under coarse task/scope buckets, 65 turns (53.3%) were dominated by another observed turn. This is not a causal agent ranking, but it shows that the history contains strategies that consumed more resources without improving the observed quality proxies.

For each proposed control action, estimate a vector:

```rust
pub struct ActionValue {
    pub expected_requirement_closure: f64,
    pub expected_information_gain: f64,
    pub expected_verification_gain: f64,
    pub expected_architecture_debt_delta: f64,
    pub expected_user_interruptions: f64,
    pub expected_latency: f64,
    pub expected_compute_cost: f64,
    pub rollback_risk: f64,
}
```

Decision procedure:

1. Apply hard safety, authority, privacy, and user-policy constraints.
2. Remove actions dominated on all relevant dimensions.
3. Select from the remaining Pareto frontier according to current mission priorities.
4. Preserve frontier diversity when uncertainty is high.

NSGA-II is a classic example of nondominated sorting for multiobjective optimization.[5] The monitor does not need an evolutionary algorithm initially; simple pairwise dominance is sufficient for a small action set.

### 4.3 Example

Suppose the current options are:

| Action | Closure | Information | User cost | Time | Architecture risk |
|---|---:|---:|---:|---:|---:|
| Retry same agent | medium | low | low | low | medium |
| Run browser probe | low | high | none | low | none |
| Spawn 12 agents | high theoretical | medium | none | high | high |
| Ask user | high if answered | high | high | medium | none |
| Fresh agent with bounded handoff | medium-high | medium | none | medium | low |

Given the observed history, “spawn 12 agents” is often dominated by a smaller bounded decomposition; “ask user” is dominated when a browser or repository probe can resolve the issue; “retry same agent” is dominated when the failure signature is unchanged and context health is poor.

## 5. Software-engineering interpretations

### 5.1 Requirements engineering: build a live traceability graph

NASA systems-engineering guidance emphasizes clear requirements, documented design decisions, and complete traceability.[1] For coding agents, represent:

```rust
pub struct RequirementNode {
    pub id: RequirementId,
    pub statement: String,
    pub kind: RequirementKind,
    pub authority: Authority,
    pub source: EvidenceRef,
    pub status: RequirementStatus,
    pub acceptance: Vec<AcceptanceCriterion>,
    pub dependencies: Vec<RequirementId>,
    pub conflicts: Vec<RequirementId>,
    pub supersedes: Vec<RequirementId>,
    pub rejected_alternatives: Vec<RejectedAlternative>,
}

pub enum RequirementKind {
    Functional,
    QualityAttribute,
    ArchitectureConstraint,
    ValidationObligation,
    UserGovernancePreference,
    RejectedAlternative,
}
```

The key addition is `RejectedAlternative`. Long-running agents need negative design memory as much as positive design memory.

### 5.2 Architecture: measure change amplification

Define:

```text
change_amplification = weighted impacted components / accepted requirement units
```

Weight components by layer, runtime, ownership, and co-change centrality. A small UI request that touches portal, controller, core ingestion, SQL schema, and e2e tests has high amplification. The monitor should pause before editing and require an architecture hypothesis.

High-centrality files can also indicate missing abstractions or god classes. `KnowledgeBaseService.java` appearing in 24 segments across nine active days is a candidate for responsibility decomposition, but the monitor should not recommend refactoring solely from count. It should combine churn, defect linkage, dependency centrality, and requirement diversity.

### 5.3 Verification and validation matrix

Each requirement should name its evidence modes:

| Requirement type | Minimum evidence |
|---|---|
| Compiler/type safety | build/compile |
| Backend business logic | unit/integration/contract test |
| API behavior | live request against current service |
| UI navigation | browser route and console check |
| Visual layout | screenshot/visual comparison and interaction |
| Multimodal parsing | real representative artifact and output assertions |
| Deployment portability | clean Windows and Linux procedure or reproducible container |
| Performance | measured workload and threshold |
| Security/configuration | policy/static/runtime check |

A green build is one cell, not universal proof.

### 5.4 Test engineering: protect the oracle

Classify test changes:

- new test for an accepted requirement;
- repair of a broken test harness;
- expectation change authorized by a changed requirement;
- fixture/data update;
- assertion weakening;
- snapshot refresh;
- deletion or skip.

The last three should receive elevated scrutiny. Add mutation or metamorphic checks around high-risk logic, particularly retrieval, parser selection, workflow inheritance, and preview conversion.

### 5.5 Process mining: conformance, not vanity metrics

Define expected process models by task class. For a UI defect:

```text
reproduce → localize → edit → build → browser validate → close requirement
```

For an operational outage:

```text
classify layer → probe health → remediate config/service → retest original operation
```

A conformance checker can flag:

- completion before reproduction;
- UI closure without browser validation;
- verification followed by edits without re-verification;
- repeated identical failures without diagnosis change;
- spawned workers without join outcomes;
- test-oracle edits without authority.

### 5.6 Safety engineering: model unsafe control actions

STPA treats accidents as control problems and asks whether a control action is unsafe because it is not provided, provided incorrectly, mistimed, or applied too long.[12] The analogy is strong for a coding supervisor.

Unsafe monitor actions include:

- allowing completion when required evidence is stale;
- forcing retry during an authentication/configuration failure;
- switching agents without preserving design constraints;
- spawning workers with overlapping file ownership;
- asking the user after an explicit autonomy preference;
- allowing an agent to change the test oracle without authority;
- stopping a verifier too early;
- continuing edits after the failure hypothesis has stopped changing.

This gives a more concrete safety analysis than “high entropy.”

### 5.7 Distributed systems: causality, leases, and idempotency

Multiple agents and worktrees make the repository a distributed shared state system. Apply familiar mechanisms:

- **causal IDs:** every mutation, verifier result, claim, and intervention references run/agent/worktree/commit;
- **ownership leases:** an agent temporarily owns a file or bounded component;
- **idempotency keys:** prevent duplicated tool actions and repeated migrations;
- **join states:** integrated, rejected, superseded, failed, timed out;
- **vector/epoch semantics:** verifier evidence is valid only for the tree/epoch it observed;
- **sagas/compensation:** multi-step refactors define rollback points and compensating actions.

This is essential for outcome attribution.

## 6. Concrete control architecture

### 6.1 Evidence ledger

```rust
pub struct EvidenceRecord {
    pub id: EvidenceId,
    pub run_id: RunId,
    pub agent_id: AgentId,
    pub parent_agent_id: Option<AgentId>,
    pub observed_at: DateTime<Utc>,
    pub repo_anchor: Option<RepoAnchor>,
    pub requirement_ids: Vec<RequirementId>,
    pub provenance: Provenance,
    pub confidence: f64,
    pub payload: EvidenceKind,
}
```

Raw transcript text can remain ephemeral or encrypted. Persist typed evidence, hashes, bounded excerpts, and provenance.

### 6.2 Completion certificate

```rust
pub struct CompletionCertificate {
    pub scoped_requirements: Vec<RequirementId>,
    pub closed_requirements: Vec<RequirementId>,
    pub unresolved_requirements: Vec<RequirementId>,
    pub verification: Vec<VerificationCertificate>,
    pub validation: Vec<ValidationCertificate>,
    pub current_repo_anchor: RepoAnchor,
    pub unresolved_agents: Vec<AgentId>,
    pub unresolved_incidents: Vec<IncidentId>,
    pub test_oracle_changes: Vec<TestOracleChange>,
    pub confidence: f64,
}
```

Hard rule:

```text
“all done” is illegal unless
  scoped requirements are closed or explicitly deferred
  AND relevant evidence is fresh
  AND all workers have terminal integration states
  AND no unresolved blocking incident exists
  AND test-oracle changes have authority
```

### 6.3 Belief state and probes

```rust
pub struct BeliefState {
    pub hypotheses: Vec<HypothesisBelief>,
    pub requirement_uncertainty: f64,
    pub attribution_uncertainty: f64,
    pub environment_uncertainty: f64,
    pub model_version: String,
}

pub enum ProbeSpec {
    ReproduceUserPath { route: String },
    InspectCurrentService { service: String },
    VerifyRepoAnchor,
    RunTargetedTest { target: String },
    ParseAcceptanceArtifact { artifact: ArtifactId },
    InspectCallGraph { symbol: String },
    CompareRequirementToDiff { requirement: RequirementId },
}
```

A probe should be selected by expected information gain and cost.

### 6.4 Subagent lifecycle

```rust
pub struct AgentLease {
    pub agent_id: AgentId,
    pub task_ids: Vec<RequirementId>,
    pub owned_paths: Vec<RepoPath>,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub expected_result_schema: String,
    pub state: AgentLeaseState,
}

pub enum AgentLeaseState {
    Running,
    Joined,
    Integrated,
    Rejected,
    Superseded,
    Failed,
    TimedOut,
    Cancelled,
}
```

Adaptive fan-out policy:

```text
initial cap = 2–4
increase only if:
  decomposition independence is high
  path overlap is low
  recent terminal-result yield is healthy
  integration queue is short
  provider health is stable
otherwise decrease cap
```

### 6.5 Action selection

```rust
pub struct ActionCandidate {
    pub action: ControlAction,
    pub preconditions: Vec<PolicyPredicate>,
    pub value: ActionValue,
    pub predicted_success: f64,
    pub prediction_interval: (f64, f64),
    pub evidence: Vec<EvidenceId>,
}
```

Controller:

```text
1. Reconstruct authoritative scope and current repo anchor.
2. Update hypotheses from new evidence.
3. Apply hard policy and safety constraints.
4. Generate Continue / Probe / Verify / Retry / Intervene / Switch / Spawn / Ask / Abort candidates.
5. Remove dominated candidates.
6. Use the LLM advisor only to discriminate among allowed frontier candidates.
7. Validate the proposal deterministically.
8. Execute one bounded action.
9. Record outcome and update calibrated models.
```

## 7. Hard policies justified by this history

### Completion

- Block broad completion if any authoritative requirement is open, ambiguous, or lacks evidence.
- Block completion when relevant repository changes occurred after the verifier’s anchor.
- Block completion while spawned workers lack terminal integration states.
- Downgrade wording automatically: “implemented but not browser-validated,” not “fully fixed.”

### UI

- Any UI-affecting diff requires a browser route check against the current build.
- Layout or visual requirements require screenshot evidence or explicit human acceptance.
- A frontend build alone cannot close a UI requirement.

### Tests

- Changes to assertions, snapshots, expected values, fixtures, or skips require `SpecAuthority`.
- Tests changed in the same turn cannot be the sole evidence that behavior is correct.
- High-risk parser/retrieval behavior should receive mutation or metamorphic checks.

### Subagents

- Enforce an adaptive WIP cap.
- Prevent overlapping path leases unless explicitly coordinated.
- Require result schemas and join outcomes.
- Do not start another workflow batch while the integration queue exceeds capacity.

### Questions

- Preserve explicit user governance preferences as hard policy.
- Run a value-of-information check before asking.
- Routine sequencing, file choice, testing, or debugging questions are resolved locally.
- Ask only for preferences, authority, credentials, irreversible choices, or genuinely unavailable business facts.

### Retry and switching

- Classify failure layer before retry.
- Maintain per-signature retry likelihood and sequential stopping thresholds.
- Switching agents on the same unhealthy provider is not diversification.
- A fresh agent must receive authoritative constraints, rejected alternatives, current repo anchor, unresolved requirements, and verified evidence—not a prose dump.

### Architecture

- High-centrality hotspot edits require an impact map.
- Cross-layer changes require contract/integration evidence.
- New domain concepts require explicit authority before implementation.

## 8. Metrics that should replace “entropy score” dashboards

| Metric | Definition | Failure it catches |
|---|---|---|
| Verified Requirement Closure Rate | Requirements closed with fresh evidence / authoritative scoped requirements | False completion |
| Completion Claim Precision | Completion claims surviving next review / claims | Overclaiming |
| Fresh Verification Coverage | Weighted changed surface covered by fresh verifier evidence | Stale tests |
| Validation Coverage | User workflows/visual requirements with intended-environment evidence | Build-as-validation |
| Requirement Recall | Authoritative requirements represented in current case file | Context loss |
| Rejected-Alternative Recurrence | Reintroduced rejected designs per run | Memory failure |
| User Interruption Regret | Questions later shown answerable by local probes | Annoying escalation |
| Subagent Join Yield | Integrated terminal results / agents started | Fan-out overload |
| Integration Queue Age | Time/results waiting for main-agent integration | Coordination bottleneck |
| Change Amplification | Weighted affected components / requirement units | Architecture drift |
| Test-Oracle Mutation Rate | Authority-sensitive test edits / implementation turns | Test laundering |
| Attribution Completeness | Outcomes linked to exact mutations/agents/anchors | Causal ambiguity |
| Probability Calibration | Brier score and reliability by task stratum | Misleading advisor confidence |
| Scope Volatility | Authorized requirement additions/removals per unit time | Unfair outcome attribution |

A dashboard should show these as a vector. It may also show entropy of the belief distribution, but not replace the vector with one scalar.

## 9. Recommended implementation slices

### Slice 1: Offline history compiler

- Parse Codex, Claude Code, Pi, and OpenCode histories.
- Reconstruct sessions, parent/subagent topology, tool attempts/outcomes, and interruptions.
- Canonicalize repository paths and worktrees.
- Emit typed observations with provenance and deduplication.
- Redact or hash raw private content.

Deliverable: repeatable case reconstruction and the same analyses used in this report.

### Slice 2: Requirement and authority ledger

- Extract candidate requirements from user text, documents, screenshots, and annotations.
- Store accepted, rejected, superseded, and proposed states.
- Add explicit acceptance criteria and evidence links.
- Preserve user governance preferences.

Deliverable: a live requirement graph and bounded case file.

### Slice 3: Repository epochs and evidence freshness

- Assign a monotonic mutation epoch.
- Anchor every verifier to commit/tree/epoch and covered paths.
- Invalidate evidence after relevant changes.
- Implement the completion certificate.

Deliverable: deterministic stale-verification and false-closure prevention.

### Slice 4: Agent lifecycle and WIP controller

- Add leases, path ownership, result schemas, join barriers, and integration queue.
- Start with a cap of 2–4 workers and adapt from join yield and overlap.
- Record every spawn-to-outcome transition.

Deliverable: controlled parallelism instead of unbounded fan-out.

### Slice 5: Failure taxonomy and sequential retry

- Separate provider, transport, auth, rate limit, shell, repository, verifier, test-oracle, and logic failures.
- Fingerprint errors.
- Maintain posteriors and sequential retry/stop decisions.

Deliverable: no more retrying the wrong layer or switching agents pointlessly.

### Slice 6: Process conformance and probes

- Define task-class process models.
- Detect missing reproduction, verification-after-edit gaps, repeated unchanged failure signatures, and unjoined workers.
- Add deterministic probes as first-class actions.

Deliverable: active uncertainty reduction before intervention.

### Slice 7: Pareto action policy and bounded LLM advisor

- Compute action value vectors.
- Apply hard constraints and remove dominated actions.
- Let the LLM rank only the allowed frontier and explain the dominant uncertainty.
- Validate all packets deterministically.

Deliverable: multiobjective control without giving the LLM unrestricted authority.

### Slice 8: Outcome learning and replay

- Define accepted outcomes and delayed regressions.
- Score probability calibration.
- Replay historical episodes under alternative policies.
- Run prospective experiments on WIP limits, completion certificates, and question gates.

Deliverable: evidence that the monitor improves outcomes, not merely produces diagnoses.

## 10. Highest-value experiments

1. **Completion-certificate experiment:** compare correction/reopen rates before and after blocking broad completion without requirement/evidence closure.
2. **WIP-cap experiment:** compare 2–4 workers against 10+ workers on matched task decompositions; measure join yield, cycle time, overlap, and accepted closure.
3. **UI evidence experiment:** require browser validation for UI diffs and measure immediate user corrections.
4. **Question-gate experiment:** run VOI/local-probe policy and measure interruption regret without increasing harmful autonomous decisions.
5. **Test-authority experiment:** gate expectation changes and measure regressions plus legitimate update latency.
6. **Requirement-memory experiment:** inject rejected alternatives and governance preferences into fresh-agent handoffs; measure recurrence.
7. **Causal attribution experiment:** require commit/worktree/agent anchors for every verifier and compare diagnosis accuracy.

For inference, use project/task-matched comparisons and hierarchical models. Do not compare raw agent averages across different task mixes.

## 11. Final architectural conclusion

The original “entropy controller” idea becomes production-grade when reframed as four coupled systems:

```text
1. Requirements and authority system
   What is actually required, rejected, or undecided?

2. Belief and evidence system
   What hypotheses explain the current state, with what probabilities?

3. Multiobjective control system
   Which allowed action best trades closure, information, cost, risk, and user burden?

4. Causal outcome system
   What action changed what state, and did it improve accepted outcomes?
```

The transcript history shows that the largest failures happen at the boundaries among these systems. Agents write competent local patches but lose the global contract. They validate the artifact they happened to create rather than the user workflow. They parallelize discovery faster than they can integrate evidence. They modify test or design assumptions without a durable authority chain. They then declare closure based on activity.

The monitor’s core product proposition should therefore be:

> **An external control plane that turns fragmented agent activity into traceable, probabilistically calibrated, verified requirement closure.**

That is more defensible and measurable than “an LLM that judges logs,” and more precise than “an entropy monitor.”

## References

[1] NASA, *NASA Systems Engineering Handbook, Rev. 2*; includes requirements traceability and the distinction between product verification and product validation. https://www.nasa.gov/wp-content/uploads/2018/09/nasa_systems_engineering_handbook_0.pdf

[2] J. D. C. Little, “A Proof for the Queuing Formula: L = λW,” *Operations Research*, 1961. https://www.isye.gatech.edu/~spyros/courses/IE7201/Fall-13/Little-OR-paper.pdf

[3] IEEE Task Force on Process Mining, *Process Mining Manifesto*, 2011. https://www.tf-pm.org/upload/1580737614108.pdf

[4] L. P. Kaelbling, M. L. Littman, and A. R. Cassandra, “Planning and Acting in Partially Observable Stochastic Domains,” *Artificial Intelligence*, 1998. https://people.csail.mit.edu/lpk/papers/aij98-pomdp.pdf

[5] K. Deb, A. Pratap, S. Agarwal, and T. Meyarivan, “A Fast and Elitist Multiobjective Genetic Algorithm: NSGA-II,” *IEEE Transactions on Evolutionary Computation*, 2002. https://ieeexplore.ieee.org/document/996017

[6] R. A. Howard, “Information Value Theory,” *IEEE Transactions on Systems Science and Cybernetics*, 1966.

[7] G. W. Brier, “Verification of Forecasts Expressed in Terms of Probability,” *Monthly Weather Review*, 1950. https://journals.ametsoc.org/view/journals/mwre/78/1/1520-0493_1950_078_0001_vofeit_2_0_co_2.xml

[8] A. Wald and J. Wolfowitz, “Optimum Character of the Sequential Probability Ratio Test,” *Annals of Mathematical Statistics*, 1948. https://projecteuclid.org/journals/annals-of-mathematical-statistics/volume-19/issue-3/Optimum-Character-of-the-Sequential-Probability-Ratio-Test/10.1214/aoms/1177730197.full

[9] Y. Jia and M. Harman, “An Analysis and Survey of the Development of Mutation Testing,” *IEEE Transactions on Software Engineering*, 2011. https://doi.org/10.1109/TSE.2010.62

[10] T. Y. Chen, S. C. Cheung, and S. M. Yiu, “Metamorphic Testing: A New Approach for Generating Next Test Cases,” HKUST Technical Report, 1998. https://www.cse.ust.hk/faculty/scc/publ/CS98-01-metamorphictesting.pdf

[11] E. S. Page, “Continuous Inspection Schemes,” *Biometrika*, 1954. https://doi.org/10.1093/biomet/41.1-2.100

[12] N. Leveson and J. Thomas, *STPA Handbook / STPA Primer*. https://psas.scripts.mit.edu/home/wp-content/uploads/2013/10/An-STPA-Primer-version-0-4.pdf
