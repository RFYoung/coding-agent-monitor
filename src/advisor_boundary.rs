use crate::*;
use std::collections::HashSet;

pub fn validate_advisor_decision(
    decision: &AdvisorDecision,
    case_file: &ControlCaseFile,
) -> Result<(), AdvisorValidationError> {
    let known_evidence = case_file
        .evidence
        .iter()
        .map(|item| item.id.as_str())
        .collect::<HashSet<_>>();
    if !decision
        .entropy_scores
        .contains_key(&decision.dominant_entropy)
    {
        return Err(AdvisorValidationError::MissingDominantEntropyScore {
            kind: decision.dominant_entropy,
        });
    }
    for (kind, estimate) in &decision.entropy_scores {
        if estimate.score > 100 || estimate.confidence > 100 {
            return Err(AdvisorValidationError::InvalidEntropyScore { kind: *kind });
        }
    }
    let mut expected_delta_kinds = HashSet::new();
    for delta in &decision.expected_entropy_delta {
        if !(-100..=100).contains(&delta.delta) {
            return Err(AdvisorValidationError::InvalidExpectedEntropyDelta {
                kind: delta.kind,
                delta: delta.delta,
            });
        }
        if !expected_delta_kinds.insert(delta.kind) {
            return Err(AdvisorValidationError::DuplicateExpectedEntropyDelta { kind: delta.kind });
        }
    }
    if !decision.confidence.is_finite() || !(0.0..=1.0).contains(&decision.confidence) {
        return Err(AdvisorValidationError::InvalidConfidence);
    }
    if decision.proposed_action.kind() == ControlActionKind::Pause {
        return Err(AdvisorValidationError::ForbiddenAction {
            action: ControlActionKind::Pause,
        });
    }
    if advisor_decision_text_is_tainted(decision) {
        return Err(AdvisorValidationError::TaintedPacket);
    }
    for evidence_id in decision
        .cited_evidence_ids
        .iter()
        .chain(
            decision
                .top_evidence
                .iter()
                .map(|evidence| &evidence.event_id),
        )
        .chain(decision.packet_draft.evidence_refs.iter())
    {
        if !known_evidence.contains(evidence_id.as_str()) {
            return Err(AdvisorValidationError::UnknownEvidence {
                evidence_id: evidence_id.clone(),
            });
        }
    }
    if !case_file
        .allowed_actions
        .contains(&decision.proposed_action.kind())
    {
        return Err(AdvisorValidationError::ForbiddenAction {
            action: decision.proposed_action.kind(),
        });
    }
    if let ControlAction::RunProbe { probe } = &decision.proposed_action
        && !advisor_supported_probe_spec(probe)
    {
        return Err(AdvisorValidationError::UnsupportedProbeSpec {
            kind: probe_spec_label(probe),
        });
    }
    for target_agent in control_action_target_agents(&decision.proposed_action) {
        match adapter_capabilities_for_case_file(case_file, target_agent) {
            Some(capabilities) if capabilities.enabled && capabilities.can_inject_context => {}
            Some(capabilities) if !capabilities.enabled => {
                return Err(AdvisorValidationError::UnsupportedTargetAgent {
                    agent: target_agent.to_string(),
                    reason: "adapter is disabled".into(),
                });
            }
            Some(_) => {
                return Err(AdvisorValidationError::UnsupportedTargetAgent {
                    agent: target_agent.to_string(),
                    reason: "adapter cannot receive injected monitor packets".into(),
                });
            }
            None => {
                return Err(AdvisorValidationError::UnsupportedTargetAgent {
                    agent: target_agent.to_string(),
                    reason: "adapter capabilities are unknown".into(),
                });
            }
        }
    }
    Ok(())
}

fn advisor_supported_probe_spec(probe: &ProbeSpec) -> bool {
    matches!(
        probe,
        ProbeSpec::LocalEvidence { .. }
            | ProbeSpec::RuntimeValidation { .. }
            | ProbeSpec::RepoInspection { .. }
            | ProbeSpec::TargetedTest { .. }
    )
}

fn probe_spec_label(probe: &ProbeSpec) -> &'static str {
    match probe {
        ProbeSpec::LocalEvidence { .. } => "local_evidence",
        ProbeSpec::RuntimeValidation { .. } => "runtime_validation",
        ProbeSpec::RepoInspection { .. } => "repo_inspection",
        ProbeSpec::TargetedTest { .. } => "targeted_test",
        ProbeSpec::BrowserValidation { .. } => "browser_validation",
    }
}

fn advisor_decision_text_is_tainted(decision: &AdvisorDecision) -> bool {
    advisor_decision_text_fields(decision)
        .iter()
        .any(|text| packet_text_is_tainted(text))
}

fn advisor_decision_text_fields(decision: &AdvisorDecision) -> Vec<String> {
    let mut fields = Vec::new();
    if let Some(diagnosis_id) = &decision.diagnosis_id {
        fields.push(diagnosis_id.clone());
    }
    fields.extend(
        decision
            .top_evidence
            .iter()
            .map(|evidence| evidence.event_id.clone()),
    );
    fields.extend(
        decision
            .top_evidence
            .iter()
            .map(|evidence| evidence.why_it_matters.clone()),
    );
    fields.extend(decision.cited_evidence_ids.iter().cloned());
    fields.extend(decision.missing_evidence.iter().cloned());
    if let Some(packet_intent) = &decision.packet_intent {
        fields.push(packet_intent.clone());
    }
    fields.push(decision.packet_draft.summary.clone());
    fields.extend(decision.packet_draft.instructions.iter().cloned());
    fields.extend(decision.packet_draft.evidence_refs.iter().cloned());
    fields.extend(control_action_text_fields(&decision.proposed_action));
    if let Some(ask_user) = &decision.ask_user {
        fields.push(ask_user.to_string());
    }
    if !decision.raw.is_null() {
        fields.push(decision.raw.to_string());
    }
    fields
}

fn control_action_text_fields(action: &ControlAction) -> Vec<String> {
    match action {
        ControlAction::ContinueWorking => Vec::new(),
        ControlAction::RetryAgent { target_agent, .. }
        | ControlAction::SendFollowUp { target_agent }
        | ControlAction::SpawnJudgeAgent { target_agent }
        | ControlAction::SpawnFreshAgent { target_agent } => target_agent.iter().cloned().collect(),
        ControlAction::ForceVerification { .. } => Vec::new(),
        ControlAction::RunProbe { probe } => probe_text_fields(probe),
        ControlAction::BlockProgressUntilTraceAndVerification { reason } => vec![reason.clone()],
        ControlAction::SwitchAgent { target_agent } => vec![target_agent.clone()],
        ControlAction::AskUser { question } => vec![question.clone()],
        ControlAction::Pause { reason } => vec![reason.clone()],
    }
}

fn probe_text_fields(probe: &ProbeSpec) -> Vec<String> {
    match probe {
        ProbeSpec::LocalEvidence { target }
        | ProbeSpec::BrowserValidation { target }
        | ProbeSpec::RepoInspection { target } => target.iter().cloned().collect(),
        ProbeSpec::RuntimeValidation { surface, target } => {
            let mut fields = vec![
                surface.kind_label().to_string(),
                surface.label().to_string(),
            ];
            fields.extend(target.iter().cloned());
            fields
        }
        ProbeSpec::TargetedTest { command } => vec![command.clone()],
    }
}

pub(super) fn control_action_target_agents(action: &ControlAction) -> Vec<&str> {
    match action {
        ControlAction::RetryAgent {
            target_agent: Some(agent),
            ..
        }
        | ControlAction::SendFollowUp {
            target_agent: Some(agent),
        }
        | ControlAction::SpawnJudgeAgent {
            target_agent: Some(agent),
        }
        | ControlAction::SpawnFreshAgent {
            target_agent: Some(agent),
        } => vec![agent.as_str()],
        ControlAction::SwitchAgent { target_agent } => vec![target_agent.as_str()],
        _ => Vec::new(),
    }
}

pub(super) fn bound_case_file_for_advisor(
    case_file: &ControlCaseFile,
    max_input_tokens: u32,
) -> ControlCaseFile {
    const APPROX_CHARS_PER_TOKEN: usize = 4;
    const MIN_CASE_FILE_CHARS: usize = 1024;
    const COMPACT_EVIDENCE_CHARS: usize = 80;

    let max_chars = (max_input_tokens as usize)
        .saturating_mul(APPROX_CHARS_PER_TOKEN)
        .max(MIN_CASE_FILE_CHARS);
    let mut bounded = sanitize_case_file_for_advisor(case_file);
    let protected = protected_advisor_evidence_ids(&bounded);

    while case_file_json_len(&bounded) > max_chars
        && remove_low_priority_evidence(&mut bounded, &protected)
    {}

    if case_file_json_len(&bounded) > max_chars {
        for evidence in &mut bounded.evidence {
            evidence.summary = truncate_chars(&evidence.summary, COMPACT_EVIDENCE_CHARS);
        }
    }

    while case_file_json_len(&bounded) > max_chars && !bounded.evidence.is_empty() {
        bounded.evidence.pop();
    }

    prune_case_file_references_to_visible_evidence(&mut bounded);
    bounded
}

fn sanitize_case_file_for_advisor(case_file: &ControlCaseFile) -> ControlCaseFile {
    let mut sanitized = case_file.clone();
    sanitized
        .evidence
        .retain(|evidence| evidence.redaction_status != RedactionStatus::Tainted);

    for evidence in &mut sanitized.evidence {
        evidence.summary = sanitize_advisor_text(&evidence.summary);
        if let Some(source) = &mut evidence.source {
            *source = sanitize_advisor_text(source);
        }
        if let Some(source_type) = &mut evidence.source_type {
            *source_type = sanitize_advisor_text(source_type);
        }
        if let Some(source_path) = &mut evidence.source_path {
            *source_path = sanitize_advisor_text(source_path);
        }
        if let Some(source_hash) = &mut evidence.source_hash {
            *source_hash = sanitize_advisor_text(source_hash);
        }
        for rule in &mut evidence.redaction_rules {
            *rule = sanitize_advisor_text(rule);
        }
    }
    for belief in &mut sanitized.belief_state.hypotheses {
        belief.rationale = sanitize_advisor_text(&belief.rationale);
        for missing in &mut belief.missing_evidence {
            *missing = sanitize_advisor_text(missing);
        }
    }

    for memory in &mut sanitized.memory_candidates {
        memory.claim = sanitize_advisor_text(&memory.claim);
    }
    for memory in &mut sanitized.durable_memory {
        memory.claim = sanitize_advisor_text(&memory.claim);
    }
    for requirement in &mut sanitized.requirements {
        requirement.requirement_id = sanitize_advisor_text(&requirement.requirement_id);
        requirement.text = sanitize_advisor_text(&requirement.text);
        if let Some(source_event_id) = &mut requirement.source_event_id {
            *source_event_id = sanitize_advisor_text(source_event_id);
        }
        for evidence_id in &mut requirement.evidence_ids {
            *evidence_id = sanitize_advisor_text(evidence_id);
        }
        for verifier_id in &mut requirement.verifier_ids {
            *verifier_id = sanitize_advisor_text(verifier_id);
        }
        for command in &mut requirement.verifier_commands {
            *command = sanitize_advisor_text(command);
        }
        if let Some(evidence_id) = &mut requirement.latest_verification_evidence_id {
            *evidence_id = sanitize_advisor_text(evidence_id);
        }
    }

    sanitize_task_summary_for_advisor(&mut sanitized.task);
    sanitize_verification_summary_for_advisor(&mut sanitized.verification);

    if let Some(repo_audit) = &mut sanitized.repo_audit {
        for change in &mut repo_audit.changes {
            change.matching_traces.clear();
        }
    }

    sanitized
}

fn sanitize_task_summary_for_advisor(task: &mut TaskSummary) {
    if let Some(goal) = &mut task.user_goal {
        *goal = sanitize_advisor_text(goal);
    }
    if let Some(event_id) = &mut task.user_goal_event_id {
        *event_id = sanitize_advisor_text(event_id);
    }
    for criterion in &mut task.acceptance_criteria {
        criterion.id = sanitize_advisor_text(&criterion.id);
        criterion.text = sanitize_advisor_text(&criterion.text);
        if let Some(event_id) = &mut criterion.source_event_id {
            *event_id = sanitize_advisor_text(event_id);
        }
    }
    for marker in &mut task.ambiguity_markers {
        marker.text = sanitize_advisor_text(&marker.text);
        if let Some(event_id) = &mut marker.source_event_id {
            *event_id = sanitize_advisor_text(event_id);
        }
    }
}

fn sanitize_verification_summary_for_advisor(summary: &mut VerificationSummary) {
    for command in &mut summary.recommended_commands {
        *command = sanitize_advisor_text(command);
    }
    for file in &mut summary.changed_source_files {
        *file = sanitize_advisor_text(file);
    }
    for criterion in &mut summary.acceptance_criteria {
        *criterion = sanitize_advisor_text(criterion);
    }
    for criterion in &mut summary.uncovered_acceptance_criteria {
        *criterion = sanitize_advisor_text(criterion);
    }
    for coverage in &mut summary.acceptance_coverage {
        coverage.criterion = sanitize_advisor_text(&coverage.criterion);
        for verifier_id in &mut coverage.verifier_ids {
            *verifier_id = sanitize_advisor_text(verifier_id);
        }
        for command in &mut coverage.verifier_commands {
            *command = sanitize_advisor_text(command);
        }
    }
    if let Some(command) = &mut summary.latest_passing_command {
        *command = sanitize_advisor_text(command);
    }
    if let Some(command) = &mut summary.latest_failing_command {
        *command = sanitize_advisor_text(command);
    }
}

fn sanitize_advisor_text(value: &str) -> String {
    let (sanitized, _) = sanitize_evidence_summary(value);
    truncate_evidence(&sanitized)
}

fn prune_case_file_references_to_visible_evidence(case_file: &mut ControlCaseFile) {
    let visible = case_file
        .evidence
        .iter()
        .map(|item| item.id.clone())
        .collect::<HashSet<_>>();

    for score in &mut case_file.entropy.scores {
        score
            .evidence_ids
            .retain(|evidence_id| visible.contains(evidence_id));
    }
    for belief in &mut case_file.belief_state.hypotheses {
        belief
            .evidence_ids
            .retain(|evidence_id| visible.contains(evidence_id));
    }

    for memory in &mut case_file.memory_candidates {
        memory
            .evidence_ids
            .retain(|evidence_id| visible.contains(evidence_id));
    }
    for memory in &mut case_file.durable_memory {
        memory
            .evidence_ids
            .retain(|evidence_id| visible.contains(evidence_id));
    }
    for requirement in &mut case_file.requirements {
        requirement
            .evidence_ids
            .retain(|evidence_id| visible.contains(evidence_id));
        if requirement
            .source_event_id
            .as_ref()
            .is_some_and(|evidence_id| !visible.contains(evidence_id))
        {
            requirement.source_event_id = None;
        }
        if requirement
            .latest_verification_evidence_id
            .as_ref()
            .is_some_and(|evidence_id| !visible.contains(evidence_id))
        {
            requirement.latest_verification_evidence_id = None;
        }
    }
    if case_file
        .task
        .user_goal_event_id
        .as_ref()
        .is_some_and(|evidence_id| !visible.contains(evidence_id))
    {
        case_file.task.user_goal_event_id = None;
    }
    for criterion in &mut case_file.task.acceptance_criteria {
        if criterion
            .source_event_id
            .as_ref()
            .is_some_and(|evidence_id| !visible.contains(evidence_id))
        {
            criterion.source_event_id = None;
        }
    }
    for marker in &mut case_file.task.ambiguity_markers {
        if marker
            .source_event_id
            .as_ref()
            .is_some_and(|evidence_id| !visible.contains(evidence_id))
        {
            marker.source_event_id = None;
        }
    }
    case_file
        .memory_candidates
        .retain(|memory| !memory.evidence_ids.is_empty());
}

fn protected_advisor_evidence_ids(case_file: &ControlCaseFile) -> HashSet<String> {
    case_file
        .entropy
        .scores
        .iter()
        .flat_map(|score| score.evidence_ids.iter().cloned())
        .chain(
            case_file
                .belief_state
                .hypotheses
                .iter()
                .flat_map(|belief| belief.evidence_ids.iter().cloned()),
        )
        .chain(
            case_file
                .requirements
                .iter()
                .flat_map(|requirement| requirement.evidence_ids.iter().cloned()),
        )
        .chain(case_file.task.user_goal_event_id.iter().cloned())
        .chain(
            case_file
                .task
                .acceptance_criteria
                .iter()
                .filter_map(|criterion| criterion.source_event_id.clone()),
        )
        .chain(
            case_file
                .task
                .ambiguity_markers
                .iter()
                .filter_map(|marker| marker.source_event_id.clone()),
        )
        .collect()
}

fn remove_low_priority_evidence(
    case_file: &mut ControlCaseFile,
    protected: &HashSet<String>,
) -> bool {
    if case_file.evidence.is_empty() {
        return false;
    }
    if let Some(index) = case_file
        .evidence
        .iter()
        .enumerate()
        .filter(|(_, item)| !protected.contains(&item.id))
        .min_by(|(left_index, left), (right_index, right)| {
            evidence_salience_score(left, protected)
                .cmp(&evidence_salience_score(right, protected))
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(index, _)| index)
    {
        case_file.evidence.remove(index);
        true
    } else {
        remove_lowest_salience_evidence(case_file, protected);
        true
    }
}

fn remove_lowest_salience_evidence(case_file: &mut ControlCaseFile, protected: &HashSet<String>) {
    let Some(index) = case_file
        .evidence
        .iter()
        .enumerate()
        .min_by(|(left_index, left), (right_index, right)| {
            evidence_salience_score(left, protected)
                .cmp(&evidence_salience_score(right, protected))
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(index, _)| index)
    else {
        return;
    };
    case_file.evidence.remove(index);
}

fn evidence_salience_score(evidence: &EvidenceItem, protected: &HashSet<String>) -> i32 {
    let mut score = 0;
    if protected.contains(&evidence.id) {
        score += 10_000;
    }
    score += evidence_kind_salience(&evidence.kind);
    score += evidence_summary_salience(&evidence.summary);
    if evidence.source_type.as_deref() == Some("git") || evidence.source_path.is_some() {
        score += 10;
    }
    if evidence.source.is_some() {
        score += 5;
    }
    score
}

fn evidence_kind_salience(kind: &str) -> i32 {
    match kind {
        "UserInstruction" => 90,
        "DesignThought" => 70,
        "FileChange" | "RepoDiff" | "repo_audit" => 65,
        "TestResult" | "VerifierRun" => 60,
        "InterventionResult" | "AgentHealth" => 55,
        "ToolResult" | "CommandResult" => 35,
        "ToolCall" | "CommandOutput" => 15,
        "ModelMessage" => 10,
        _ => 20,
    }
}

fn evidence_summary_salience(summary: &str) -> i32 {
    let text = summary.to_lowercase();
    let mut score = 0;
    if [
        "acceptance criterion",
        "acceptance criteria",
        "requirement",
        "user asked",
        "must",
        "constraint",
        "invariant",
    ]
    .iter()
    .any(|signal| text.contains(signal))
    {
        score += 60;
    }
    if [
        "failed",
        "failure",
        "error",
        "panic",
        "timeout",
        "unverified",
        "stale",
        "dirty",
        "untraced",
        "missing rationale",
    ]
    .iter()
    .any(|signal| text.contains(signal))
    {
        score += 45;
    }
    if ["changed", "wrote", "patched", "edited", "diff"]
        .iter()
        .any(|signal| text.contains(signal))
    {
        score += 25;
    }
    score
}

fn case_file_json_len(case_file: &ControlCaseFile) -> usize {
    serde_json::to_string(case_file)
        .map(|json| json.len())
        .unwrap_or(usize::MAX)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let prefix = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{}…", prefix.trim_end())
}
