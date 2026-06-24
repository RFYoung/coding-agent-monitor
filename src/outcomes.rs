use crate::{
    AdviceRun, ControlActionKind, ControlCaseFile, ControlPacket, DispatchResult, EntropyKind,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntropyDelta {
    pub kind: EntropyKind,
    pub delta: i16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntropyDeltaEvidence {
    pub kind: EntropyKind,
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cause_evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub result_evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionOutcome {
    pub outcome_id: String,
    pub advice_id: String,
    pub action: ControlActionKind,
    pub status: OutcomeStatus,
    pub expected_entropy_delta: Vec<EntropyDelta>,
    pub observed_entropy_delta: Vec<EntropyDelta>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_entropy_delta_evidence: Vec<EntropyDeltaEvidence>,
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirement_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecisionTrail {
    pub case_file: ControlCaseFile,
    pub advice: AdviceRun,
    pub packet: ControlPacket,
    pub dispatch: DispatchResult,
    pub outcomes: Vec<ActionOutcome>,
}
