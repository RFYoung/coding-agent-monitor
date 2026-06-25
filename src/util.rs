//! Cross-cutting helpers: id generation, git introspection, and path/acceptance classification.

use crate::*;

pub(crate) static NEXT_ID_SEQUENCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn current_id_fragment() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let sequence = NEXT_ID_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{millis}-{}-{sequence}", std::process::id())
}

pub(crate) fn current_git_head(workspace: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8(output.stdout).ok()?;
    let head = head.trim();
    if head.is_empty() {
        None
    } else {
        Some(head.to_string())
    }
}

pub(crate) fn current_git_branch(workspace: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?;
    let branch = branch.trim();
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

pub(crate) fn current_git_dirty(workspace: &Path) -> Option<bool> {
    let output = Command::new("git")
        .current_dir(workspace)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!output.stdout.is_empty())
}

pub(crate) fn is_source_or_test_file(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".txt") {
        return false;
    }
    lower.starts_with("src/")
        || lower.starts_with("tests/")
        || lower.contains("/src/")
        || lower.contains("/tests/")
        || [
            ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java", ".cs", ".cpp", ".c", ".h",
            ".hpp", ".toml", ".json", ".yaml", ".yml",
        ]
        .iter()
        .any(|extension| lower.ends_with(extension))
}

pub(crate) fn test_oracle_change_lacks_authority(event: &Event, file: &str) -> bool {
    is_test_oracle_file(file)
        && test_oracle_change_is_authority_sensitive(event, file)
        && !test_oracle_change_has_authority(event)
}

pub(crate) fn is_test_oracle_file(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    lower.starts_with("tests/")
        || lower.starts_with("test/")
        || lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.contains("__snapshots__")
        || lower.contains("/fixtures/")
        || lower.contains("/testdata/")
        || file_name.ends_with(".snap")
        || file_name.ends_with(".snapshot")
        || file_name.contains(".spec.")
        || file_name.contains(".test.")
}

pub(crate) fn test_oracle_change_is_authority_sensitive(event: &Event, file: &str) -> bool {
    let text = test_oracle_change_text(event, file);
    is_snapshot_or_fixture_path(file)
        || contains_failure_signal(
            &text,
            &[
                "expected value",
                "expected output",
                "expectation",
                "assertion",
                "assert ",
                "snapshot",
                "fixture",
                "golden",
                "baseline",
                "skip",
                "ignored test",
                "ignore test",
                "delete test",
                "remove test",
                "update expected",
                "refresh snapshot",
                "match implementation",
                "match current output",
                "weaken assertion",
            ],
        )
}

pub(crate) fn is_snapshot_or_fixture_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    lower.contains("__snapshots__")
        || lower.contains("/fixtures/")
        || lower.contains("/testdata/")
        || file_name.ends_with(".snap")
        || file_name.ends_with(".snapshot")
}

pub(crate) fn test_oracle_change_has_authority(event: &Event) -> bool {
    let text = test_oracle_change_text(event, "");
    contains_failure_signal(
        &text,
        &[
            "user-authorized",
            "user authorized",
            "user requested",
            "accepted requirement",
            "authorized requirement",
            "acceptance",
            "requirement",
            "spec authority",
            "product requirement",
            "old oracle invalid",
            "old behavior invalid",
            "changed requirement",
        ],
    )
}

pub(crate) fn test_oracle_change_text(event: &Event, file: &str) -> String {
    let mut text = String::new();
    text.push_str(file);
    text.push('\n');
    if let Some(rationale) = event.rationale.as_deref() {
        text.push_str(rationale);
        text.push('\n');
    }
    if let Some(content) = event.content.as_deref() {
        text.push_str(content);
        text.push('\n');
    }
    if let Some(command) = event.command.as_deref() {
        text.push_str(command);
    }
    text.to_ascii_lowercase()
}

pub(crate) fn is_verification_relevant_file(path: &str, policy: &PolicyConfig) -> bool {
    if !policy.require_verification_after_source_change {
        return false;
    }
    if policy.allow_docs_only_continue_without_tests && is_documentation_file(path) {
        return false;
    }
    is_source_or_test_file(path)
        || (!policy.allow_docs_only_continue_without_tests && is_documentation_file(path))
}

pub(crate) fn is_documentation_file(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    lower.starts_with("docs/")
        || lower.starts_with("doc/")
        || lower.starts_with("documentation/")
        || lower.contains("/docs/")
        || lower.contains("/doc/")
        || lower.contains("/documentation/")
        || file_name.starts_with("readme")
        || file_name.starts_with("changelog")
        || file_name.starts_with("license")
        || [".md", ".mdx", ".txt", ".rst", ".adoc"]
            .iter()
            .any(|extension| lower.ends_with(extension))
}

pub(crate) fn security_path_user_decision_cause(
    path: &str,
    security: &SecurityConfig,
) -> Option<String> {
    if security.redact_env && is_env_path(path) {
        return Some(format!(
            "security deny path `{}` requires explicit user authorization",
            normalize_path_for_match(path)
        ));
    }

    if security.redact_auth_files && is_auth_file_path(path) {
        return Some(format!(
            "security deny path `{}` requires explicit user authorization",
            normalize_path_for_match(path)
        ));
    }

    if security
        .deny_paths
        .iter()
        .any(|pattern| path_matches_security_pattern(path, pattern))
    {
        return Some(format!(
            "security deny path `{}` requires explicit user authorization",
            normalize_path_for_match(path)
        ));
    }

    if security
        .protected_paths
        .iter()
        .any(|pattern| path_matches_security_pattern(path, pattern))
    {
        return Some(format!(
            "security protected path `{}` requires explicit user authorization",
            normalize_path_for_match(path)
        ));
    }

    None
}

pub(crate) fn is_env_path(path: &str) -> bool {
    let normalized = normalize_path_for_match(path);
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    file_name == ".env" || file_name.starts_with(".env.")
}

pub(crate) fn is_auth_file_path(path: &str) -> bool {
    let normalized = normalize_path_for_match(path);
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    file_name == "auth.json" || file_name == "id_rsa" || file_name.ends_with(".pem")
}

pub(crate) fn path_matches_security_pattern(path: &str, pattern: &str) -> bool {
    let path = normalize_path_for_match(path);
    let pattern = normalize_path_for_match(pattern);
    if path == pattern {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }

    if let Some(suffix) = pattern.strip_prefix("**/") {
        if suffix.contains('*') {
            return path
                .rsplit('/')
                .next()
                .is_some_and(|file_name| wildcard_match(suffix, file_name))
                || wildcard_match(suffix, &path);
        }
        return path == suffix || path.ends_with(&format!("/{suffix}"));
    }

    if pattern.contains('*') {
        return wildcard_match(&pattern, &path);
    }

    false
}

pub(crate) fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star_index = None;
    let mut value_after_star = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == value[value_index] {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            value_after_star = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            value_after_star += 1;
            value_index = value_after_star;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }

    pattern_index == pattern.len()
}

pub(crate) fn verifier_matches_path(verifier: &VerifierConfig, file: &str) -> bool {
    let file = normalize_path_for_match(file);
    verifier.paths.iter().any(|pattern| {
        let pattern = normalize_path_for_match(pattern);
        file == pattern
            || file.starts_with(pattern.trim_end_matches('/'))
            || pattern.ends_with("/**")
                && file.starts_with(pattern.trim_end_matches("/**").trim_end_matches('/'))
    })
}

pub(crate) fn verifier_matches_acceptance(verifier: &VerifierConfig, criterion: &str) -> bool {
    if verifier
        .acceptance_patterns
        .iter()
        .any(|pattern| acceptance_pattern_matches(pattern, criterion))
    {
        return true;
    }

    let criterion_tokens = meaningful_acceptance_tokens(criterion);
    if criterion_tokens.is_empty() {
        return false;
    }
    let verifier_text = std::iter::once(verifier.id.as_str())
        .chain(std::iter::once(verifier.command.as_str()))
        .chain(verifier.paths.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let verifier_tokens = text_tokens(&verifier_text);
    let matched = criterion_tokens
        .iter()
        .filter(|token| verifier_tokens.contains(token) || verifier_text.contains(token.as_str()))
        .count();
    matched >= 2 || (matched == 1 && criterion_tokens.len() == 1)
}

pub(crate) fn acceptance_pattern_matches(pattern: &str, criterion: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    let pattern_text = pattern.to_lowercase();
    let criterion_text = criterion.to_lowercase();
    if criterion_text.contains(&pattern_text) || pattern_text.contains(&criterion_text) {
        return true;
    }

    let criterion_tokens = text_tokens(&criterion_text);
    let pattern_tokens = meaningful_acceptance_tokens(pattern);
    !pattern_tokens.is_empty()
        && pattern_tokens.iter().all(|token| {
            criterion_tokens.contains(token) || criterion_text.contains(token.as_str())
        })
}

pub(crate) fn meaningful_acceptance_tokens(text: &str) -> Vec<String> {
    text_tokens(text)
        .into_iter()
        .filter(|token| token.len() >= 3)
        .filter(|token| !acceptance_stop_word(token))
        .collect()
}

pub(crate) fn text_tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn acceptance_stop_word(token: &str) -> bool {
    matches!(
        token,
        "acceptance"
            | "criterion"
            | "criteria"
            | "should"
            | "must"
            | "pass"
            | "passes"
            | "passing"
            | "verify"
            | "verified"
            | "test"
            | "tests"
            | "behavior"
            | "feature"
            | "work"
            | "works"
            | "with"
            | "without"
            | "and"
            | "the"
            | "for"
            | "from"
            | "that"
            | "this"
    )
}

pub(crate) fn normalize_path_for_match(path: &str) -> String {
    normalize_path_text(path)
        .trim_start_matches("./")
        .to_string()
}

pub(crate) fn normalize_blame_path(workspace: &Path, path: &str) -> String {
    let workspace = normalize_path_text(&workspace.display().to_string());
    let normalized = normalize_path_text(path);
    if normalized == workspace {
        return String::new();
    }
    let workspace_prefix = format!("{workspace}/");
    normalized
        .strip_prefix(&workspace_prefix)
        .unwrap_or(&normalized)
        .to_string()
}

pub(crate) fn blame_match_workspace(workspace: &Path) -> PathBuf {
    if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(workspace))
            .unwrap_or_else(|_| workspace.to_path_buf())
    }
}

pub(crate) fn normalize_path_text(path: &str) -> String {
    let path = path.replace('\\', "/").to_lowercase();
    let rooted = path.starts_with('/');
    let mut components = Vec::<&str>::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.last().is_some_and(|last| *last != "..") {
                    components.pop();
                } else if !rooted {
                    components.push(component);
                }
            }
            _ => components.push(component),
        }
    }
    let normalized = components.join("/");
    if rooted {
        format!("/{normalized}")
    } else {
        normalized
    }
}

pub(crate) fn is_verification_command(command: &str) -> bool {
    let lower = command.to_lowercase();
    [
        "test",
        "cargo check",
        "cargo build",
        "gradle build",
        "gradlew build",
        "xcodebuild build",
        "flutter build",
        "swift build",
        "npm run build",
        "pnpm build",
        "yarn build",
        "pytest",
        "vitest",
        "jest",
        "tsc",
    ]
    .iter()
    .any(|signal| lower.contains(signal))
}

pub(crate) fn fnv1a64_digest(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}
