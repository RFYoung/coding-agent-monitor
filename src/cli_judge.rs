//! External judge invocation for the `agent-monitor judge` command.
//!
//! Maps a read-only judge review into interventions and pipes a bounded,
//! evidence-first prompt plus the review report to an external judge process.

use coding_agent_monitor::{
    Action, AgentReviewAction, AgentReviewReport, Intervention, InterventionKind,
    prepare_wrapped_launch,
};
use std::io::Write;
use std::process::{Command, Stdio};

pub(crate) const EXTERNAL_JUDGE_PROMPT: &str = "You are a read-only external judge for a coding-agent supervisor.\n\
Output exactly one line: decision=<continue | force_verification | handoff | restart>; evidence=<ids/files/tests>; risk=<short reason>.\n\
Judge the control loop, not code style. Prioritize unverified completion, stale verification, lost durable intent, unsafe edits, and telemetry gaps.\n\
Treat intended-environment validation as first-class: web may use browser/Playwright, but mobile/native/system/ML need platform runtime evidence.\n\
Prefer force_verification over handoff when verification is stale or missing.\n\
Do not propose broad refactors, new product scope, or implementation patches.";

pub(crate) fn interventions_from_review(report: &AgentReviewReport) -> Vec<Intervention> {
    report
        .findings
        .iter()
        .filter_map(|finding| {
            let (kind, action) = match finding.recommended_action {
                AgentReviewAction::Continue => return None,
                AgentReviewAction::ContinueWorking | AgentReviewAction::InstallTelemetry => {
                    (InterventionKind::PrematureStop, Action::ContinueWorking)
                }
                AgentReviewAction::ForceVerification => {
                    (InterventionKind::PrematureStop, Action::ContinueWorking)
                }
                AgentReviewAction::SpawnJudgeAgent => {
                    (InterventionKind::SuspiciousChange, Action::SpawnJudgeAgent)
                }
                AgentReviewAction::SpawnFreshAgent => {
                    (InterventionKind::AgentDegraded, Action::SpawnFreshAgent)
                }
            };
            Some(Intervention {
                kind,
                action,
                agent: finding.agent.clone(),
                reason: format!("judge {}: {}", finding.category, finding.evidence),
            })
        })
        .collect()
}

pub(crate) fn run_external_judge(
    command: &[String],
    report: &AgentReviewReport,
) -> Result<(), String> {
    // External judges are subprocesses, not privileged monitor components. They
    // receive a bounded read-only report over stdin and inherit stdout/stderr.
    let launch = prepare_wrapped_launch(command).map_err(|error| error.to_string())?;
    let mut child = Command::new(&launch.program)
        .args(&launch.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("spawn external judge {}: {error}", launch.program))?;

    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "{EXTERNAL_JUDGE_PROMPT}\n")
            .map_err(|error| format!("write external judge prompt: {error}"))?;
        serde_json::to_writer_pretty(&mut stdin, report)
            .map_err(|error| format!("encode external judge packet: {error}"))?;
        writeln!(stdin).map_err(|error| format!("finish external judge prompt: {error}"))?;
    }

    let status = child
        .wait()
        .map_err(|error| format!("wait external judge: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("external judge exited with {status}"))
    }
}

#[cfg(test)]
#[path = "cli_judge_tests.rs"]
mod tests;
