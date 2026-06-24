use crate::{Event, TraceEntry};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RedactionStatus {
    #[default]
    Clean,
    Redacted,
    Tainted,
}

pub(crate) fn event_redaction_status(event: &Event) -> RedactionStatus {
    match event
        .redaction_status
        .as_deref()
        .map(str::to_ascii_lowercase)
    {
        Some(status) if status == "tainted" => RedactionStatus::Tainted,
        Some(status) if status == "redacted" => RedactionStatus::Redacted,
        Some(status) if status == "clean" => RedactionStatus::Clean,
        Some(_) => RedactionStatus::Tainted,
        None => RedactionStatus::Clean,
    }
}

pub(crate) fn strongest_redaction_status(
    left: RedactionStatus,
    right: RedactionStatus,
) -> RedactionStatus {
    match (left, right) {
        (RedactionStatus::Tainted, _) | (_, RedactionStatus::Tainted) => RedactionStatus::Tainted,
        (RedactionStatus::Redacted, _) | (_, RedactionStatus::Redacted) => {
            RedactionStatus::Redacted
        }
        (RedactionStatus::Clean, RedactionStatus::Clean) => RedactionStatus::Clean,
    }
}

pub(crate) fn packet_text_is_tainted(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_secret_assignment(&lower, "api_key")
        || contains_secret_assignment(&lower, "password")
        || contains_secret_assignment(&lower, "secret")
        || contains_secret_assignment(&lower, "token")
        || contains_secret_assignment(&lower, "access_token")
        || contains_secret_assignment(&lower, "client_secret")
        || contains_secret_assignment(&lower, "aws_secret_access_key")
        || lower.contains("authorization: bearer")
        || lower.contains("x-api-key:")
        || lower.contains("-----begin ")
        || has_secret_token_prefix(text)
        || looks_like_jwt(text)
        || redact_sk_tokens(text) != text
}

pub(crate) fn storage_redacted_event(event: &Event) -> Event {
    let mut event = event.clone();
    let mut redacted = false;
    redacted |= redact_optional_storage_text(&mut event.content);
    redacted |= redact_optional_storage_text(&mut event.command);
    redacted |= redact_optional_storage_text(&mut event.rationale);
    redacted |= redact_optional_storage_text(&mut event.source_path);
    if redacted {
        if event.redaction_status.as_deref() != Some("tainted") {
            event.redaction_status = Some("redacted".into());
        }
        if !event
            .redaction_rules
            .iter()
            .any(|rule| rule == "storage_secret")
        {
            event.redaction_rules.push("storage_secret".into());
        }
    }
    event
}

pub(crate) fn storage_redacted_trace(trace: &TraceEntry) -> TraceEntry {
    let mut trace = trace.clone();
    redact_optional_storage_text(&mut trace.rationale);
    trace
}

fn redact_optional_storage_text(value: &mut Option<String>) -> bool {
    let Some(text) = value else {
        return false;
    };
    let (redacted, status) = sanitize_evidence_summary(text);
    if status == RedactionStatus::Clean {
        return false;
    }
    *text = redacted;
    true
}

fn contains_secret_assignment(lower_text: &str, key: &str) -> bool {
    let mut start = 0;
    while let Some(relative) = lower_text[start..].find(key) {
        let key_start = start + relative;
        let key_end = key_start + key.len();
        let before_ok =
            key_start == 0 || !lower_text.as_bytes()[key_start - 1].is_ascii_alphanumeric();
        let after = lower_text[key_end..].trim_start();
        if before_ok && after.starts_with('=') {
            return true;
        }
        start = key_end;
    }
    false
}

fn has_secret_token_prefix(text: &str) -> bool {
    text.split_whitespace().any(|token| {
        let lower = token.to_lowercase();
        (lower.starts_with("ghp_")
            || lower.starts_with("github_pat_")
            || lower.starts_with("xoxb-")
            || lower.starts_with("xoxp-")
            || lower.starts_with("akia"))
            && token.len() >= 8
    })
}

fn looks_like_jwt(text: &str) -> bool {
    text.split_whitespace().any(|token| {
        let mut parts = token.split('.');
        let Some(header) = parts.next() else {
            return false;
        };
        let Some(payload) = parts.next() else {
            return false;
        };
        let Some(signature) = parts.next() else {
            return false;
        };
        parts.next().is_none()
            && header.starts_with("eyJ")
            && payload.len() >= 8
            && signature.len() >= 8
    })
}

pub(crate) fn sanitize_evidence_summary(summary: &str) -> (String, RedactionStatus) {
    let mut redacted = summary.to_string();
    let original = redacted.clone();
    redacted = redact_after_case_insensitive_prefix(&redacted, "authorization: bearer ");
    redacted = redact_key_value_secret(&redacted, "api_key=");
    redacted = redact_key_value_secret(&redacted, "password=");
    redacted = redact_key_value_secret(&redacted, "secret=");
    redacted = redact_key_value_secret(&redacted, "token=");
    redacted = redact_key_value_secret(&redacted, "access_token=");
    redacted = redact_key_value_secret(&redacted, "client_secret=");
    redacted = redact_key_value_secret(&redacted, "aws_secret_access_key=");
    redacted = redact_after_case_insensitive_prefix(&redacted, "x-api-key: ");
    redacted = redact_github_tokens(&redacted);
    redacted = redact_slack_tokens(&redacted);
    redacted = redact_sk_tokens(&redacted);
    if redacted == original {
        (redacted, RedactionStatus::Clean)
    } else {
        (redacted, RedactionStatus::Redacted)
    }
}

fn redact_after_case_insensitive_prefix(input: &str, prefix: &str) -> String {
    let lower = input.to_lowercase();
    let mut output = input.to_string();
    if let Some(relative_start) = lower.find(prefix) {
        let start = relative_start + prefix.len();
        let end = input[start..]
            .find(char::is_whitespace)
            .map(|offset| start + offset)
            .unwrap_or(input.len());
        output.replace_range(start..end, "[REDACTED]");
    }
    output
}

fn redact_key_value_secret(input: &str, key: &str) -> String {
    let lower = input.to_lowercase();
    let Some(start) = lower.find(key) else {
        return input.to_string();
    };
    let value_start = start + key.len();
    let value_end = input[value_start..]
        .find(char::is_whitespace)
        .map(|offset| value_start + offset)
        .unwrap_or(input.len());
    let mut output = input.to_string();
    output.replace_range(value_start..value_end, "[REDACTED]");
    output
}

fn redact_sk_tokens(input: &str) -> String {
    let mut changed = false;
    let redacted = input
        .split_whitespace()
        .map(|token| {
            if token.starts_with("sk-") && token.len() >= 8 {
                changed = true;
                "[REDACTED]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if changed { redacted } else { input.to_string() }
}

fn redact_github_tokens(input: &str) -> String {
    let mut changed = false;
    let redacted = input
        .split_whitespace()
        .map(|token| {
            let lower = token.to_lowercase();
            if (lower.starts_with("ghp_") || lower.starts_with("github_pat_")) && token.len() >= 8 {
                changed = true;
                "[REDACTED]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if changed { redacted } else { input.to_string() }
}

fn redact_slack_tokens(input: &str) -> String {
    let mut changed = false;
    let redacted = input
        .split_whitespace()
        .map(|token| {
            let lower = token.to_lowercase();
            if (lower.starts_with("xoxb-") || lower.starts_with("xoxp-")) && token.len() >= 8 {
                changed = true;
                "[REDACTED]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if changed { redacted } else { input.to_string() }
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    #[test]
    fn clean_sanitization_preserves_structured_whitespace() {
        let input =
            "Build the monitor supervisor.\nAcceptance criteria:\n- parser handles nested calls";

        let (sanitized, status) = sanitize_evidence_summary(input);

        assert_eq!(status, RedactionStatus::Clean);
        assert_eq!(sanitized, input);
    }
}
