use crate::{
    ActionOutcome, AdviceRun, ControlActionKind, EntropyDelta, EntropyDeltaEvidence, EntropyKind,
    OutcomeStatus, StoreError, read_all_jsonl,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalibrationQuery {
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<ControlActionKind>,
}

impl Default for CalibrationQuery {
    fn default() -> Self {
        Self {
            limit: 25,
            action: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalibrationReport {
    pub workspace: String,
    pub advice_count: usize,
    pub outcome_count: usize,
    pub unresolved_advice_count: usize,
    pub actions: Vec<ActionCalibration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<ActionTargetCalibration>,
    pub recent_outcomes: Vec<CalibrationOutcomeSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionCalibration {
    pub action: ControlActionKind,
    pub advice_count: usize,
    pub outcome_count: usize,
    pub unresolved_advice_count: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub unknown: usize,
    pub expected_entropy_delta: Vec<EntropyDelta>,
    pub observed_entropy_delta: Vec<EntropyDelta>,
    pub absolute_error: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionTargetCalibration {
    pub action: ControlActionKind,
    pub target_agent: String,
    pub advice_count: usize,
    pub outcome_count: usize,
    pub unresolved_advice_count: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub unknown: usize,
    pub expected_entropy_delta: Vec<EntropyDelta>,
    pub observed_entropy_delta: Vec<EntropyDelta>,
    pub absolute_error: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalibrationOutcomeSample {
    pub advice_id: String,
    pub outcome_id: String,
    pub action: ControlActionKind,
    pub status: OutcomeStatus,
    pub expected_entropy_delta: Vec<EntropyDelta>,
    pub observed_entropy_delta: Vec<EntropyDelta>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_entropy_delta_evidence: Vec<EntropyDeltaEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub necessary_evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub correlated_evidence_ids: Vec<String>,
    pub absolute_error: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Default)]
struct ActionAccumulator {
    advice_ids: BTreeSet<String>,
    outcome_advice_ids: BTreeSet<String>,
    outcome_count: usize,
    succeeded: usize,
    failed: usize,
    unknown: usize,
    expected: BTreeMap<EntropyKind, i32>,
    observed: BTreeMap<EntropyKind, i32>,
    absolute_error: i32,
}

pub fn load_calibration_report(
    workspace: impl AsRef<Path>,
    query: CalibrationQuery,
) -> Result<CalibrationReport, StoreError> {
    let workspace = workspace.as_ref();
    let store_root = workspace.join(".agent-monitor");
    let advice_records = read_all_jsonl::<AdviceRun>(&store_root.join("advice.jsonl"))?;
    let outcomes = read_all_jsonl::<ActionOutcome>(&store_root.join("outcomes.jsonl"))?;
    let mut actions = HashMap::<ControlActionKind, ActionAccumulator>::new();
    let mut targets = HashMap::<(ControlActionKind, String), ActionAccumulator>::new();
    let mut advice_targets = HashMap::<String, (ControlActionKind, String)>::new();

    for advice in &advice_records {
        let action = advice.final_action.kind();
        if query.action.is_some_and(|filter| filter != action) {
            continue;
        }
        let target_agent = target_agent_for_advice(advice);
        advice_targets.insert(advice.advice_id.clone(), (action, target_agent.clone()));
        actions
            .entry(action)
            .or_default()
            .advice_ids
            .insert(advice.advice_id.clone());
        targets
            .entry((action, target_agent))
            .or_default()
            .advice_ids
            .insert(advice.advice_id.clone());
    }

    let mut outcome_count = 0;
    let mut recent_outcomes = Vec::new();
    for outcome in &outcomes {
        if query.action.is_some_and(|filter| filter != outcome.action) {
            continue;
        }
        outcome_count += 1;
        let absolute_error = entropy_delta_absolute_error(
            &outcome.expected_entropy_delta,
            &outcome.observed_entropy_delta,
        );
        let accumulator = actions.entry(outcome.action).or_default();
        accumulator.outcome_count += 1;
        accumulator
            .outcome_advice_ids
            .insert(outcome.advice_id.clone());
        match outcome.status {
            OutcomeStatus::Succeeded => accumulator.succeeded += 1,
            OutcomeStatus::Failed => accumulator.failed += 1,
            OutcomeStatus::Unknown => accumulator.unknown += 1,
        }
        add_entropy_deltas(&mut accumulator.expected, &outcome.expected_entropy_delta);
        add_entropy_deltas(&mut accumulator.observed, &outcome.observed_entropy_delta);
        accumulator.absolute_error += absolute_error;
        let (target_action, target_agent) = advice_targets
            .get(&outcome.advice_id)
            .cloned()
            .unwrap_or_else(|| (outcome.action, "<unknown>".into()));
        let target_accumulator = targets.entry((target_action, target_agent)).or_default();
        target_accumulator.outcome_count += 1;
        target_accumulator
            .outcome_advice_ids
            .insert(outcome.advice_id.clone());
        match outcome.status {
            OutcomeStatus::Succeeded => target_accumulator.succeeded += 1,
            OutcomeStatus::Failed => target_accumulator.failed += 1,
            OutcomeStatus::Unknown => target_accumulator.unknown += 1,
        }
        add_entropy_deltas(
            &mut target_accumulator.expected,
            &outcome.expected_entropy_delta,
        );
        add_entropy_deltas(
            &mut target_accumulator.observed,
            &outcome.observed_entropy_delta,
        );
        target_accumulator.absolute_error += absolute_error;
        let (necessary_evidence_ids, correlated_evidence_ids) =
            calibration_outcome_evidence_attribution(outcome);
        recent_outcomes.push(CalibrationOutcomeSample {
            advice_id: outcome.advice_id.clone(),
            outcome_id: outcome.outcome_id.clone(),
            action: outcome.action,
            status: outcome.status,
            expected_entropy_delta: outcome.expected_entropy_delta.clone(),
            observed_entropy_delta: outcome.observed_entropy_delta.clone(),
            observed_entropy_delta_evidence: outcome.observed_entropy_delta_evidence.clone(),
            necessary_evidence_ids,
            correlated_evidence_ids,
            absolute_error,
            note: outcome.note.clone(),
        });
    }

    recent_outcomes.reverse();
    recent_outcomes.truncate(query.limit);

    let mut action_summaries = actions
        .into_iter()
        .map(|(action, accumulator)| action_calibration_from_accumulator(action, accumulator))
        .collect::<Vec<_>>();
    action_summaries.sort_by_key(|summary| summary.action);
    let mut target_summaries = targets
        .into_iter()
        .map(|((action, target_agent), accumulator)| {
            target_calibration_from_accumulator(action, target_agent, accumulator)
        })
        .collect::<Vec<_>>();
    target_summaries.sort_by(|left, right| {
        left.action
            .cmp(&right.action)
            .then_with(|| left.target_agent.cmp(&right.target_agent))
    });

    let advice_count = action_summaries
        .iter()
        .map(|action| action.advice_count)
        .sum();
    let unresolved_advice_count = action_summaries
        .iter()
        .map(|action| action.unresolved_advice_count)
        .sum();

    Ok(CalibrationReport {
        workspace: workspace.display().to_string(),
        advice_count,
        outcome_count,
        unresolved_advice_count,
        actions: action_summaries,
        targets: target_summaries,
        recent_outcomes,
    })
}

fn target_agent_for_advice(advice: &AdviceRun) -> String {
    let target = advice.packet.target_agent.trim();
    if target.is_empty() {
        "<unknown>".into()
    } else {
        target.into()
    }
}

fn action_calibration_from_accumulator(
    action: ControlActionKind,
    accumulator: ActionAccumulator,
) -> ActionCalibration {
    let unresolved_advice_count = accumulator
        .advice_ids
        .difference(&accumulator.outcome_advice_ids)
        .count();
    ActionCalibration {
        action,
        advice_count: accumulator.advice_ids.len(),
        outcome_count: accumulator.outcome_count,
        unresolved_advice_count,
        succeeded: accumulator.succeeded,
        failed: accumulator.failed,
        unknown: accumulator.unknown,
        expected_entropy_delta: entropy_map_to_deltas(accumulator.expected),
        observed_entropy_delta: entropy_map_to_deltas(accumulator.observed),
        absolute_error: accumulator.absolute_error,
    }
}

fn target_calibration_from_accumulator(
    action: ControlActionKind,
    target_agent: String,
    accumulator: ActionAccumulator,
) -> ActionTargetCalibration {
    let summary = action_calibration_from_accumulator(action, accumulator);
    ActionTargetCalibration {
        action: summary.action,
        target_agent,
        advice_count: summary.advice_count,
        outcome_count: summary.outcome_count,
        unresolved_advice_count: summary.unresolved_advice_count,
        succeeded: summary.succeeded,
        failed: summary.failed,
        unknown: summary.unknown,
        expected_entropy_delta: summary.expected_entropy_delta,
        observed_entropy_delta: summary.observed_entropy_delta,
        absolute_error: summary.absolute_error,
    }
}

fn add_entropy_deltas(target: &mut BTreeMap<EntropyKind, i32>, deltas: &[EntropyDelta]) {
    for delta in deltas {
        *target.entry(delta.kind).or_default() += i32::from(delta.delta);
    }
}

fn entropy_map_to_deltas(map: BTreeMap<EntropyKind, i32>) -> Vec<EntropyDelta> {
    map.into_iter()
        .map(|(kind, delta)| EntropyDelta {
            kind,
            delta: delta.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        })
        .collect()
}

fn entropy_delta_absolute_error(expected: &[EntropyDelta], observed: &[EntropyDelta]) -> i32 {
    let mut expected_map = BTreeMap::new();
    let mut observed_map = BTreeMap::new();
    add_entropy_deltas(&mut expected_map, expected);
    add_entropy_deltas(&mut observed_map, observed);
    let kinds = expected_map
        .keys()
        .chain(observed_map.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    kinds
        .into_iter()
        .map(|kind| {
            let expected = expected_map.get(&kind).copied().unwrap_or_default();
            let observed = observed_map.get(&kind).copied().unwrap_or_default();
            (expected - observed).abs()
        })
        .sum()
}

fn calibration_outcome_evidence_attribution(outcome: &ActionOutcome) -> (Vec<String>, Vec<String>) {
    let mut necessary = outcome
        .observed_entropy_delta_evidence
        .iter()
        .flat_map(|evidence| evidence.result_evidence_ids.iter())
        .cloned()
        .collect::<Vec<_>>();
    if necessary.is_empty() {
        necessary.extend(outcome.evidence_ids.iter().cloned());
    }
    dedup_evidence_ids(&mut necessary);

    let mut correlated = outcome
        .observed_entropy_delta_evidence
        .iter()
        .flat_map(|evidence| evidence.cause_evidence_ids.iter())
        .filter(|id| !necessary.iter().any(|necessary_id| necessary_id == *id))
        .cloned()
        .collect::<Vec<_>>();
    dedup_evidence_ids(&mut correlated);

    (necessary, correlated)
}

fn dedup_evidence_ids(ids: &mut Vec<String>) {
    ids.retain(|id| !id.trim().is_empty());
    ids.sort();
    ids.dedup();
}
