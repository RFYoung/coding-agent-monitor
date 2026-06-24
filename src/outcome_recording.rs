use crate::*;
use std::collections::{BTreeSet, HashMap, HashSet};

pub(crate) fn record_verifier_outcome_for_latest_advice(
    store: &mut ProjectStore,
    run: &VerifierRun,
) -> Result<(), StoreError> {
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    let Some(advice) = advice_records.into_iter().next_back() else {
        return Ok(());
    };
    if !latest_dispatch_matches_advice_packet(store, &advice)? {
        return Ok(());
    }
    let force_verification_match = advice.final_action.kind()
        == ControlActionKind::ForceVerification
        && force_verification_advice_matches_verifier(&advice, run);
    if advice.final_action.kind() == ControlActionKind::ForceVerification
        && !force_verification_match
    {
        return Ok(());
    }
    let derived_requirement_ids = verifier_requirement_ids_for_advice_case(store, &advice, run)?;
    if !force_verification_match && derived_requirement_ids.is_empty() {
        return Ok(());
    }

    let existing_outcomes = read_all_jsonl::<ActionOutcome>(&store.root.join("outcomes.jsonl"))?;
    if existing_outcomes.iter().any(|outcome| {
        outcome.advice_id == advice.advice_id
            && outcome
                .evidence_ids
                .iter()
                .any(|id| id == &run.verifier_run_id)
    }) {
        return Ok(());
    }

    let status = match run.status {
        VerificationRunStatus::Passed => OutcomeStatus::Succeeded,
        VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => OutcomeStatus::Failed,
    };
    let action = if force_verification_match {
        ControlActionKind::ForceVerification
    } else {
        advice.final_action.kind()
    };
    let default_delta = if force_verification_match { -55 } else { -20 };
    let expected_entropy_delta =
        expected_entropy_delta_for_outcome(&advice, EntropyKind::Verification, default_delta);
    let note = format!(
        "Verifier {} finished with {:?} after {:?} advice {}.",
        run.verifier_id.as_deref().unwrap_or("<unknown>"),
        run.status,
        action,
        advice.advice_id
    );
    let observed_entropy_delta = observed_entropy_deltas_for_outcome(
        store,
        &advice,
        &expected_entropy_delta,
        EntropyKind::Verification,
    );
    let evidence_ids = vec![run.verifier_run_id.clone()];
    let cause_evidence_ids = outcome_cause_evidence_ids(&advice);
    let mut requirement_ids = advice.control_rationale.requirement_ids.clone();
    for requirement_id in derived_requirement_ids {
        push_unique_requirement_id(&mut requirement_ids, &requirement_id);
    }
    store.append_action_outcome(&ActionOutcome {
        outcome_id: format!("outcome-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        action,
        status,
        observed_entropy_delta_evidence: entropy_delta_evidence_refs(
            &observed_entropy_delta,
            &cause_evidence_ids,
            &evidence_ids,
        ),
        observed_entropy_delta,
        expected_entropy_delta,
        evidence_ids,
        requirement_ids,
        note: Some(note),
    })
}

fn verifier_requirement_ids_for_advice_case(
    store: &ProjectStore,
    advice: &AdviceRun,
    run: &VerifierRun,
) -> Result<Vec<String>, StoreError> {
    let case_files = read_all_jsonl::<ControlCaseFile>(&store.root.join("case-files.jsonl"))?;
    let Some(case_file) = case_files
        .into_iter()
        .find(|case_file| case_file.case_file_id == advice.case_file_id)
    else {
        return Ok(Vec::new());
    };
    let verifier_id = run.verifier_id.as_deref();
    let command = run.command.trim();
    let mut requirement_ids = Vec::new();
    for requirement in case_file.requirements {
        let id_matches = verifier_id.is_some_and(|id| {
            requirement
                .verifier_ids
                .iter()
                .any(|verifier_id| verifier_id == id)
        });
        let command_matches = requirement
            .verifier_commands
            .iter()
            .any(|verifier_command| verifier_command.trim() == command);
        if id_matches || command_matches {
            push_unique_requirement_id(&mut requirement_ids, &requirement.requirement_id);
        }
    }
    Ok(requirement_ids)
}

fn push_unique_requirement_id(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

pub(super) fn record_immediate_outcome_for_advice(
    store: &mut ProjectStore,
    advice: &AdviceRun,
) -> Result<(), StoreError> {
    let Some(outcome) = immediate_outcome_for_advice(advice) else {
        return Ok(());
    };
    store.append_action_outcome(&outcome)
}

fn immediate_outcome_for_advice(advice: &AdviceRun) -> Option<ActionOutcome> {
    let action = if advice.dispatch_result.status == DispatchStatus::Failed {
        advice.final_action.kind()
    } else {
        match advice.final_action {
            ControlAction::Pause { .. } => ControlActionKind::Pause,
            ControlAction::BlockProgressUntilTraceAndVerification { .. } => {
                ControlActionKind::BlockProgressUntilTraceAndVerification
            }
            _ => return None,
        }
    };
    let status = match advice.dispatch_result.status {
        DispatchStatus::OutboxWritten => OutcomeStatus::Succeeded,
        DispatchStatus::SuppressedDuplicate => OutcomeStatus::Succeeded,
        DispatchStatus::Failed => OutcomeStatus::Failed,
    };
    Some(ActionOutcome {
        outcome_id: format!("outcome-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        action,
        status,
        expected_entropy_delta: Vec::new(),
        observed_entropy_delta: Vec::new(),
        observed_entropy_delta_evidence: Vec::new(),
        evidence_ids: vec![advice.dispatch_result.dispatch_id.clone()],
        requirement_ids: advice.control_rationale.requirement_ids.clone(),
        note: Some(format!(
            "{} packet dispatch {} finished with {:?} for advice {}.",
            control_action_kind_label(action),
            advice.dispatch_result.dispatch_id,
            advice.dispatch_result.status,
            advice.advice_id
        )),
    })
}

pub(super) fn record_timed_out_handoff_outcomes(
    store: &mut ProjectStore,
    current_case_file: &ControlCaseFile,
    policy: &PolicyConfig,
) -> Result<(), StoreError> {
    let Some(timeout_secs) = policy
        .handoff_outcome_timeout_secs
        .filter(|timeout_secs| *timeout_secs >= 0)
    else {
        return Ok(());
    };
    let now = case_file_policy_time(current_case_file);
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    let case_files = read_all_jsonl::<ControlCaseFile>(&store.root.join("case-files.jsonl"))?;
    let case_files_by_id = case_files
        .into_iter()
        .map(|case_file| (case_file.case_file_id.clone(), case_file))
        .collect::<HashMap<_, _>>();
    let events = read_all_jsonl::<Event>(&store.root.join("events.jsonl"))?;
    let mut existing_outcomes =
        read_all_jsonl::<ActionOutcome>(&store.root.join("outcomes.jsonl"))?
            .into_iter()
            .map(|outcome| (outcome.advice_id, outcome.action))
            .collect::<HashSet<_>>();

    for advice in advice_records {
        let Some(action) = handoff_outcome_action_kind(&advice.final_action) else {
            continue;
        };
        let outcome_key = (advice.advice_id.clone(), action);
        if existing_outcomes.contains(&outcome_key) {
            continue;
        }
        let Some(case_file) = case_files_by_id.get(&advice.case_file_id) else {
            continue;
        };
        let Some(handoff_started_at) = parse_utc_seconds(&case_file.built_at) else {
            continue;
        };
        if now - handoff_started_at < timeout_secs {
            continue;
        }
        if handoff_target_event_after_case_file(&events, &advice.packet.target_agent, case_file) {
            continue;
        }

        let outcome = timed_out_handoff_outcome_for_advice(&advice, store, action, timeout_secs);
        store.append_action_outcome(&outcome)?;
        release_failed_handoff_lock_for_advice(store, &advice, &outcome)?;
        existing_outcomes.insert(outcome_key);
    }

    Ok(())
}

fn handoff_target_event_after_case_file(
    events: &[Event],
    target_agent: &str,
    case_file: &ControlCaseFile,
) -> bool {
    let case_file_built_at = parse_utc_seconds(&case_file.built_at);
    events.iter().enumerate().any(|(index, event)| {
        if event.agent != target_agent || !agent_progress_outcome_event_is_signal(event) {
            return false;
        }
        if index + 1 > case_file.event_count {
            return true;
        }
        event
            .time
            .as_deref()
            .and_then(parse_utc_seconds)
            .zip(case_file_built_at)
            .is_some_and(|(event_at, built_at)| event_at > built_at)
    })
}

fn timed_out_handoff_outcome_for_advice(
    advice: &AdviceRun,
    store: &ProjectStore,
    action: ControlActionKind,
    timeout_secs: i64,
) -> ActionOutcome {
    let (default_kind, default_delta) = handoff_outcome_default_delta(advice);
    let expected_entropy_delta =
        expected_entropy_delta_for_outcome(advice, default_kind, default_delta);
    let observed_entropy_delta =
        observed_entropy_deltas_for_outcome(store, advice, &expected_entropy_delta, default_kind);
    let evidence_ids = vec![advice.dispatch_result.dispatch_id.clone()];
    let cause_evidence_ids = outcome_cause_evidence_ids(advice);
    ActionOutcome {
        outcome_id: format!("outcome-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        action,
        status: OutcomeStatus::Failed,
        observed_entropy_delta_evidence: entropy_delta_evidence_refs(
            &observed_entropy_delta,
            &cause_evidence_ids,
            &evidence_ids,
        ),
        observed_entropy_delta,
        expected_entropy_delta,
        evidence_ids,
        requirement_ids: advice.control_rationale.requirement_ids.clone(),
        note: Some(format!(
            "{} outcome for advice {} timed out after {timeout_secs} second(s) without target-agent activity.",
            control_action_kind_label(action),
            advice.advice_id
        )),
    }
}

pub fn record_event_outcome_for_latest_advice(
    store: &mut ProjectStore,
    event: &Event,
) -> Result<(), StoreError> {
    if let Some(outcome) = retry_agent_outcome_for_event(store, event)? {
        return store.append_action_outcome(&outcome);
    }
    if let Some(outcome) = ask_user_outcome_for_event(store, event)? {
        return store.append_action_outcome(&outcome);
    }
    if let Some(outcome) = send_follow_up_outcome_for_event(store, event)? {
        return store.append_action_outcome(&outcome);
    }
    if let Some(outcome) = spawn_judge_outcome_for_event(store, event)? {
        return store.append_action_outcome(&outcome);
    }
    if let Some(outcome) = handoff_outcome_for_event(store, event)? {
        store.append_action_outcome(&outcome)?;
        release_failed_handoff_lock_for_outcome(store, &outcome)?;
        return Ok(());
    }
    Ok(())
}

fn send_follow_up_outcome_for_event(
    store: &ProjectStore,
    event: &Event,
) -> Result<Option<ActionOutcome>, StoreError> {
    if !agent_progress_outcome_event_is_signal(event) {
        return Ok(None);
    }
    let Some(event_id) = event.event_id.clone() else {
        return Ok(None);
    };
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    let Some(advice) = advice_records.into_iter().next_back() else {
        return Ok(None);
    };
    if !matches!(advice.final_action, ControlAction::SendFollowUp { .. }) {
        return Ok(None);
    }
    if !latest_dispatch_matches_advice_packet(store, &advice)? {
        return Ok(None);
    }
    if event.agent != advice.packet.target_agent {
        return Ok(None);
    }
    let existing_outcomes = read_all_jsonl::<ActionOutcome>(&store.root.join("outcomes.jsonl"))?;
    if existing_outcomes.iter().any(|outcome| {
        outcome.advice_id == advice.advice_id && outcome.action == ControlActionKind::SendFollowUp
    }) {
        return Ok(None);
    }

    let (default_kind, default_delta) = send_follow_up_outcome_default_delta(&advice);
    let expected_entropy_delta =
        expected_entropy_delta_for_outcome(&advice, default_kind, default_delta);
    let note = format!(
        "Event {event_id} from {} produced succeeded send_follow_up outcome for advice {}.",
        event.agent, advice.advice_id
    );
    let observed_entropy_delta =
        observed_entropy_deltas_for_outcome(store, &advice, &expected_entropy_delta, default_kind);
    let evidence_ids = vec![event_id];
    let cause_evidence_ids = outcome_cause_evidence_ids(&advice);

    Ok(Some(ActionOutcome {
        outcome_id: format!("outcome-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        action: ControlActionKind::SendFollowUp,
        status: OutcomeStatus::Succeeded,
        observed_entropy_delta_evidence: entropy_delta_evidence_refs(
            &observed_entropy_delta,
            &cause_evidence_ids,
            &evidence_ids,
        ),
        observed_entropy_delta,
        expected_entropy_delta,
        evidence_ids,
        requirement_ids: advice.control_rationale.requirement_ids.clone(),
        note: Some(note),
    }))
}

fn send_follow_up_outcome_default_delta(advice: &AdviceRun) -> (EntropyKind, i16) {
    if let Some(delta) = advice.control_rationale.expected_entropy_delta.first() {
        return (delta.kind, delta.delta);
    }
    (EntropyKind::Plan, -25)
}

fn spawn_judge_outcome_for_event(
    store: &ProjectStore,
    event: &Event,
) -> Result<Option<ActionOutcome>, StoreError> {
    if !spawn_judge_outcome_event_is_signal(event) {
        return Ok(None);
    }
    let Some(event_id) = event.event_id.clone() else {
        return Ok(None);
    };
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    let Some(advice) = advice_records.into_iter().next_back() else {
        return Ok(None);
    };
    if !matches!(advice.final_action, ControlAction::SpawnJudgeAgent { .. }) {
        return Ok(None);
    }
    if !latest_dispatch_matches_advice_packet(store, &advice)? {
        return Ok(None);
    }
    if event.agent != advice.packet.target_agent {
        return Ok(None);
    }
    let existing_outcomes = read_all_jsonl::<ActionOutcome>(&store.root.join("outcomes.jsonl"))?;
    if existing_outcomes.iter().any(|outcome| {
        outcome.advice_id == advice.advice_id
            && outcome.action == ControlActionKind::SpawnJudgeAgent
    }) {
        return Ok(None);
    }

    let (default_kind, default_delta) = spawn_judge_outcome_default_delta(&advice);
    let expected_entropy_delta =
        expected_entropy_delta_for_outcome(&advice, default_kind, default_delta);
    let status = spawn_judge_outcome_status_for_event(event);
    let note = format!(
        "Event {event_id} from {} produced {:?} spawn_judge_agent outcome for advice {}.",
        event.agent, status, advice.advice_id
    );
    let observed_entropy_delta =
        observed_entropy_deltas_for_outcome(store, &advice, &expected_entropy_delta, default_kind);
    let evidence_ids = vec![event_id];
    let cause_evidence_ids = outcome_cause_evidence_ids(&advice);

    Ok(Some(ActionOutcome {
        outcome_id: format!("outcome-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        action: ControlActionKind::SpawnJudgeAgent,
        status,
        observed_entropy_delta_evidence: entropy_delta_evidence_refs(
            &observed_entropy_delta,
            &cause_evidence_ids,
            &evidence_ids,
        ),
        observed_entropy_delta,
        expected_entropy_delta,
        evidence_ids,
        requirement_ids: advice.control_rationale.requirement_ids.clone(),
        note: Some(note),
    }))
}

fn spawn_judge_outcome_event_is_signal(event: &Event) -> bool {
    if spawn_judge_outcome_status_for_event(event) == OutcomeStatus::Failed {
        return true;
    }
    if !matches!(
        event.kind,
        EventKind::ModelMessage
            | EventKind::ToolResult
            | EventKind::CommandResult
            | EventKind::HandoffSummary
            | EventKind::VerificationClaim
    ) {
        return false;
    }
    let text = event_outcome_text(event);
    let has_review_context = [
        "read-only judge",
        "judge review",
        "review:",
        "review result",
        "audit",
        "assessment",
    ]
    .iter()
    .any(|signal| text.contains(signal));
    let has_disposition = [
        "keep",
        "revert",
        "should stay",
        "should be removed",
        "needs trace",
        "trace rationale",
        "send back",
        "repair",
        "unjustified",
        "suspicious",
        "approve",
        "reject",
    ]
    .iter()
    .any(|signal| text.contains(signal));
    has_review_context && has_disposition
}

fn spawn_judge_outcome_status_for_event(event: &Event) -> OutcomeStatus {
    if matches!(event.kind, EventKind::FileChange | EventKind::RepoDiff) {
        return OutcomeStatus::Failed;
    }
    handoff_outcome_status_for_event(event)
}

fn event_outcome_text(event: &Event) -> String {
    [
        event.content.as_deref(),
        event.rationale.as_deref(),
        event.command.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
    .to_ascii_lowercase()
}

fn spawn_judge_outcome_default_delta(advice: &AdviceRun) -> (EntropyKind, i16) {
    if let Some(delta) = advice.control_rationale.expected_entropy_delta.first() {
        return (delta.kind, delta.delta);
    }
    (EntropyKind::RepoBlame, -35)
}

fn handoff_outcome_for_event(
    store: &ProjectStore,
    event: &Event,
) -> Result<Option<ActionOutcome>, StoreError> {
    if !agent_progress_outcome_event_is_signal(event) {
        return Ok(None);
    }
    let Some(event_id) = event.event_id.clone() else {
        return Ok(None);
    };
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    let Some(advice) = advice_records.into_iter().next_back() else {
        return Ok(None);
    };
    let Some(action) = handoff_outcome_action_kind(&advice.final_action) else {
        return Ok(None);
    };
    if !latest_dispatch_matches_advice_packet(store, &advice)? {
        return Ok(None);
    }
    if event.agent != advice.packet.target_agent {
        return Ok(None);
    }
    let existing_outcomes = read_all_jsonl::<ActionOutcome>(&store.root.join("outcomes.jsonl"))?;
    if existing_outcomes
        .iter()
        .any(|outcome| outcome.advice_id == advice.advice_id && outcome.action == action)
    {
        return Ok(None);
    }

    let (default_kind, default_delta) = handoff_outcome_default_delta(&advice);
    let expected_entropy_delta =
        expected_entropy_delta_for_outcome(&advice, default_kind, default_delta);
    let status = handoff_outcome_status_for_event(event);
    let note = format!(
        "Event {event_id} from {} produced {:?} {} outcome for advice {}.",
        event.agent,
        status,
        control_action_kind_label(action),
        advice.advice_id
    );
    let observed_entropy_delta =
        observed_entropy_deltas_for_outcome(store, &advice, &expected_entropy_delta, default_kind);
    let evidence_ids = vec![event_id];
    let cause_evidence_ids = outcome_cause_evidence_ids(&advice);

    Ok(Some(ActionOutcome {
        outcome_id: format!("outcome-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        action,
        status,
        observed_entropy_delta_evidence: entropy_delta_evidence_refs(
            &observed_entropy_delta,
            &cause_evidence_ids,
            &evidence_ids,
        ),
        observed_entropy_delta,
        expected_entropy_delta,
        evidence_ids,
        requirement_ids: advice.control_rationale.requirement_ids.clone(),
        note: Some(note),
    }))
}

fn release_failed_handoff_lock_for_outcome(
    store: &mut ProjectStore,
    outcome: &ActionOutcome,
) -> Result<(), StoreError> {
    if outcome.status != OutcomeStatus::Failed || !is_handoff_action_kind(outcome.action) {
        return Ok(());
    }
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    let Some(advice) = advice_records
        .iter()
        .find(|advice| advice.advice_id == outcome.advice_id)
    else {
        return Ok(());
    };
    release_failed_handoff_lock_for_advice(store, advice, outcome)
}

fn release_failed_handoff_lock_for_advice(
    store: &mut ProjectStore,
    advice: &AdviceRun,
    outcome: &ActionOutcome,
) -> Result<(), StoreError> {
    if outcome.status != OutcomeStatus::Failed
        || handoff_outcome_action_kind(&advice.final_action) != Some(outcome.action)
    {
        return Ok(());
    }
    let target_agent = advice.packet.target_agent.trim();
    if target_agent.is_empty() {
        return Ok(());
    }
    let case_files = read_all_jsonl::<ControlCaseFile>(&store.root.join("case-files.jsonl"))?;
    let Some(case_file) = case_files
        .iter()
        .find(|case_file| case_file.case_file_id == advice.case_file_id)
    else {
        return Ok(());
    };
    let Some(lock) = store.active_worktree_lock_for(&case_file.workspace)? else {
        return Ok(());
    };
    if lock.owner_agent != target_agent {
        return Ok(());
    }
    store.release_worktree_lock(&lock.worktree, &lock.lock_id)?;
    Ok(())
}

fn is_handoff_action_kind(action: ControlActionKind) -> bool {
    matches!(
        action,
        ControlActionKind::SpawnFreshAgent | ControlActionKind::SwitchAgent
    )
}

fn agent_progress_outcome_event_is_signal(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::ModelMessage
            | EventKind::ToolCall
            | EventKind::ToolResult
            | EventKind::CommandResult
            | EventKind::FileChange
            | EventKind::RepoDiff
            | EventKind::UserInstruction
            | EventKind::HandoffSummary
            | EventKind::AgentHealth
            | EventKind::VerificationClaim
    )
}

fn handoff_outcome_status_for_event(event: &Event) -> OutcomeStatus {
    if event
        .exit_code
        .is_some_and(|exit_code| exit_code != 0 && !is_verification_command_event(event))
    {
        return OutcomeStatus::Failed;
    }
    let failure_text = event
        .content
        .as_deref()
        .or(event.rationale.as_deref())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let explicit_failure = matches!(event.kind, EventKind::AgentHealth | EventKind::ToolResult)
        && [
            "session error",
            "session failed",
            "process_crash",
            "provider_unavailable",
            "tool_failure",
            "tool failed",
            "agent process exited",
            "crashed",
        ]
        .iter()
        .any(|signal| failure_text.contains(signal));
    if explicit_failure {
        OutcomeStatus::Failed
    } else {
        OutcomeStatus::Succeeded
    }
}

fn is_verification_command_event(event: &Event) -> bool {
    event
        .command
        .as_deref()
        .map(normalize_command_signature)
        .is_some_and(|command| is_verification_command(&command))
}

fn handoff_outcome_action_kind(action: &ControlAction) -> Option<ControlActionKind> {
    match action {
        ControlAction::SpawnFreshAgent { .. } => Some(ControlActionKind::SpawnFreshAgent),
        ControlAction::SwitchAgent { .. } => Some(ControlActionKind::SwitchAgent),
        _ => None,
    }
}

fn handoff_outcome_default_delta(advice: &AdviceRun) -> (EntropyKind, i16) {
    if let Some(delta) = advice.control_rationale.expected_entropy_delta.first() {
        return (delta.kind, delta.delta);
    }
    match advice.final_action {
        ControlAction::SwitchAgent { .. } => (EntropyKind::AgentHealth, -45),
        ControlAction::SpawnFreshAgent { .. } => (EntropyKind::Context, -50),
        _ => (EntropyKind::Context, -25),
    }
}

fn ask_user_outcome_for_event(
    store: &ProjectStore,
    event: &Event,
) -> Result<Option<ActionOutcome>, StoreError> {
    if event.kind != EventKind::UserInstruction {
        return Ok(None);
    }
    let Some(event_id) = event.event_id.clone() else {
        return Ok(None);
    };
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    let Some(advice) = advice_records.into_iter().next_back() else {
        return Ok(None);
    };
    if !matches!(advice.final_action, ControlAction::AskUser { .. }) {
        return Ok(None);
    }
    if !latest_dispatch_matches_advice_packet(store, &advice)? {
        return Ok(None);
    }
    let existing_outcomes = read_all_jsonl::<ActionOutcome>(&store.root.join("outcomes.jsonl"))?;
    if existing_outcomes.iter().any(|outcome| {
        outcome.advice_id == advice.advice_id && outcome.action == ControlActionKind::AskUser
    }) {
        return Ok(None);
    }

    let expected_entropy_delta =
        expected_entropy_delta_for_outcome(&advice, EntropyKind::UserDecision, -70);
    let note = format!(
        "Event {event_id} produced succeeded ask_user outcome for advice {}.",
        advice.advice_id
    );
    let observed_entropy_delta = observed_entropy_deltas_for_outcome(
        store,
        &advice,
        &expected_entropy_delta,
        EntropyKind::UserDecision,
    );
    let evidence_ids = vec![event_id];
    let cause_evidence_ids = outcome_cause_evidence_ids(&advice);

    Ok(Some(ActionOutcome {
        outcome_id: format!("outcome-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        action: ControlActionKind::AskUser,
        status: OutcomeStatus::Succeeded,
        observed_entropy_delta_evidence: entropy_delta_evidence_refs(
            &observed_entropy_delta,
            &cause_evidence_ids,
            &evidence_ids,
        ),
        observed_entropy_delta,
        expected_entropy_delta,
        evidence_ids,
        requirement_ids: advice.control_rationale.requirement_ids.clone(),
        note: Some(note),
    }))
}

fn retry_agent_outcome_for_event(
    store: &ProjectStore,
    event: &Event,
) -> Result<Option<ActionOutcome>, StoreError> {
    if event.kind != EventKind::CommandResult || event.exit_code.is_none() {
        return Ok(None);
    }
    let Some(event_id) = event.event_id.clone() else {
        return Ok(None);
    };
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    let Some(advice) = advice_records.into_iter().next_back() else {
        return Ok(None);
    };
    if !matches!(advice.final_action, ControlAction::RetryAgent { .. }) {
        return Ok(None);
    }
    if !latest_dispatch_matches_advice_packet(store, &advice)? {
        return Ok(None);
    }
    let target_agent = retry_outcome_target_agent(&advice);
    if event.agent != target_agent {
        return Ok(None);
    }
    let existing_outcomes = read_all_jsonl::<ActionOutcome>(&store.root.join("outcomes.jsonl"))?;
    if existing_outcomes.iter().any(|outcome| {
        outcome.advice_id == advice.advice_id && outcome.action == ControlActionKind::RetryAgent
    }) {
        return Ok(None);
    }

    let failed_signatures = retry_failure_signatures_for_advice(store, &advice, &target_agent)?;
    let Some(status) = retry_outcome_status_for_event(event, &failed_signatures) else {
        return Ok(None);
    };
    let expected_entropy_delta =
        expected_entropy_delta_for_outcome(&advice, EntropyKind::AgentHealth, -35);
    let note = format!(
        "Event {event_id} produced {:?} retry_agent outcome for advice {}.",
        status, advice.advice_id
    );
    let observed_entropy_delta = observed_entropy_deltas_for_outcome(
        store,
        &advice,
        &expected_entropy_delta,
        EntropyKind::AgentHealth,
    );
    let evidence_ids = vec![event_id];
    let cause_evidence_ids = outcome_cause_evidence_ids(&advice);

    Ok(Some(ActionOutcome {
        outcome_id: format!("outcome-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        action: ControlActionKind::RetryAgent,
        status,
        observed_entropy_delta_evidence: entropy_delta_evidence_refs(
            &observed_entropy_delta,
            &cause_evidence_ids,
            &evidence_ids,
        ),
        observed_entropy_delta,
        expected_entropy_delta,
        evidence_ids,
        requirement_ids: advice.control_rationale.requirement_ids.clone(),
        note: Some(note),
    }))
}

fn retry_outcome_target_agent(advice: &AdviceRun) -> String {
    match &advice.final_action {
        ControlAction::RetryAgent {
            target_agent: Some(agent),
            ..
        } => agent.clone(),
        ControlAction::RetryAgent { .. } => advice.packet.target_agent.clone(),
        _ => String::new(),
    }
}

fn retry_failure_signatures_for_advice(
    store: &ProjectStore,
    advice: &AdviceRun,
    target_agent: &str,
) -> Result<HashSet<String>, StoreError> {
    let case_files = read_all_jsonl::<ControlCaseFile>(&store.root.join("case-files.jsonl"))?;
    let Some(case_file) = case_files
        .into_iter()
        .find(|case_file| case_file.case_file_id == advice.case_file_id)
    else {
        return Ok(HashSet::new());
    };
    let evidence_ids = case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .map(|score| score.evidence_ids.iter().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();
    if evidence_ids.is_empty() {
        return Ok(HashSet::new());
    }

    let events = read_all_jsonl::<Event>(&store.root.join("events.jsonl"))?;
    Ok(events
        .into_iter()
        .filter(|event| event.agent == target_agent)
        .filter(|event| event.kind == EventKind::CommandResult)
        .filter(|event| event.exit_code.is_some_and(|code| code != 0))
        .filter(|event| {
            event
                .event_id
                .as_ref()
                .is_some_and(|event_id| evidence_ids.contains(event_id))
        })
        .filter_map(|event| {
            event
                .command
                .map(|command| normalize_command_signature(&command))
        })
        .filter(|command| !is_verification_command(command))
        .collect())
}

fn retry_outcome_status_for_event(
    event: &Event,
    failed_signatures: &HashSet<String>,
) -> Option<OutcomeStatus> {
    let command = event.command.as_deref().map(normalize_command_signature)?;
    if is_verification_command(&command) {
        return None;
    }
    match event.exit_code {
        Some(0) if failed_signatures.contains(&command) => Some(OutcomeStatus::Succeeded),
        Some(_) if failed_signatures.contains(&command) => Some(OutcomeStatus::Failed),
        _ => None,
    }
}

fn latest_dispatch_matches_advice_packet(
    store: &ProjectStore,
    advice: &AdviceRun,
) -> Result<bool, StoreError> {
    Ok(
        read_all_jsonl::<DispatchResult>(&store.root.join("dispatch.jsonl"))?
            .into_iter()
            .next_back()
            .is_some_and(|dispatch| dispatch.packet_id == advice.packet.packet_id),
    )
}

fn force_verification_advice_matches_verifier(advice: &AdviceRun, run: &VerifierRun) -> bool {
    let suite = match &advice.final_action {
        ControlAction::ForceVerification { suite, .. } => *suite,
        _ => return false,
    };
    let command = run.command.trim();
    let instructions = advice
        .packet
        .instructions
        .iter()
        .map(|instruction| instruction.text.as_str())
        .collect::<Vec<_>>();
    let has_explicit_command = instructions.iter().any(|text| text.contains("Run `"));
    if has_explicit_command {
        return instructions
            .iter()
            .any(|text| text.contains(&format!("`{command}`")));
    }
    suite == VerificationSuite::Full || suite == VerificationSuite::Targeted
}

fn expected_entropy_delta_for_outcome(
    advice: &AdviceRun,
    default_kind: EntropyKind,
    default_delta: i16,
) -> Vec<EntropyDelta> {
    let advisor_delta_applies = matches!(
        &advice.validation_outcome,
        ValidationOutcome::Approved(action) if action == &advice.final_action
    );
    if advisor_delta_applies
        && let Some(mut deltas) = advice
            .advisor_decision
            .as_ref()
            .map(|decision| decision.expected_entropy_delta.clone())
        && !deltas.is_empty()
    {
        if !deltas.iter().any(|delta| delta.kind == default_kind) {
            deltas.push(EntropyDelta {
                kind: default_kind,
                delta: default_delta,
            });
        }
        return deltas;
    }
    vec![EntropyDelta {
        kind: default_kind,
        delta: default_delta,
    }]
}

fn observed_entropy_deltas_for_outcome(
    store: &ProjectStore,
    advice: &AdviceRun,
    expected_entropy_delta: &[EntropyDelta],
    default_kind: EntropyKind,
) -> Vec<EntropyDelta> {
    let before_case_file = read_all_jsonl::<ControlCaseFile>(&store.root.join("case-files.jsonl"))
        .ok()
        .and_then(|case_files| {
            case_files
                .into_iter()
                .find(|case_file| case_file.case_file_id == advice.case_file_id)
        });
    let after_case_file = DashboardSnapshot::load(store.root(), 500)
        .ok()
        .map(|snapshot| {
            let config = ProjectConfig::load(store.root()).unwrap_or_default();
            build_control_case_file_with_config(&store.workspace_root, &snapshot, &config)
        });
    let mut kinds = expected_entropy_delta
        .iter()
        .map(|delta| delta.kind)
        .collect::<BTreeSet<_>>();
    kinds.insert(default_kind);
    kinds
        .into_iter()
        .map(|kind| {
            let before = before_case_file
                .as_ref()
                .and_then(|case_file| entropy_score_value(case_file, kind))
                .unwrap_or_default();
            let after = after_case_file
                .as_ref()
                .and_then(|case_file| entropy_score_value(case_file, kind))
                .unwrap_or_default();
            EntropyDelta {
                kind,
                delta: after as i16 - before as i16,
            }
        })
        .collect()
}

fn entropy_delta_evidence_refs(
    observed_entropy_delta: &[EntropyDelta],
    cause_evidence_ids: &[String],
    result_evidence_ids: &[String],
) -> Vec<EntropyDeltaEvidence> {
    let evidence_ids = merged_evidence_ids(cause_evidence_ids, result_evidence_ids);
    if evidence_ids.is_empty() {
        return Vec::new();
    }
    observed_entropy_delta
        .iter()
        .map(|delta| EntropyDeltaEvidence {
            kind: delta.kind,
            evidence_ids: evidence_ids.clone(),
            cause_evidence_ids: cause_evidence_ids.to_vec(),
            result_evidence_ids: result_evidence_ids.to_vec(),
        })
        .collect()
}

fn outcome_cause_evidence_ids(advice: &AdviceRun) -> Vec<String> {
    merged_evidence_ids(
        &advice.control_rationale.evidence_ids,
        &advice.packet.evidence_refs,
    )
}

fn merged_evidence_ids(left: &[String], right: &[String]) -> Vec<String> {
    let mut ids = left
        .iter()
        .chain(right.iter())
        .filter(|id| !id.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids
}

fn entropy_score_value(case_file: &ControlCaseFile, kind: EntropyKind) -> Option<u8> {
    case_file.entropy.score(kind).map(|score| score.score)
}
