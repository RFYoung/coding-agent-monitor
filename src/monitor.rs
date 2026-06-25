//! The `Monitor` facade tying config, store, and the streaming control loop together.

use crate::*;

#[derive(Debug)]
pub struct Monitor {
    config: Config,
    service_failures: HashMap<String, usize>,
    robustness_scores: HashMap<String, i32>,
    design: Vec<DesignEntry>,
    trace: Vec<TraceEntry>,
}

impl Monitor {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            service_failures: HashMap::new(),
            robustness_scores: HashMap::new(),
            design: Vec::new(),
            trace: Vec::new(),
        }
    }

    pub fn ingest(&mut self, event: Event) -> Vec<Intervention> {
        match event.kind {
            EventKind::DesignThought => {
                if let Some(content) = event.content.clone().filter(|content| !content.is_empty()) {
                    self.design.push(DesignEntry {
                        time: event.time.clone(),
                        agent: event.agent.clone(),
                        session: event.session.clone(),
                        content,
                    });
                }
            }
            EventKind::FileChange | EventKind::RepoDiff => {
                if let Some(file) = event.file.clone().filter(|file| !file.is_empty()) {
                    self.trace.push(TraceEntry {
                        time: event.time.clone(),
                        event_id: event.event_id.clone(),
                        agent: event.agent.clone(),
                        provider: event.provider.clone(),
                        model: event.model.clone(),
                        session: event.session.clone(),
                        file,
                        line: event.line,
                        line_end: event.line_end,
                        rationale: event.rationale.clone(),
                        related_event_ids: event.related_event_ids.clone(),
                        requirement_ids: event.requirement_ids.clone(),
                    });
                }
            }
            EventKind::ModelMessage
            | EventKind::CommandOutput
            | EventKind::CommandResult
            | EventKind::ToolCall
            | EventKind::ToolResult
            | EventKind::TestResult
            | EventKind::UserInstruction
            | EventKind::HandoffSummary
            | EventKind::AgentHealth
            | EventKind::VerificationClaim
            | EventKind::InterventionResult => {}
        }

        let mut interventions = Vec::new();
        let content = event.content.as_deref().unwrap_or_default();
        if self.config.open_work && looks_like_premature_stop(content) {
            self.adjust_robustness(&event.agent, -2);
            interventions.push(Intervention {
                kind: InterventionKind::PrematureStop,
                action: Action::ContinueWorking,
                agent: Some(event.agent.clone()),
                reason: "remaining work is open; continue obvious next steps instead of asking the user to decide"
                    .into(),
            });
        }

        if looks_like_service_failure(content) {
            self.adjust_robustness(&event.agent, -1);
            let failures = self
                .service_failures
                .entry(event.agent.clone())
                .and_modify(|count| *count += 1)
                .or_insert(1);

            if *failures <= self.config.retry_limit {
                interventions.push(Intervention {
                    kind: InterventionKind::ServiceFailure,
                    action: Action::RetrySameAgent,
                    agent: Some(event.agent.clone()),
                    reason: "transient service failure; retry the same agent before switching"
                        .into(),
                });
            } else if let Some(fallback_agent) = self.next_fallback(&event.agent) {
                interventions.push(Intervention {
                    kind: InterventionKind::ServiceFailure,
                    action: Action::SwitchAgent,
                    agent: Some(fallback_agent),
                    reason: "retry limit exceeded; switch to a fallback agent".into(),
                });
            } else {
                interventions.push(Intervention {
                    kind: InterventionKind::ServiceFailure,
                    action: Action::RetrySameAgent,
                    agent: Some(event.agent.clone()),
                    reason: "retry limit exceeded but no fallback agent is available; keep the same agent under monitor supervision".into(),
                });
            }
        } else if event_can_clear_service_failure(&event, content) {
            self.service_failures.remove(&event.agent);
        }

        if looks_like_forgetting_design_memory(content) {
            self.adjust_robustness(&event.agent, -3);
            interventions.push(Intervention {
                kind: InterventionKind::AgentDegraded,
                action: Action::SpawnFreshAgent,
                agent: Some(event.agent.clone()),
                reason: "agent appears to have lost design memory; spawn a fresh agent with durable project context"
                    .into(),
            });
        }

        interventions
    }

    pub fn design_record(&self) -> &[DesignEntry] {
        &self.design
    }

    pub fn trace(&self) -> &[TraceEntry] {
        &self.trace
    }

    pub fn robustness_score(&self, agent: &str) -> i32 {
        self.robustness_scores
            .get(agent)
            .copied()
            .unwrap_or_default()
    }

    fn adjust_robustness(&mut self, agent: &str, delta: i32) {
        self.robustness_scores
            .entry(agent.into())
            .and_modify(|score| *score += delta)
            .or_insert(delta);
    }

    fn next_fallback(&self, current: &str) -> Option<String> {
        self.config
            .fallback_agents
            .iter()
            .find(|agent| agent.as_str() != current)
            .cloned()
    }
}
