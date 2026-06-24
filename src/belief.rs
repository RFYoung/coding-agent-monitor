use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    CompletionCertificate, EntropyKind, EntropyScore, EntropyVector, RequirementNode,
    VerificationStatus, VerificationSummary,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum FailureHypothesisKind {
    RequirementClosureGap,
    RequirementScopeGap,
    StaleVerification,
    WeakTestOracle,
    SubagentLifecycleGap,
    ProcessConformanceGap,
    ContextLoss,
    RepoAttributionGap,
    AgentLoop,
    OperationalInstability,
    UserAuthorityGap,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailureHypothesisBelief {
    pub kind: FailureHypothesisKind,
    pub estimated_probability: u8,
    pub confidence: u8,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_evidence: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BeliefState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_hypothesis: Option<FailureHypothesisKind>,
    pub uncertainty_score: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hypotheses: Vec<FailureHypothesisBelief>,
}

#[derive(Debug, Clone)]
struct BeliefDraft {
    estimated_probability: u8,
    confidence: u8,
    rationale: String,
    evidence_ids: Vec<String>,
    missing_evidence: Vec<String>,
}

pub(crate) fn build_belief_state(
    entropy: &EntropyVector,
    verification: &VerificationSummary,
    requirements: &[RequirementNode],
    completion: &CompletionCertificate,
) -> BeliefState {
    let mut drafts = BTreeMap::<FailureHypothesisKind, BeliefDraft>::new();

    for incident in &completion.unresolved_incidents {
        match incident.kind.as_str() {
            "requirement_closure" => push_belief(
                &mut drafts,
                FailureHypothesisKind::RequirementClosureGap,
                80,
                completion.confidence,
                incident.summary.clone(),
                incident.evidence_ids.clone(),
                incident.missing_evidence.clone(),
            ),
            "verification" => push_belief(
                &mut drafts,
                FailureHypothesisKind::StaleVerification,
                82,
                completion.confidence,
                incident.summary.clone(),
                incident.evidence_ids.clone(),
                incident.missing_evidence.clone(),
            ),
            "subagent_lifecycle" => push_belief(
                &mut drafts,
                FailureHypothesisKind::SubagentLifecycleGap,
                82,
                completion.confidence,
                incident.summary.clone(),
                incident.evidence_ids.clone(),
                incident.missing_evidence.clone(),
            ),
            "test_oracle_authority" => push_belief(
                &mut drafts,
                FailureHypothesisKind::WeakTestOracle,
                86,
                completion.confidence,
                incident.summary.clone(),
                incident.evidence_ids.clone(),
                incident.missing_evidence.clone(),
            ),
            _ => {}
        }
    }

    if !completion.unresolved_requirement_ids.is_empty() {
        let evidence_ids = requirements
            .iter()
            .filter(|requirement| {
                completion
                    .unresolved_requirement_ids
                    .contains(&requirement.requirement_id)
            })
            .flat_map(|requirement| requirement.evidence_ids.iter().cloned())
            .collect::<Vec<_>>();
        push_belief(
            &mut drafts,
            FailureHypothesisKind::RequirementClosureGap,
            78,
            completion.confidence,
            format!(
                "{} scoped requirement(s) lack closure evidence",
                completion.unresolved_requirement_ids.len()
            ),
            evidence_ids,
            vec!["fresh requirement-linked verification or trace evidence".into()],
        );
    }

    for change in completion
        .test_oracle_changes
        .iter()
        .filter(|change| !change.authorized)
    {
        push_belief(
            &mut drafts,
            FailureHypothesisKind::WeakTestOracle,
            86,
            completion.confidence,
            format!("test oracle change in `{}` lacks authority", change.file),
            change.evidence_id.iter().cloned().collect(),
            vec!["spec authority and independent behavior evidence".into()],
        );
    }

    match verification.status {
        VerificationStatus::Stale => push_belief(
            &mut drafts,
            FailureHypothesisKind::StaleVerification,
            80,
            75,
            "latest verifier evidence is stale for the current repo state".into(),
            entropy_evidence(entropy, EntropyKind::Verification),
            vec!["fresh passing verifier evidence for changed paths".into()],
        ),
        VerificationStatus::Failed => push_belief(
            &mut drafts,
            FailureHypothesisKind::ProcessConformanceGap,
            72,
            70,
            "latest verifier failed and needs diagnosis before closure".into(),
            entropy_evidence(entropy, EntropyKind::Verification),
            vec!["failure isolation or passing targeted verifier".into()],
        ),
        VerificationStatus::NotRun => push_belief(
            &mut drafts,
            FailureHypothesisKind::StaleVerification,
            60,
            60,
            "no verifier has established closure for this case file".into(),
            entropy_evidence(entropy, EntropyKind::Verification),
            vec!["fresh verifier evidence".into()],
        ),
        VerificationStatus::Passed => {}
    }

    for score in &entropy.scores {
        if score.score < 55 {
            continue;
        }
        let Some(kind) = hypothesis_for_entropy(score) else {
            continue;
        };
        push_belief(
            &mut drafts,
            kind,
            entropy_probability(score),
            score.confidence,
            entropy_rationale(score),
            score.evidence_ids.clone(),
            score.missing_evidence.clone(),
        );
    }

    let mut hypotheses = drafts
        .into_iter()
        .map(|(kind, draft)| FailureHypothesisBelief {
            kind,
            estimated_probability: draft.estimated_probability,
            confidence: draft.confidence,
            rationale: draft.rationale,
            evidence_ids: draft.evidence_ids,
            missing_evidence: draft.missing_evidence,
        })
        .collect::<Vec<_>>();
    hypotheses.sort_by(|left, right| {
        right
            .estimated_probability
            .cmp(&left.estimated_probability)
            .then_with(|| right.confidence.cmp(&left.confidence))
            .then_with(|| left.kind.cmp(&right.kind))
    });

    let top_hypothesis = hypotheses.first().map(|belief| belief.kind);
    let uncertainty_score = hypotheses
        .iter()
        .map(|belief| belief.estimated_probability)
        .max()
        .unwrap_or(0);

    BeliefState {
        top_hypothesis,
        uncertainty_score,
        hypotheses,
    }
}

fn push_belief(
    drafts: &mut BTreeMap<FailureHypothesisKind, BeliefDraft>,
    kind: FailureHypothesisKind,
    estimated_probability: u8,
    confidence: u8,
    rationale: String,
    evidence_ids: Vec<String>,
    missing_evidence: Vec<String>,
) {
    let entry = drafts.entry(kind).or_insert_with(|| BeliefDraft {
        estimated_probability,
        confidence,
        rationale: rationale.clone(),
        evidence_ids: Vec::new(),
        missing_evidence: Vec::new(),
    });
    if estimated_probability > entry.estimated_probability {
        entry.estimated_probability = estimated_probability;
        entry.confidence = confidence;
        entry.rationale = rationale;
    } else if confidence > entry.confidence {
        entry.confidence = confidence;
    }
    append_unique(&mut entry.evidence_ids, evidence_ids);
    append_unique(&mut entry.missing_evidence, missing_evidence);
}

fn append_unique(target: &mut Vec<String>, values: Vec<String>) {
    for value in values {
        if !value.is_empty() && !target.contains(&value) {
            target.push(value);
        }
    }
}

fn entropy_probability(score: &EntropyScore) -> u8 {
    let score_part = score.score as u16 * 2;
    let confidence_part = score.confidence as u16;
    ((score_part + confidence_part) / 3).min(100) as u8
}

fn entropy_evidence(entropy: &EntropyVector, kind: EntropyKind) -> Vec<String> {
    entropy
        .score(kind)
        .map(|score| score.evidence_ids.clone())
        .unwrap_or_default()
}

fn entropy_rationale(score: &EntropyScore) -> String {
    score
        .top_causes
        .first()
        .cloned()
        .unwrap_or_else(|| format!("{:?} entropy is elevated", score.kind))
}

fn hypothesis_for_entropy(score: &EntropyScore) -> Option<FailureHypothesisKind> {
    match score.kind {
        EntropyKind::Goal => Some(FailureHypothesisKind::RequirementScopeGap),
        EntropyKind::Context => Some(FailureHypothesisKind::ContextLoss),
        EntropyKind::RepoBlame => Some(FailureHypothesisKind::RepoAttributionGap),
        EntropyKind::Verification => {
            if score_mentions(score, &["test oracle", "authority"]) {
                Some(FailureHypothesisKind::WeakTestOracle)
            } else {
                Some(FailureHypothesisKind::StaleVerification)
            }
        }
        EntropyKind::Plan => Some(FailureHypothesisKind::ProcessConformanceGap),
        EntropyKind::AgentHealth => {
            if score_mentions(
                score,
                &[
                    "provider",
                    "transport",
                    "rate",
                    "auth",
                    "service",
                    "timeout",
                    "connection",
                    "permission",
                ],
            ) {
                Some(FailureHypothesisKind::OperationalInstability)
            } else {
                Some(FailureHypothesisKind::AgentLoop)
            }
        }
        EntropyKind::UserDecision => Some(FailureHypothesisKind::UserAuthorityGap),
    }
}

fn score_mentions(score: &EntropyScore, needles: &[&str]) -> bool {
    let text = score
        .top_causes
        .iter()
        .chain(score.missing_evidence.iter())
        .chain(score.recommended_observations.iter())
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    needles.iter().all(|needle| text.contains(needle))
}
