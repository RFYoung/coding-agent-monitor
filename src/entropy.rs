//! Entropy scoring: map a bounded case file plus dev-history priors to the seven-dimension operational-risk vector that gates control actions.

use crate::*;

pub(crate) struct EntropyScoringInput<'a> {
    pub(crate) snapshot: &'a DashboardSnapshot,
    pub(crate) intent_events: &'a [Event],
    pub(crate) task: &'a TaskSummary,
    pub(crate) verification: &'a VerificationSummary,
    pub(crate) durable_memory: &'a [MemoryCandidate],
    pub(crate) repo_audit: Option<&'a RepoAuditReport>,
    pub(crate) policy: &'a PolicyConfig,
    pub(crate) security: &'a SecurityConfig,
}

pub(crate) fn score_entropy(input: EntropyScoringInput<'_>) -> EntropyVector {
    let EntropyScoringInput {
        snapshot,
        intent_events,
        task,
        verification,
        durable_memory,
        repo_audit,
        policy,
        security,
    } = input;
    let mut vector = EntropyVector::baseline();

    if task.user_goal.is_none() && task.acceptance_criteria.is_empty() {
        vector.raise(
            EntropyKind::Goal,
            65,
            75,
            "no current user goal captured",
            None,
            Some("current user goal or acceptance criteria".into()),
        );
    }

    for marker in &task.ambiguity_markers {
        vector.raise(
            EntropyKind::Goal,
            82,
            80,
            "user goal contains unresolved ambiguity",
            marker.source_event_id.clone(),
            Some("clarified acceptance criteria or bounded user decision".into()),
        );
    }

    let mut latest_source_write: Option<((i64, usize), String)> = None;
    let mut latest_passing_verifier: Option<(i64, usize)> = None;
    let mut latest_failed_verifier: Option<((i64, usize), String, String, String)> = None;
    let mut failing_commands = HashMap::<(String, String, FailureLayer), (usize, String)>::new();
    let mut service_failures = HashMap::<(String, FailureLayer), (usize, String)>::new();
    let mut permission_denials = HashMap::<String, (usize, String)>::new();
    let mut permission_requests = HashMap::<String, (usize, String)>::new();
    let mut inspection_loops = HashMap::<(String, String), (usize, String)>::new();
    let mut unresolved_subagents = HashMap::<String, (usize, String)>::new();
    let mut unresolved_subagent_paths = HashMap::<(String, String), (usize, String)>::new();
    let mut verifier_failure_loops = HashMap::<(String, String), VerifierFailureLoop>::new();
    let mut latest_domain_validation_write: Option<(
        (i64, usize),
        String,
        String,
        ValidationSurface,
    )> = None;
    let mut latest_domain_validation_pass = HashMap::<ValidationSurface, (i64, usize)>::new();
    let mut latest_completion_claim: Option<((i64, usize), String, String)> = None;
    let bug_fix_goal = task_suggests_bug_fix(task);
    let mut bug_fix_probe_seen = false;
    let mut bug_fix_pre_edit_gap_recorded = false;
    let rejected_alternatives =
        rejected_alternatives_from_intent_and_memory(intent_events, durable_memory);

    for (index, event) in snapshot.recent_events.iter().enumerate() {
        let evidence_id = event
            .event_id
            .clone()
            .unwrap_or_else(|| format!("event-{}", index + 1));
        let time = event.time.as_deref().and_then(parse_utc_seconds);
        let content = event.content.as_deref().unwrap_or_default();

        if event_is_change_like(event) {
            if bug_fix_goal
                && !bug_fix_probe_seen
                && !bug_fix_pre_edit_gap_recorded
                && event
                    .file
                    .as_deref()
                    .is_some_and(|file| is_verification_relevant_file(file, policy))
            {
                vector.raise(
                    EntropyKind::Plan,
                    72,
                    82,
                    "bug-fix edit happened before reproduction or localization evidence",
                    Some(evidence_id.clone()),
                    Some(
                        "reproduction, failing verifier, or localization probe before more edits"
                            .into(),
                    ),
                );
                bug_fix_pre_edit_gap_recorded = true;
            }
            for ((agent, _), loop_state) in verifier_failure_loops.iter_mut() {
                if agent == &event.agent {
                    loop_state.edits_since_last_failure += 1;
                }
            }
            if let Some(file) = event.file.as_deref()
                && test_oracle_change_lacks_authority(event, file)
            {
                vector.raise(
                    EntropyKind::Verification,
                    83,
                    85,
                    format!(
                        "test oracle change `{file}` lacks authority and independent behavior evidence"
                    ),
                    Some(evidence_id.clone()),
                    Some(
                        "spec authority plus independent behavior evidence for the test oracle change"
                            .into(),
                    ),
                );
            }
            if let Some(cause) = event
                .file
                .as_deref()
                .and_then(|file| security_path_user_decision_cause(file, security))
            {
                vector.raise(
                    EntropyKind::UserDecision,
                    88,
                    90,
                    cause,
                    Some(evidence_id.clone()),
                    Some("user authorization or required external input".into()),
                );
            }
            if event
                .file
                .as_deref()
                .is_some_and(|file| is_verification_relevant_file(file, policy))
            {
                let order = (time.unwrap_or(i64::MAX), index);
                if latest_source_write
                    .as_ref()
                    .is_none_or(|(current, _)| order > *current)
                {
                    latest_source_write = Some((order, evidence_id.clone()));
                }
                if policy.require_verification_after_source_change
                    && let Some(file) = event.file.as_deref()
                    && let Some(surface) = validation_surface_for_path(file)
                    && latest_domain_validation_write
                        .as_ref()
                        .is_none_or(|(current, _, _, _)| order > *current)
                {
                    latest_domain_validation_write =
                        Some((order, evidence_id.clone(), file.to_string(), surface));
                }
            }
            if event
                .rationale
                .as_deref()
                .is_none_or(|rationale| rationale.trim().is_empty())
            {
                vector.raise(
                    EntropyKind::RepoBlame,
                    75,
                    85,
                    "file change lacks rationale",
                    Some(evidence_id.clone()),
                    Some("rationale linked to file change".into()),
                );
            }
        }

        if event_records_failure_hypothesis(event, content) {
            for ((agent, _), loop_state) in verifier_failure_loops.iter_mut() {
                if agent == &event.agent {
                    loop_state.hypothesis_since_last_failure = true;
                }
            }
        }

        if matches!(event.kind, EventKind::CommandResult | EventKind::TestResult)
            && event.exit_code == Some(0)
        {
            let order = (time.unwrap_or(i64::MAX), index);
            for surface in validation_surfaces_for_event(event) {
                let entry = latest_domain_validation_pass
                    .entry(surface)
                    .or_insert(order);
                if order > *entry {
                    *entry = order;
                }
            }
        }

        if event_is_verification_result(event) {
            match event.exit_code {
                Some(0) => {
                    verifier_failure_loops.retain(|(agent, _), _| agent != &event.agent);
                    if let Some(time) = time {
                        let order = (time, index);
                        latest_passing_verifier = Some(
                            latest_passing_verifier
                                .map(|current| current.max(order))
                                .unwrap_or(order),
                        );
                    }
                }
                Some(_) => {
                    if let Some(signature) = verifier_failure_signature(event) {
                        let entry = verifier_failure_loops
                            .entry((event.agent.clone(), signature))
                            .or_default();
                        if !entry.evidence_id.is_empty()
                            && entry.edits_since_last_failure > 0
                            && !entry.hypothesis_since_last_failure
                        {
                            entry.repeated_after_edits += 1;
                        }
                        entry.command = event.command.clone().unwrap_or_default();
                        entry.evidence_id = evidence_id.clone();
                        entry.edits_since_last_failure = 0;
                        entry.hypothesis_since_last_failure = false;
                    }
                    let failure_order = (time.unwrap_or(i64::MAX), index);
                    if latest_failed_verifier
                        .as_ref()
                        .is_none_or(|(current, _, _, _)| failure_order > *current)
                    {
                        latest_failed_verifier = Some((
                            failure_order,
                            evidence_id.clone(),
                            "verification command failed".into(),
                            "passing verification result".into(),
                        ));
                    }
                }
                None => {}
            }
        }

        if event.kind == EventKind::CommandResult && event.exit_code == Some(0) {
            failing_commands.retain(|(agent, _, _), _| agent != &event.agent);
        }

        if event.kind == EventKind::CommandResult
            && let Some(command) = event.command.as_deref().map(normalize_command_signature)
            && !is_verification_command(&command)
        {
            let key = (event.agent.clone(), command);
            match event.exit_code {
                Some(0) => {}
                Some(_) => {
                    let layer = classify_command_failure_layer(event, content);
                    let entry = failing_commands
                        .entry((key.0, key.1, layer))
                        .or_insert((0, evidence_id.clone()));
                    entry.0 += 1;
                    entry.1 = evidence_id.clone();
                }
                None => {}
            }
        }

        if let Some(layer) = classify_service_failure_layer(content) {
            let entry = service_failures
                .entry((event.agent.clone(), layer))
                .or_insert((0, evidence_id.clone()));
            entry.0 += 1;
            entry.1 = evidence_id.clone();
        } else if event_can_clear_service_failure(event, content) {
            service_failures.retain(|(agent, _), _| agent != &event.agent);
        }
        if looks_like_permission_denial(content) {
            let entry = permission_denials
                .entry(event.agent.clone())
                .or_insert((0, evidence_id.clone()));
            entry.0 += 1;
            entry.1 = evidence_id.clone();
        } else if looks_like_permission_request(content) {
            let entry = permission_requests
                .entry(event.agent.clone())
                .or_insert((0, evidence_id.clone()));
            entry.0 += 1;
            entry.1 = evidence_id.clone();
        } else if event_can_clear_service_failure(event, content) {
            permission_denials.remove(&event.agent);
            permission_requests.remove(&event.agent);
        }
        if event_breaks_rediscovery_loop(event) {
            inspection_loops.retain(|(agent, _), _| agent != &event.agent);
        } else if let Some(target) = inspection_loop_target(event) {
            let entry = inspection_loops
                .entry((event.agent.clone(), target))
                .or_insert((0, evidence_id.clone()));
            entry.0 += 1;
            entry.1 = evidence_id.clone();
        }
        if let Some(action) = subagent_lifecycle_action(event) {
            let entry = unresolved_subagents
                .entry(event.agent.clone())
                .or_insert((0, evidence_id.clone()));
            match action {
                SubagentLifecycleAction::Spawned => {
                    entry.0 += 1;
                    entry.1 = evidence_id.clone();
                    for path in subagent_ownership_paths(event) {
                        let path_entry = unresolved_subagent_paths
                            .entry((event.agent.clone(), path))
                            .or_insert((0, evidence_id.clone()));
                        path_entry.0 += 1;
                        path_entry.1 = evidence_id.clone();
                    }
                }
                SubagentLifecycleAction::Terminal => {
                    entry.0 = entry.0.saturating_sub(1);
                    if entry.0 == 0 {
                        entry.1 = evidence_id.clone();
                    }
                    unresolved_subagent_paths.retain(|(agent, _), _| agent != &event.agent);
                }
            }
        }
        if looks_like_unverified_completion(content) {
            vector.raise(
                EntropyKind::Verification,
                90,
                90,
                "agent claimed completion without verification",
                Some(evidence_id.clone()),
                Some("passing verification result after completion claim".into()),
            );
        }
        if looks_like_completion_claim(content) {
            let order = (time.unwrap_or(i64::MAX), index);
            if latest_completion_claim
                .as_ref()
                .is_none_or(|(current, _, _)| order > *current)
            {
                latest_completion_claim = Some((order, evidence_id.clone(), event.agent.clone()));
            }
        }
        if looks_like_premature_stop(content) {
            vector.raise(
                EntropyKind::Plan,
                70,
                80,
                "agent attempted to stop while obvious work remained",
                Some(evidence_id.clone()),
                Some("next concrete action".into()),
            );
        }
        if event_asks_routine_user_question(event, content) {
            vector.raise(
                EntropyKind::Plan,
                74,
                84,
                "agent asked a routine user question before exhausting local probes",
                Some(evidence_id.clone()),
                Some("local probe or obvious next step before user interruption".into()),
            );
        }
        if looks_like_forgetting_design_memory(content) {
            vector.raise(
                EntropyKind::Context,
                85,
                90,
                "agent appears to have lost durable design memory",
                Some(evidence_id.clone()),
                Some("fresh handoff case file".into()),
            );
        }
        if looks_like_context_compaction(content) {
            vector.raise(
                EntropyKind::Context,
                85,
                90,
                "agent context was compacted or summarized",
                Some(evidence_id.clone()),
                Some("fresh handoff case file".into()),
            );
        }
        if looks_like_session_error(content) {
            vector.raise(
                EntropyKind::AgentHealth,
                85,
                85,
                "agent session reported a lifecycle error",
                Some(evidence_id.clone()),
                Some("recovered agent session or fallback agent".into()),
            );
        }
        if let Some(cause) = user_decision_cause_for_event(event, content) {
            vector.raise(
                EntropyKind::UserDecision,
                85,
                90,
                cause,
                Some(evidence_id.clone()),
                Some("user authorization or required external input".into()),
            );
        }
        if let Some(rejected) = event_reintroduces_rejected_alternative(
            event,
            &evidence_id,
            content,
            &rejected_alternatives,
        ) {
            let cause = format!(
                "{} reintroduced rejected alternative `{}`",
                event.agent, rejected.subject
            );
            vector.raise(
                EntropyKind::Context,
                82,
                86,
                cause.clone(),
                Some(evidence_id.clone()),
                Some("user authorization to revisit the rejected alternative".into()),
            );
            vector.raise(
                EntropyKind::Plan,
                72,
                82,
                cause,
                Some(evidence_id.clone()),
                Some("revise plan to honor rejected-alternative memory".into()),
            );
        }
        if bug_fix_goal && event_establishes_bug_reproduction_or_localization(event, content) {
            bug_fix_probe_seen = true;
        }
    }

    if policy.require_verification_after_source_change
        && let Some((write_order, evidence_id, path, surface)) =
            latest_domain_validation_write.as_ref()
        && latest_domain_validation_pass
            .get(surface)
            .is_none_or(|pass_order| pass_order < write_order)
    {
        vector.raise(
            EntropyKind::Verification,
            82,
            84,
            format!(
                "{} `{path}` lacks intended-environment validation",
                surface.change_label()
            ),
            Some(evidence_id.clone()),
            Some(surface.missing_evidence().into()),
        );
    }

    for ((agent, command, layer), (count, evidence_id)) in failing_commands {
        if count >= 3 {
            vector.raise(
                EntropyKind::AgentHealth,
                82,
                88,
                format!(
                    "{agent} repeated failing command `{command}` {count} times at {} layer",
                    layer.as_str()
                ),
                Some(evidence_id),
                Some(format!(
                    "loop-breaking retry packet after {}-layer diagnosis",
                    layer.as_str()
                )),
            );
        }
    }

    for ((agent, layer), (count, evidence_id)) in service_failures {
        if count >= 3 {
            vector.raise(
                EntropyKind::AgentHealth,
                90,
                90,
                format!(
                    "{agent} hit repeated service failures at {} layer",
                    layer.as_str()
                ),
                Some(evidence_id),
                Some(format!(
                    "{}-layer recovery evidence before retry or fallback",
                    layer.as_str()
                )),
            );
        }
    }

    for (agent, (count, evidence_id)) in permission_denials {
        if count >= 2 {
            vector.raise(
                EntropyKind::AgentHealth,
                80,
                85,
                format!("{agent} hit repeated permission denials"),
                Some(evidence_id),
                Some("permission-aware retry packet".into()),
            );
        }
    }

    for (agent, (count, evidence_id)) in permission_requests {
        if count >= 2 {
            vector.raise(
                EntropyKind::AgentHealth,
                78,
                85,
                format!("{agent} hit repeated permission requests"),
                Some(evidence_id),
                Some("permission-aware retry packet".into()),
            );
        }
    }

    for ((agent, target), (count, evidence_id)) in inspection_loops {
        if count >= 4 {
            let cause =
                format!("{agent} repeatedly inspected `{target}` {count} times without progress");
            vector.raise(
                EntropyKind::Context,
                78,
                80,
                cause.clone(),
                Some(evidence_id.clone()),
                Some("new hypothesis, edit, or verification for the inspected target".into()),
            );
            vector.raise(
                EntropyKind::Plan,
                65,
                75,
                cause,
                Some(evidence_id),
                Some("new hypothesis, edit, or verification before more broad search".into()),
            );
        }
    }

    for ((agent, _signature), loop_state) in verifier_failure_loops {
        if loop_state.repeated_after_edits > 0 {
            let command = if loop_state.command.trim().is_empty() {
                "verifier".into()
            } else {
                format!("`{}`", loop_state.command)
            };
            let cause = format!(
                "{agent} saw the same verifier failure signature recur in {command} after edits without a new hypothesis"
            );
            vector.raise(
                EntropyKind::Verification,
                86,
                88,
                cause.clone(),
                Some(loop_state.evidence_id.clone()),
                Some("failure hypothesis or isolation probe before more edits".into()),
            );
            vector.raise(
                EntropyKind::Plan,
                72,
                82,
                cause,
                Some(loop_state.evidence_id),
                Some("failure hypothesis before another edit or verifier retry".into()),
            );
        }
    }

    for (agent, (count, evidence_id)) in &unresolved_subagents {
        if *count >= SUBAGENT_WIP_CAP {
            let cause = format!(
                "{agent} has {count} unresolved spawned worker(s), reaching the subagent WIP cap"
            );
            vector.raise(
                EntropyKind::Plan,
                72,
                84,
                cause.clone(),
                Some(evidence_id.clone()),
                Some("join or cancel spawned workers before starting more subagents".into()),
            );
            vector.raise(
                EntropyKind::AgentHealth,
                72,
                80,
                cause,
                Some(evidence_id.clone()),
                Some("subagent terminal outcomes before more fan-out".into()),
            );
        }
    }

    for ((agent, path), (count, evidence_id)) in &unresolved_subagent_paths {
        if *count >= 2 {
            let cause = format!(
                "{agent} has overlapping subagent path ownership for `{path}` across {count} unresolved worker(s)"
            );
            vector.raise(
                EntropyKind::Plan,
                74,
                84,
                cause.clone(),
                Some(evidence_id.clone()),
                Some("disjoint worker path ownership or terminal worker outcomes".into()),
            );
            vector.raise(
                EntropyKind::AgentHealth,
                70,
                78,
                cause,
                Some(evidence_id.clone()),
                Some("join, cancel, or reassign overlapping subagents before more fan-out".into()),
            );
        }
    }

    let verifier_sequence_base = snapshot.recent_events.len();
    for (index, run) in snapshot.recent_verifier_runs.iter().enumerate() {
        let sequence = verifier_sequence_base + index;
        let time = verifier_run_time(run);
        match run.status {
            VerificationRunStatus::Passed => {
                if let Some(time) = time {
                    let order = (time, sequence);
                    latest_passing_verifier = Some(
                        latest_passing_verifier
                            .map(|current| current.max(order))
                            .unwrap_or(order),
                    );
                }
            }
            VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => {
                let failure_order = (time.unwrap_or(i64::MAX), sequence);
                let cause = match run.status {
                    VerificationRunStatus::Failed => "verifier run failed",
                    VerificationRunStatus::TimedOut => "verifier run timed out",
                    VerificationRunStatus::Passed => unreachable!(),
                };
                if latest_failed_verifier
                    .as_ref()
                    .is_none_or(|(current, _, _, _)| failure_order > *current)
                {
                    latest_failed_verifier = Some((
                        failure_order,
                        run.verifier_run_id.clone(),
                        cause.into(),
                        "passing verifier run".into(),
                    ));
                }
            }
        }
    }

    if let Some(repo_audit) = repo_audit {
        let repo_sequence_base = snapshot.recent_events.len() + snapshot.recent_verifier_runs.len();
        for (index, change) in repo_audit.changes.iter().enumerate() {
            if !is_verification_relevant_file(&change.path, policy) {
                continue;
            }
            let order = (
                change.modified_at.unwrap_or(i64::MAX),
                repo_sequence_base + index,
            );
            if latest_source_write
                .as_ref()
                .is_none_or(|(current, _)| order > *current)
            {
                latest_source_write =
                    Some((order, format!("repo-audit-{}", safe_slug(&change.path))));
            }
        }
    }

    if policy.require_verification_after_source_change
        && let Some((write_order, evidence_id)) = latest_source_write
    {
        let stale = verification.status == VerificationStatus::Stale
            || latest_passing_verifier.is_none_or(|pass_order| pass_order < write_order);
        if stale {
            let cause = if evidence_id.starts_with("repo-audit-") {
                "dirty source/test git hunks after last passing verification"
            } else {
                "source changes after last passing verification"
            };
            vector.raise(
                EntropyKind::Verification,
                85,
                90,
                cause,
                Some(evidence_id),
                Some("passing verification after latest source change".into()),
            );
        }
    }

    let unresolved_failed_verifier =
        latest_failed_verifier
            .as_ref()
            .is_some_and(|(failure_order, _, _, _)| {
                latest_passing_verifier.is_none_or(|pass_order| pass_order <= *failure_order)
            });

    if unresolved_failed_verifier
        && let Some((_, evidence_id, cause, missing)) = latest_failed_verifier.as_ref()
    {
        vector.raise(
            EntropyKind::Verification,
            80,
            90,
            cause.clone(),
            Some(evidence_id.clone()),
            Some(missing.clone()),
        );
    }

    if unresolved_failed_verifier {
        vector.raise(
            EntropyKind::Verification,
            80,
            85,
            "failing verification has not been cleared",
            None,
            Some("later passing verification result".into()),
        );
    }

    if let Some((claim_order, evidence_id, _agent)) = latest_completion_claim.as_ref()
        && latest_passing_verifier.is_none_or(|pass_order| pass_order < *claim_order)
    {
        vector.raise(
            EntropyKind::Verification,
            84,
            86,
            "agent completion claim lacks objective verification evidence",
            Some(evidence_id.clone()),
            Some("passing verifier result after completion claim".into()),
        );
    }

    if let Some((_, completion_evidence_id, agent)) = latest_completion_claim.as_ref()
        && let Some((count, lifecycle_evidence_id)) = unresolved_subagents.get(agent)
        && *count > 0
    {
        vector.raise(
            EntropyKind::Verification,
            86,
            86,
            format!(
                "{agent} completion claim has {count} spawned worker(s) without terminal outcomes"
            ),
            Some(lifecycle_evidence_id.clone()),
            Some(
                "joined_with_summary, cancelled_with_reason, timed_out, superseded, or failed worker outcome"
                    .into(),
            ),
        );
        vector.raise(
            EntropyKind::Plan,
            76,
            82,
            "spawned worker lifecycle is unresolved at completion",
            Some(completion_evidence_id.clone()),
            Some("terminal worker outcome before completion".into()),
        );
    }

    for session in &snapshot.agent_sessions {
        match session.status {
            AgentActivityStatus::Degraded => vector.raise(
                EntropyKind::AgentHealth,
                85,
                85,
                format!("{} is degraded", session.agent),
                None,
                Some("fresh or recovered agent session".into()),
            ),
            AgentActivityStatus::Stale => vector.raise(
                EntropyKind::AgentHealth,
                60,
                75,
                format!("{} is stale", session.agent),
                None,
                Some("recent agent event".into()),
            ),
            AgentActivityStatus::Active => {}
        }
    }

    if !verification.recommended_commands.is_empty() {
        let verification_score = vector.score_mut(EntropyKind::Verification);
        for command in &verification.recommended_commands {
            if !verification_score
                .recommended_observations
                .contains(command)
            {
                verification_score
                    .recommended_observations
                    .push(command.clone());
            }
        }
    }

    if !verification.uncovered_acceptance_criteria.is_empty() {
        let acceptance_evidence_id =
            acceptance_criteria_evidence_id(intent_events, &verification.acceptance_criteria);
        vector.raise(
            EntropyKind::Verification,
            82,
            80,
            "acceptance criteria have no mapped verifier",
            acceptance_evidence_id,
            Some("mapped verifier for acceptance criterion".into()),
        );
    }

    let covered_acceptance_exists = !verification.acceptance_criteria.is_empty()
        && verification.uncovered_acceptance_criteria.len()
            < verification.acceptance_criteria.len()
        && !verification.recommended_commands.is_empty();
    if covered_acceptance_exists
        && verification
            .latest_passing_command
            .as_ref()
            .is_none_or(|command| {
                !verification
                    .recommended_commands
                    .iter()
                    .any(|recommended| recommended == command)
            })
    {
        let acceptance_evidence_id =
            acceptance_criteria_evidence_id(intent_events, &verification.acceptance_criteria);
        vector.raise(
            EntropyKind::Verification,
            78,
            80,
            "acceptance criteria verifier has not passed",
            acceptance_evidence_id,
            Some("passing verifier for acceptance criterion".into()),
        );
    }

    if let Some(repo_audit) = repo_audit {
        let first_untraced = repo_audit
            .changes
            .iter()
            .find(|change| change.trace_status == RepoTraceStatus::Untraced);
        if let Some(change) = first_untraced {
            vector.raise(
                EntropyKind::RepoBlame,
                88,
                90,
                "dirty git hunks lack trace evidence",
                Some(format!("repo-audit-{}", safe_slug(&change.path))),
                Some("trace rationale for every dirty hunk".into()),
            );
        } else if let Some(change) = repo_audit
            .changes
            .iter()
            .find(|change| change.trace_status == RepoTraceStatus::MissingRationale)
        {
            vector.raise(
                EntropyKind::RepoBlame,
                78,
                88,
                "dirty git hunks have trace evidence without rationale",
                Some(format!("repo-audit-{}", safe_slug(&change.path))),
                Some("rationale for every traced dirty hunk".into()),
            );
        }
    }

    apply_dev_history_entropy_priors(&mut vector, snapshot, repo_audit);

    vector
}

pub(crate) struct DevHistoryEntropyPrior {
    kind: EntropyKind,
    score: u8,
    confidence: u8,
    cause: &'static str,
    missing_evidence: &'static str,
    recommended_observation: &'static str,
}

pub(crate) fn apply_dev_history_entropy_priors(
    vector: &mut EntropyVector,
    snapshot: &DashboardSnapshot,
    repo_audit: Option<&RepoAuditReport>,
) {
    for report in &snapshot.recent_dev_history {
        for (finding_index, finding) in report.findings.iter().enumerate() {
            let Some(prior) = dev_history_entropy_prior(finding.kind.as_str()) else {
                continue;
            };
            if prior.kind == EntropyKind::RepoBlame
                && !dev_history_blame_hotspot_overlaps_current_change(finding, snapshot, repo_audit)
            {
                continue;
            }
            let evidence_id = dev_history_finding_evidence_id(report, finding_index, finding);
            vector.raise(
                prior.kind,
                prior.score,
                prior.confidence,
                prior.cause,
                Some(evidence_id),
                Some(prior.missing_evidence.into()),
            );
            let score = vector.score_mut(prior.kind);
            if !score
                .recommended_observations
                .iter()
                .any(|observation| observation == prior.recommended_observation)
            {
                score
                    .recommended_observations
                    .push(prior.recommended_observation.into());
            }
        }
    }
}

pub(crate) fn dev_history_blame_hotspot_overlaps_current_change(
    finding: &DevHistoryFinding,
    snapshot: &DashboardSnapshot,
    repo_audit: Option<&RepoAuditReport>,
) -> bool {
    let mut current_paths = snapshot
        .recent_events
        .iter()
        .filter(|event| event_is_change_like(event))
        .filter_map(|event| event.file.as_deref())
        .map(normalize_path_for_match)
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();

    if let Some(repo_audit) = repo_audit {
        current_paths.extend(
            repo_audit
                .changes
                .iter()
                .map(|change| normalize_path_for_match(&change.path))
                .filter(|path| !path.is_empty()),
        );
    }

    current_paths.iter().any(|path| {
        finding
            .evidence
            .iter()
            .any(|evidence| dev_history_hotspot_matches_path(evidence, path))
    })
}

pub(crate) fn dev_history_hotspot_matches_path(evidence: &str, current_path: &str) -> bool {
    let hotspot = evidence
        .split_once(" (")
        .map_or(evidence, |(path, _)| path)
        .trim();
    let hotspot = normalize_path_for_match(hotspot);
    if hotspot.is_empty() || current_path.is_empty() {
        return false;
    }
    hotspot == current_path
        || current_path
            .strip_suffix(&hotspot)
            .is_some_and(|prefix| prefix.ends_with('/'))
        || hotspot
            .strip_suffix(current_path)
            .is_some_and(|prefix| prefix.ends_with('/'))
}

pub(crate) fn dev_history_entropy_prior(kind: &str) -> Option<DevHistoryEntropyPrior> {
    match kind {
        "verification_entropy" => Some(DevHistoryEntropyPrior {
            kind: EntropyKind::Verification,
            score: 58,
            confidence: 60,
            cause: "local dev-history shows recurring verification uncertainty",
            missing_evidence: "fresh verifier evidence for the current run",
            recommended_observation: "check verifier freshness against the latest current-run write",
        }),
        "agent_health_entropy" => Some(DevHistoryEntropyPrior {
            kind: EntropyKind::AgentHealth,
            score: 55,
            confidence: 55,
            cause: "local dev-history shows recurring agent-health instability",
            missing_evidence: "current-run loop, provider, or tool-failure evidence",
            recommended_observation: "watch for repeated failures before retry or handoff",
        }),
        "blame_hotspots" => Some(DevHistoryEntropyPrior {
            kind: EntropyKind::RepoBlame,
            score: 55,
            confidence: 55,
            cause: "local dev-history shows recurring repo/blame hotspots",
            missing_evidence: "current-run hunk rationale for touched hotspot files",
            recommended_observation: "compare current dirty hunks with trace and rationale records",
        }),
        "subagent_lifecycle_entropy" => Some(DevHistoryEntropyPrior {
            kind: EntropyKind::Plan,
            score: 55,
            confidence: 55,
            cause: "local dev-history shows recurring subagent lifecycle fragmentation",
            missing_evidence: "current-run terminal worker outcomes and integration summaries",
            recommended_observation: "require joined_with_summary, cancelled_with_reason, timed_out, superseded, or failed outcomes before more fan-out",
        }),
        _ => None,
    }
}

pub(crate) fn acceptance_criteria_evidence_id(
    intent_events: &[Event],
    acceptance_criteria: &[String],
) -> Option<String> {
    if acceptance_criteria.is_empty() {
        return None;
    }
    intent_events
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, event)| event.kind == EventKind::UserInstruction)
        .find_map(|(index, event)| {
            let content = event.content.as_deref()?;
            let extracted = extract_acceptance_criteria(content);
            let introduced = extracted
                .iter()
                .any(|criterion| acceptance_criteria.iter().any(|target| target == criterion));
            if introduced {
                Some(event_evidence_id(event, index))
            } else {
                None
            }
        })
}

pub(crate) fn verification_summary(
    snapshot: &DashboardSnapshot,
    intent_events: &[Event],
    verifiers: &[VerifierConfig],
    policy: &PolicyConfig,
    repo_audit: Option<&RepoAuditReport>,
) -> VerificationSummary {
    let acceptance_criteria = acceptance_criteria_from_events(intent_events);
    let uncovered_acceptance_criteria = acceptance_criteria
        .iter()
        .filter(|criterion| {
            !verifiers
                .iter()
                .any(|verifier| verifier_matches_acceptance(verifier, criterion))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut changed_source_files = Vec::new();
    for event in &snapshot.recent_events {
        if event_is_change_like(event)
            && event
                .file
                .as_deref()
                .is_some_and(|file| is_verification_relevant_file(file, policy))
            && let Some(file) = &event.file
        {
            push_changed_source_file(&mut changed_source_files, file);
        }
    }
    if let Some(repo_audit) = repo_audit {
        for change in &repo_audit.changes {
            if is_verification_relevant_file(&change.path, policy) {
                push_changed_source_file(&mut changed_source_files, &change.path);
            }
        }
    }

    let mut latest_passing: Option<((i64, usize), String)> = None;
    let mut latest_failing: Option<((i64, usize), String, Option<VerificationFailureClass>)> = None;
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        if !event_is_verification_result(event) {
            continue;
        }
        let Some(time) = event.time.as_deref().and_then(parse_utc_seconds) else {
            continue;
        };
        let order = (time, index);
        let command = event.command.clone().unwrap_or_default();
        match event.exit_code {
            Some(0) => {
                if latest_passing
                    .as_ref()
                    .is_none_or(|(current, _)| order > *current)
                {
                    latest_passing = Some((order, command));
                }
            }
            Some(_)
                if latest_failing
                    .as_ref()
                    .is_none_or(|(current, _, _)| order > *current) =>
            {
                latest_failing = Some((order, command, None));
            }
            Some(_) => {}
            None => {}
        }
    }
    let verifier_sequence_base = snapshot.recent_events.len();
    for (index, run) in snapshot.recent_verifier_runs.iter().enumerate() {
        let Some(time) = verifier_run_time(run) else {
            continue;
        };
        let order = (time, verifier_sequence_base + index);
        match run.status {
            VerificationRunStatus::Passed => {
                if latest_passing
                    .as_ref()
                    .is_none_or(|(current, _)| order > *current)
                {
                    latest_passing = Some((order, run.command.clone()));
                }
            }
            VerificationRunStatus::Failed | VerificationRunStatus::TimedOut
                if latest_failing
                    .as_ref()
                    .is_none_or(|(current, _, _)| order > *current) =>
            {
                latest_failing = Some((order, run.command.clone(), run.failure_class));
            }
            VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => {}
        }
    }
    let status = latest_verification_status(snapshot, verifiers, policy, repo_audit);
    let acceptance_coverage =
        acceptance_coverage_for_criteria(&acceptance_criteria, verifiers, snapshot, status);

    let mut recommended_commands = Vec::new();
    for verifier in verifiers {
        let matches_changed_path = verifier.paths.is_empty()
            || changed_source_files
                .iter()
                .any(|file| verifier_matches_path(verifier, file));
        let matches_acceptance = acceptance_criteria
            .iter()
            .any(|criterion| verifier_matches_acceptance(verifier, criterion));
        if (matches_changed_path || matches_acceptance)
            && !recommended_commands.contains(&verifier.command)
        {
            recommended_commands.push(verifier.command.clone());
        }
    }

    VerificationSummary {
        status,
        recommended_commands,
        changed_source_files,
        acceptance_criteria,
        uncovered_acceptance_criteria,
        acceptance_coverage,
        latest_passing_command: latest_passing.map(|(_, command)| command),
        latest_failing_command: latest_failing
            .as_ref()
            .map(|(_, command, _)| command.clone()),
        latest_failure_class: latest_failing.and_then(|(_, _, failure_class)| failure_class),
    }
}

pub(crate) fn acceptance_coverage_for_criteria(
    acceptance_criteria: &[String],
    verifiers: &[VerifierConfig],
    snapshot: &DashboardSnapshot,
    verification_status: VerificationStatus,
) -> Vec<AcceptanceCoverage> {
    let latest_status_by_key = latest_verification_status_by_key(snapshot);
    acceptance_criteria
        .iter()
        .map(|criterion| {
            let mapped = verifiers
                .iter()
                .filter(|verifier| verifier_matches_acceptance(verifier, criterion))
                .collect::<Vec<_>>();
            let latest_status = latest_status_for_verifiers(&mapped, &latest_status_by_key);
            AcceptanceCoverage {
                criterion: criterion.clone(),
                status: acceptance_coverage_status(
                    !mapped.is_empty(),
                    latest_status.map(|(_, status)| status),
                    verification_status,
                ),
                verifier_ids: mapped.iter().map(|verifier| verifier.id.clone()).collect(),
                verifier_commands: mapped
                    .iter()
                    .map(|verifier| verifier.command.clone())
                    .collect(),
                latest_status: latest_status.map(|(_, status)| status),
            }
        })
        .collect()
}

pub(crate) fn latest_verification_status_by_key(
    snapshot: &DashboardSnapshot,
) -> HashMap<String, ((i64, usize), VerificationStatus)> {
    let mut latest = HashMap::<String, ((i64, usize), VerificationStatus)>::new();
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        if !event_is_verification_result(event) {
            continue;
        }
        let Some(command) = event.command.as_deref() else {
            continue;
        };
        let Some(time) = event.time.as_deref().and_then(parse_utc_seconds) else {
            continue;
        };
        let status = match event.exit_code {
            Some(0) => VerificationStatus::Passed,
            Some(_) => VerificationStatus::Failed,
            None => continue,
        };
        update_latest_verification_status(
            &mut latest,
            normalize_command_signature(command),
            (time, index),
            status,
        );
    }

    let verifier_sequence_base = snapshot.recent_events.len();
    for (index, run) in snapshot.recent_verifier_runs.iter().enumerate() {
        let Some(time) = verifier_run_time(run) else {
            continue;
        };
        let order = (time, verifier_sequence_base + index);
        let status = verifier_run_verification_status(run);
        update_latest_verification_status(
            &mut latest,
            normalize_command_signature(&run.command),
            order,
            status,
        );
        if let Some(verifier_id) = run.verifier_id.as_deref() {
            update_latest_verification_status(&mut latest, verifier_id.to_string(), order, status);
        }
    }
    latest
}

pub(crate) fn update_latest_verification_status(
    latest: &mut HashMap<String, ((i64, usize), VerificationStatus)>,
    key: String,
    order: (i64, usize),
    status: VerificationStatus,
) {
    if key.trim().is_empty() {
        return;
    }
    if latest
        .get(&key)
        .is_none_or(|(current_order, _)| order > *current_order)
    {
        latest.insert(key, (order, status));
    }
}

pub(crate) fn latest_status_for_verifiers(
    verifiers: &[&VerifierConfig],
    latest_status_by_key: &HashMap<String, ((i64, usize), VerificationStatus)>,
) -> Option<((i64, usize), VerificationStatus)> {
    verifiers
        .iter()
        .filter_map(|verifier| {
            let command_key = normalize_command_signature(&verifier.command);
            latest_status_by_key
                .get(&command_key)
                .or_else(|| latest_status_by_key.get(&verifier.id))
                .copied()
        })
        .max_by_key(|(order, _)| *order)
}

pub(crate) fn acceptance_coverage_status(
    has_mapping: bool,
    latest_status: Option<VerificationStatus>,
    verification_status: VerificationStatus,
) -> AcceptanceCoverageStatus {
    if !has_mapping {
        return AcceptanceCoverageStatus::Unmapped;
    }
    match latest_status {
        Some(VerificationStatus::Passed) if verification_status == VerificationStatus::Stale => {
            AcceptanceCoverageStatus::Stale
        }
        Some(VerificationStatus::Passed) => AcceptanceCoverageStatus::Covered,
        Some(VerificationStatus::Failed) => AcceptanceCoverageStatus::Failed,
        Some(VerificationStatus::Stale) => AcceptanceCoverageStatus::Stale,
        Some(VerificationStatus::NotRun) | None => AcceptanceCoverageStatus::Unverified,
    }
}

pub(crate) fn push_changed_source_file(files: &mut Vec<String>, file: &str) {
    if !files.iter().any(|existing| existing == file) {
        files.push(file.to_string());
    }
}

pub(crate) fn acceptance_criteria_from_events(events: &[Event]) -> Vec<String> {
    let mut criteria = Vec::new();
    for event in events {
        if event.kind != EventKind::UserInstruction {
            continue;
        }
        let Some(content) = event.content.as_deref() else {
            continue;
        };
        for criterion in extract_acceptance_criteria(content) {
            if !criteria.iter().any(|existing| existing == &criterion) {
                criteria.push(criterion);
            }
        }
    }
    criteria
}

pub(crate) fn extract_acceptance_criteria(content: &str) -> Vec<String> {
    let mut criteria = Vec::new();
    let mut in_acceptance_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            in_acceptance_block = false;
            continue;
        }

        if let Some(rest) = acceptance_prefix_rest(trimmed) {
            if rest.trim().is_empty() {
                in_acceptance_block = true;
            } else {
                append_inline_acceptance_criteria(&mut criteria, rest);
                in_acceptance_block = false;
            }
            continue;
        }

        if in_acceptance_block {
            let Some(item) = acceptance_block_item(trimmed) else {
                in_acceptance_block = false;
                continue;
            };
            let item = clean_acceptance_criterion(item);
            if !item.is_empty() {
                criteria.push(item);
            }
        }
    }
    criteria
}

pub(crate) fn append_inline_acceptance_criteria(criteria: &mut Vec<String>, value: &str) {
    for item in inline_acceptance_items(value) {
        let item = clean_acceptance_criterion(item);
        if !item.is_empty() {
            criteria.push(item);
        }
    }
}

pub(crate) fn inline_acceptance_items(value: &str) -> Vec<&str> {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return rest.split(" - ").collect();
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return rest.split(" * ").collect();
    }
    vec![trimmed]
}

pub(crate) fn acceptance_prefix_rest(line: &str) -> Option<&str> {
    let lower = line.to_lowercase();
    for prefix in [
        "acceptance:",
        "acceptance criterion:",
        "acceptance criteria:",
        "acceptance criteria -",
        "acceptance criteria - ",
    ] {
        if lower.starts_with(prefix) {
            return Some(line[prefix.len()..].trim());
        }
    }
    None
}

pub(crate) fn acceptance_block_item(line: &str) -> Option<&str> {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix("- ") {
        return Some(rest);
    }
    if let Some(rest) = line.strip_prefix("* ") {
        return Some(rest);
    }
    if let Some(rest) = line.strip_prefix("[ ] ") {
        return Some(rest);
    }
    if let Some(rest) = line.strip_prefix("[x] ") {
        return Some(rest);
    }
    if let Some((number, rest)) = line.split_once(". ")
        && number.chars().all(|ch| ch.is_ascii_digit())
    {
        return Some(rest);
    }
    None
}

pub(crate) fn clean_acceptance_criterion(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim()
        .to_string()
}
