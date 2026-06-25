//! Durable memory loading and conflict resolution: assemble source-backed memory candidates and detect contradicting active claims.

use crate::*;

#[derive(Debug, Default)]
pub(crate) struct DurableMemoryLoad {
    pub(crate) memories: Vec<MemoryCandidate>,
    pub(crate) warnings: Vec<EvidenceItem>,
}

pub(crate) fn load_durable_memory(workspace: &Path) -> DurableMemoryLoad {
    let path = workspace.join(".agent-monitor/memories.jsonl");
    let (records, warnings) = read_durable_memory_records(&path);
    let mut warnings = warnings;
    let mut latest_by_id = HashMap::<String, (usize, MemoryCandidate)>::new();
    for (sequence, memory) in records {
        latest_by_id.insert(memory.memory_id.clone(), (sequence, memory));
    }
    let mut latest = latest_by_id.into_values().collect::<Vec<_>>();
    latest.sort_by(|(left, _), (right, _)| right.cmp(left));
    let conflicts = durable_memory_conflicts(&latest);
    let conflicted_ids = conflicts
        .iter()
        .flat_map(|conflict| {
            [
                conflict.left_memory_id.clone(),
                conflict.right_memory_id.clone(),
            ]
        })
        .collect::<HashSet<_>>();
    warnings.extend(
        conflicts
            .iter()
            .map(|conflict| durable_memory_conflict_evidence(&path, conflict)),
    );
    let memories = latest
        .into_iter()
        .map(|(_, memory)| memory)
        .filter(|memory| !conflicted_ids.contains(&memory.memory_id))
        .filter(memory_is_durable_active)
        .take(20)
        .collect();
    DurableMemoryLoad { memories, warnings }
}

pub(crate) fn latest_active_durable_memory_records(path: &Path) -> Vec<(usize, MemoryCandidate)> {
    let (records, _) = read_durable_memory_records(path);
    let mut latest_by_id = HashMap::<String, (usize, MemoryCandidate)>::new();
    for (sequence, memory) in records {
        latest_by_id.insert(memory.memory_id.clone(), (sequence, memory));
    }
    let mut latest = latest_by_id.into_values().collect::<Vec<_>>();
    latest.sort_by(|(left, _), (right, _)| right.cmp(left));
    latest
        .into_iter()
        .filter(|(_, memory)| memory_is_durable_active(memory))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableMemoryConflict {
    left_memory_id: String,
    right_memory_id: String,
    subject: String,
}

pub(crate) fn durable_memory_conflicts(
    records: &[(usize, MemoryCandidate)],
) -> Vec<DurableMemoryConflict> {
    let active = records
        .iter()
        .filter(|(_, memory)| memory_is_durable_active(memory))
        .map(|(_, memory)| memory)
        .collect::<Vec<_>>();
    let mut conflicts = Vec::new();
    for (left_index, left) in active.iter().enumerate() {
        for right in active.iter().skip(left_index + 1) {
            if let Some(subject) = memory_conflict_subject(left, right) {
                conflicts.push(DurableMemoryConflict {
                    left_memory_id: left.memory_id.clone(),
                    right_memory_id: right.memory_id.clone(),
                    subject,
                });
            }
        }
    }
    conflicts
}

pub(crate) fn memory_claims_conflict(left: &MemoryCandidate, right: &MemoryCandidate) -> bool {
    memory_conflict_subject(left, right).is_some()
}

pub(crate) fn memory_conflict_subject(
    left: &MemoryCandidate,
    right: &MemoryCandidate,
) -> Option<String> {
    if left.memory_id == right.memory_id {
        return None;
    }
    let left = memory_claim_polarity_subject(&left.claim)?;
    let right = memory_claim_polarity_subject(&right.claim)?;
    if left.subject == right.subject && left.deny != right.deny {
        Some(left.subject)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemoryClaimPolarity {
    deny: bool,
    subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RejectedAlternative {
    pub(crate) subject: String,
    pub(crate) evidence_id: String,
}

pub(crate) fn rejected_alternatives_from_intent_and_memory(
    intent_events: &[Event],
    durable_memory: &[MemoryCandidate],
) -> Vec<RejectedAlternative> {
    let mut rejected = Vec::new();
    for (index, event) in intent_events.iter().enumerate() {
        let Some(content) = event.content.as_deref() else {
            continue;
        };
        let evidence_id = event_evidence_id(event, index);
        for subject in rejected_alternative_subjects_from_text(content) {
            push_rejected_alternative(&mut rejected, subject, evidence_id.clone());
        }
    }
    for memory in durable_memory {
        let evidence_id = memory
            .evidence_ids
            .first()
            .cloned()
            .unwrap_or_else(|| memory.memory_id.clone());
        for subject in rejected_alternative_subjects_from_text(&memory.claim) {
            push_rejected_alternative(&mut rejected, subject, evidence_id.clone());
        }
    }
    rejected
}

pub(crate) fn push_rejected_alternative(
    rejected: &mut Vec<RejectedAlternative>,
    subject: String,
    evidence_id: String,
) {
    if subject.is_empty() || rejected.iter().any(|existing| existing.subject == subject) {
        return;
    }
    rejected.push(RejectedAlternative {
        subject,
        evidence_id,
    });
}

pub(crate) fn rejected_alternative_subjects_from_text(text: &str) -> Vec<String> {
    let mut subjects = Vec::new();
    for line in text.lines().map(strip_user_memory_line_prefix) {
        let normalized = normalize_memory_claim_for_conflict(line);
        if let Some(subject) = normalized
            .strip_prefix("rejected alternative ")
            .or_else(|| normalized.strip_prefix("rejected approach "))
            .or_else(|| normalized.strip_prefix("rejected design "))
        {
            push_unique_string(&mut subjects, &rejected_alternative_subject_key(subject));
            continue;
        }
        if let Some(polarity) = memory_claim_polarity_subject(line)
            && polarity.deny
        {
            push_unique_string(
                &mut subjects,
                &rejected_alternative_subject_key(&polarity.subject),
            );
        }
    }
    subjects
}

pub(crate) fn rejected_alternative_subject_key(subject: &str) -> String {
    let mut tokens = normalize_memory_claim_for_conflict(subject)
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    while tokens
        .first()
        .is_some_and(|token| rejected_alternative_leading_token(token))
    {
        tokens.remove(0);
    }
    tokens.join(" ")
}

pub(crate) fn rejected_alternative_leading_token(token: &str) -> bool {
    matches!(
        token,
        "create"
            | "add"
            | "introduce"
            | "use"
            | "preserve"
            | "keep"
            | "build"
            | "implement"
            | "make"
            | "a"
            | "an"
            | "the"
    )
}

pub(crate) fn event_reintroduces_rejected_alternative<'a>(
    event: &Event,
    evidence_id: &str,
    content: &str,
    rejected_alternatives: &'a [RejectedAlternative],
) -> Option<&'a RejectedAlternative> {
    if rejected_alternatives.is_empty() || event.kind == EventKind::UserInstruction {
        return None;
    }
    let text = normalized_event_text(event, content);
    if text.is_empty() {
        return None;
    }
    rejected_alternatives
        .iter()
        .filter(|rejected| rejected.evidence_id != evidence_id)
        .find(|rejected| {
            event_text_reintroduces_rejected_subject(&text, &rejected.subject)
                && !event_text_reaffirms_rejected_subject(&text, &rejected.subject)
        })
}

pub(crate) fn event_text_reintroduces_rejected_subject(text: &str, subject: &str) -> bool {
    !subject.is_empty()
        && (text.contains(subject)
            || subject
                .split_whitespace()
                .filter(|token| token.len() > 2)
                .all(|token| text.contains(token)))
}

pub(crate) fn event_text_reaffirms_rejected_subject(text: &str, subject: &str) -> bool {
    [
        "do not",
        "never",
        "must not",
        "should not",
        "avoid",
        "reject",
        "rejected",
    ]
    .iter()
    .any(|prefix| text.contains(&format!("{prefix} {subject}")))
}

pub(crate) fn normalized_event_text(event: &Event, content: &str) -> String {
    let mut text = String::new();
    if let Some(command) = event.command.as_deref() {
        text.push_str(command);
        text.push(' ');
    }
    text.push_str(content);
    text.push(' ');
    if let Some(rationale) = event.rationale.as_deref() {
        text.push_str(rationale);
    }
    normalize_memory_claim_for_conflict(&text)
}

pub(crate) fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

pub(crate) fn memory_claim_polarity_subject(claim: &str) -> Option<MemoryClaimPolarity> {
    let normalized = normalize_memory_claim_for_conflict(claim);
    if normalized.is_empty() {
        return None;
    }
    for prefix in [
        "do not ",
        "never ",
        "must not ",
        "should not ",
        "dont ",
        "don t ",
    ] {
        if let Some(subject) = normalized.strip_prefix(prefix) {
            let subject = subject.trim().to_string();
            if !subject.is_empty() {
                return Some(MemoryClaimPolarity {
                    deny: true,
                    subject,
                });
            }
        }
    }
    Some(MemoryClaimPolarity {
        deny: false,
        subject: normalized,
    })
}

pub(crate) fn normalize_memory_claim_for_conflict(claim: &str) -> String {
    claim
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn durable_memory_conflict_evidence(
    path: &Path,
    conflict: &DurableMemoryConflict,
) -> EvidenceItem {
    let issue = format!(
        "Durable memory conflict between {} and {} on subject '{}'; both memories quarantined",
        conflict.left_memory_id, conflict.right_memory_id, conflict.subject
    );
    let (summary, redaction_status) = sanitize_evidence_summary(&issue);
    EvidenceItem {
        id: format!(
            "memory-conflict-{}-{}",
            safe_slug(&conflict.left_memory_id),
            safe_slug(&conflict.right_memory_id)
        ),
        kind: "memory_conflict".into(),
        agent: None,
        session: None,
        run_id: None,
        agent_session_id: None,
        summary: truncate_evidence(&summary),
        redaction_status,
        source: Some(path.display().to_string()),
        source_type: Some("memory".into()),
        source_path: Some(path.display().to_string()),
        source_offset: None,
        source_hash: None,
        redaction_rules: Vec::new(),
    }
}

pub(crate) fn read_durable_memory_records(
    path: &Path,
) -> (Vec<(usize, MemoryCandidate)>, Vec<EvidenceItem>) {
    if !path.exists() {
        return (Vec::new(), Vec::new());
    }

    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(source) => {
            return (
                Vec::new(),
                vec![durable_memory_load_warning(
                    path,
                    None,
                    &format!("could not read durable memory log: {source}"),
                )],
            );
        }
    };

    let reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut warnings = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        match line {
            Ok(line) if !line.trim().is_empty() => lines.push((index + 1, line)),
            Ok(_) => {}
            Err(source) => warnings.push(durable_memory_load_warning(
                path,
                Some(index + 1),
                &format!("could not read durable memory log line: {source}"),
            )),
        }
    }

    let last_line_number = lines.last().map(|(line_number, _)| *line_number);
    let mut records = Vec::new();
    for (line_number, line) in lines {
        match serde_json::from_str::<MemoryCandidate>(&line) {
            Ok(memory) => records.push((line_number, memory)),
            Err(source) if Some(line_number) == last_line_number && source.is_eof() => {
                warnings.push(durable_memory_load_warning(
                    path,
                    Some(line_number),
                    "trailing partial durable memory record ignored",
                ));
                break;
            }
            Err(source) => warnings.push(durable_memory_load_warning(
                path,
                Some(line_number),
                &format!("malformed durable memory record skipped: {source}"),
            )),
        }
    }

    (records, warnings)
}

pub(crate) fn durable_memory_load_warning(
    path: &Path,
    line_number: Option<usize>,
    issue: &str,
) -> EvidenceItem {
    let id = line_number
        .map(|line| format!("memory-load-warning-line-{line}"))
        .unwrap_or_else(|| "memory-load-warning-read".into());
    let location = line_number
        .map(|line| format!("line {line}"))
        .unwrap_or_else(|| "file".into());
    let (summary, redaction_status) =
        sanitize_evidence_summary(&format!("Durable memory log {location}: {issue}"));
    EvidenceItem {
        id,
        kind: "memory_load_warning".into(),
        agent: None,
        session: None,
        run_id: None,
        agent_session_id: None,
        summary: truncate_evidence(&summary),
        redaction_status,
        source: Some(path.display().to_string()),
        source_type: Some("memory".into()),
        source_path: Some(path.display().to_string()),
        source_offset: line_number.map(|line| line as u64),
        source_hash: None,
        redaction_rules: Vec::new(),
    }
}

pub(crate) fn memory_is_durable_active(memory: &MemoryCandidate) -> bool {
    memory.status == MemoryStatus::Active
        && matches!(
            memory.source,
            MemorySource::User | MemorySource::VerifiedResult | MemorySource::ManualReview
        )
        && !packet_text_is_tainted(&memory.claim)
}
