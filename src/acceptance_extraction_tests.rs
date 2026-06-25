//! Tests for acceptance-criteria extraction, included into `lib.rs` via
//! `#[path]` so they can reach the crate root's private helpers.

use super::*;

#[test]
fn extracts_acceptance_block_after_goal_sentence() {
    assert_eq!(
        extract_acceptance_criteria(
            "Build the monitor supervisor.\nAcceptance criteria:\n- parser handles nested calls\n- advisor packets cite evidence",
        ),
        vec![
            "parser handles nested calls".to_string(),
            "advisor packets cite evidence".to_string()
        ]
    );
}
