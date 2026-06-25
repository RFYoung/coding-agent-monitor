//! Tests for console-output decoding, advisor id fragments, and HTTPS endpoint
//! handling, included into `lib.rs` via `#[path]` so they can reach the crate
//! root's private helpers.

use super::{AdvisorClientError, current_id_fragment, decode_console_line, post_json_http};
use std::collections::HashSet;

#[test]
fn valid_utf8_is_passed_through_unchanged() {
    assert_eq!(
        decode_console_line("版本 build ok".as_bytes()),
        "版本 build ok"
    );
}

#[test]
fn empty_input_decodes_to_empty_string() {
    assert_eq!(decode_console_line(&[]), "");
}

#[test]
fn invalid_utf8_does_not_panic_and_yields_text() {
    // 0x80 is not valid standalone UTF-8; decoding must fall back gracefully
    // (OEM code page on Windows, lossy elsewhere) without crashing.
    let decoded = decode_console_line(&[b'o', b'k', 0x80]);
    assert!(decoded.starts_with("ok"));
}

#[test]
fn https_advisor_endpoint_is_not_rejected_before_transport() {
    let error = post_json_http(
        "https://127.0.0.1:9/v1/chat/completions",
        "test-key",
        "{}",
        1,
    )
    .expect_err("unreachable local endpoint should fail");

    assert!(!matches!(error, AdvisorClientError::InvalidEndpoint(_)));
    assert!(!error.to_string().contains("https transport"));
}

#[test]
fn generated_id_fragments_are_unique_inside_one_millisecond_window() {
    let ids = (0..100)
        .map(|_| current_id_fragment())
        .collect::<HashSet<_>>();

    assert_eq!(ids.len(), 100);
}

#[test]
fn generated_id_fragments_include_process_id() {
    let id = current_id_fragment();

    assert!(id.contains(&std::process::id().to_string()));
}
