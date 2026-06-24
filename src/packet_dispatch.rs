use crate::{
    AdviceRun, ControlAction, ControlPacket, DispatchResult, DispatchStatus, PacketUrgency,
    ProjectStore, StoreError, current_id_fragment, normalize_agent_label, read_all_jsonl,
};

pub(crate) struct DuplicatePacketDispatch {
    pub(crate) packet: ControlPacket,
    pub(crate) dispatch: DispatchResult,
}

pub(crate) fn duplicate_urgent_packet_dispatch(
    store: &ProjectStore,
    action: &ControlAction,
    packet: &ControlPacket,
) -> Result<Option<DuplicatePacketDispatch>, StoreError> {
    if !action_is_duplicate_suppression_candidate(action, packet) {
        return Ok(None);
    }

    let candidate_evidence = normalized_evidence_refs(&packet.evidence_refs);
    let candidate_target = normalize_agent_label(&packet.target_agent);
    let action_kind = action.kind();
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root().join("advice.jsonl"))?;

    for advice in advice_records.into_iter().rev() {
        if advice.dispatch_result.status == DispatchStatus::Failed {
            continue;
        }
        if !action_is_duplicate_suppression_candidate(&advice.final_action, &advice.packet) {
            continue;
        }
        if advice.final_action.kind() != action_kind {
            continue;
        }
        if normalize_agent_label(&advice.packet.target_agent) != candidate_target {
            continue;
        }
        if advice.packet.preconditions != packet.preconditions {
            continue;
        }
        if normalized_evidence_refs(&advice.packet.evidence_refs) != candidate_evidence {
            continue;
        }

        let dispatch = DispatchResult {
            dispatch_id: format!("dispatch-{}", current_id_fragment()),
            packet_id: advice.packet.packet_id.clone(),
            target_agent: advice.packet.target_agent.clone(),
            status: DispatchStatus::SuppressedDuplicate,
            path: advice.dispatch_result.path.clone(),
            reason: Some(format!(
                "duplicate urgent packet suppressed; prior packet {} already covers the same action, target, evidence, and preconditions",
                advice.packet.packet_id
            )),
        };
        return Ok(Some(DuplicatePacketDispatch {
            packet: advice.packet,
            dispatch,
        }));
    }

    Ok(None)
}

fn action_is_duplicate_suppression_candidate(
    action: &ControlAction,
    packet: &ControlPacket,
) -> bool {
    matches!(
        action,
        ControlAction::ForceVerification { .. }
            | ControlAction::BlockProgressUntilTraceAndVerification { .. }
            | ControlAction::RetryAgent { .. }
    ) && matches!(
        packet.urgency,
        PacketUrgency::Urgent | PacketUrgency::Verification
    )
}

fn normalized_evidence_refs(evidence_refs: &[String]) -> Vec<String> {
    let mut refs = evidence_refs
        .iter()
        .map(|evidence| evidence.trim().to_string())
        .filter(|evidence| !evidence.is_empty())
        .collect::<Vec<_>>();
    refs.sort();
    refs.dedup();
    refs
}
