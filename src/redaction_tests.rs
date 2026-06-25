//! Tests for evidence-summary redaction, included into `redaction.rs` via
//! `#[path]` so they can reach the module's private helpers.

use super::*;

#[test]
fn clean_sanitization_preserves_structured_whitespace() {
    let input =
        "Build the monitor supervisor.\nAcceptance criteria:\n- parser handles nested calls";

    let (sanitized, status) = sanitize_evidence_summary(input);

    assert_eq!(status, RedactionStatus::Clean);
    assert_eq!(sanitized, input);
}
