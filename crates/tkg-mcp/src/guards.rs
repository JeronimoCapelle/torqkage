//! Composable security guards, path confinement, and UTF-8 truncation
//! primitives.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use tkg_common::{GateCheck, ProjectSecurityConfig, matches_directory_scope_with_containers};

use crate::models::{AgentGuard, ToolDefinition};

/// Inspects a target path against dangerous files, secrets, and repository
/// internals, guided by optional per-project security configuration.
///
/// # Errors
/// Returns an error if the path attempts to access sensitive files,
/// credentials, or VCS internals (`.git/`, `.jj/`), unless explicitly exempted
/// in `security.allowed_patterns`.
pub(crate) fn check_dangerous_files_with_security(
    path: &Path,
    security: Option<&ProjectSecurityConfig>,
) -> Result<(), String> {
    tkg_common::check_dangerous_files_with_security(path, security)
}

/// Resolves and confines a relative path within the canonical workspace bounds.
///
/// # Errors
/// Returns an error if the path escapes workspace bounds, traverses `..`, or
/// violates directory allowlists.
pub(crate) fn resolve_safe_path(
    workspace_root: &Path,
    rel_path_str: &str,
    allowed_directories: &[String],
    enforce_bounds: bool,
) -> Result<PathBuf, String> {
    let clean_str = rel_path_str.trim();
    if clean_str.is_empty() {
        return Err("Path argument cannot be empty".to_owned());
    }
    if clean_str.contains('\0') {
        return Err("Security policy violation: Path containing NUL byte is forbidden".to_owned());
    }
    if clean_str.split(['/', '\\']).any(|segment| segment == "..") {
        return Err("Security policy violation: Path traversal via '..' is forbidden".to_owned());
    }

    let input_path = Path::new(clean_str);

    // Reject any raw parent component traversal attempts
    for component in input_path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(
                "Security policy violation: Path traversal via '..' is forbidden".to_owned(),
            );
        }
    }

    let candidate = if input_path.has_root() {
        input_path.to_path_buf()
    } else {
        workspace_root.join(input_path)
    };

    if enforce_bounds {
        let canonical_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());

        let resolved = if candidate.exists() {
            Some(candidate.canonicalize().map_err(|err| {
                format!(
                    "Failed to canonicalize path '{}': {err}",
                    candidate.display()
                )
            })?)
        } else {
            // Walk up to the nearest existing ancestor so non-existent nested
            // workspace paths resolve inside root while rooted or non-existent
            // escaping paths fail closed across Unix and Windows.
            candidate
                .ancestors()
                .find(|ancestor| ancestor.exists())
                .map(|ancestor| {
                    ancestor.canonicalize().map_err(|err| {
                        format!(
                            "Failed to canonicalize ancestor '{}': {err}",
                            ancestor.display()
                        )
                    })
                })
                .transpose()?
        };

        if !resolved
            .as_ref()
            .is_some_and(|path| path.starts_with(&canonical_root))
        {
            let escaped_display = resolved.as_ref().unwrap_or(&candidate);
            return Err(format!(
                "Security policy violation: Path '{}' escapes canonical workspace root '{}'",
                escaped_display.display(),
                canonical_root.display()
            ));
        }
    }

    // Check allowed directories whitelist
    if !allowed_directories.is_empty() {
        let relative = candidate.strip_prefix(workspace_root).unwrap_or(input_path);

        let rel_lossy = relative.to_string_lossy();
        let project = tkg_common::ProjectConfig::detect_or_load(workspace_root);
        let containers = project.resolved_containers(workspace_root);
        let allowed = allowed_directories
            .iter()
            .any(|dir| matches_directory_scope_with_containers(&rel_lossy, dir, &containers));

        if !allowed {
            return Err(format!(
                "Security policy violation: Path '{rel_lossy}' is outside allowed directories: {allowed_directories:?}"
            ));
        }
    }

    Ok(candidate)
}

/// Enforces single-directory confinement (`enforce_dir`) and single-file target
/// locking (`enforce_file_target`).
///
/// # Errors
/// Returns an error if the candidate path is outside `enforce_dir` or does not
/// match `enforce_file_target`.
pub(crate) fn check_enforce_boundaries(
    workspace_root: &Path,
    candidate: &Path,
    enforce_dir: Option<&str>,
    enforce_file_target: Option<&str>,
) -> Result<(), String> {
    let relative = match candidate.strip_prefix(workspace_root) {
        Ok(rel) => rel,
        Err(_) if candidate.is_absolute() => {
            return Err(format!(
                "Security policy violation: Path '{}' is outside workspace root '{}'.",
                candidate.display(),
                workspace_root.display()
            ));
        },
        Err(_) => candidate,
    };
    let rel_lossy = relative.to_string_lossy();

    // 1. Enforce single-file target locking
    if let Some(target) = enforce_file_target {
        let expected_rel = Path::new(target)
            .strip_prefix(workspace_root)
            .unwrap_or_else(|_| Path::new(target));
        let expected_lossy = expected_rel.to_string_lossy();

        if rel_lossy != expected_lossy {
            return Err(format!(
                "Security policy violation: Tool is strictly locked to target file '{expected_lossy}', but '{rel_lossy}' was requested."
            ));
        }
    }

    // 2. Enforce single-directory confinement
    if let Some(dir) = enforce_dir {
        let project = tkg_common::ProjectConfig::detect_or_load(workspace_root);
        let containers = project.resolved_containers(workspace_root);
        if !matches_directory_scope_with_containers(&rel_lossy, dir, &containers) {
            return Err(format!(
                "Security policy violation: Path '{rel_lossy}' is outside enforced directory '{dir}'"
            ));
        }
    }

    Ok(())
}

/// Enforces agent-specific directory and file path boundaries.
///
/// # Errors
/// Returns an error if the agent is restricted and the target path violates the
/// agent's guard.
pub(crate) fn check_agent_guard(
    workspace_root: &Path,
    candidate: &Path,
    agent_name: Option<&str>,
    guards: &BTreeMap<String, AgentGuard>,
) -> Result<(), String> {
    let guard = agent_name.and_then(|a| guards.get(a));
    tkg_common::check_agent_guard_direct(workspace_root, candidate, agent_name, guard)
}

/// Enforces declarative agent path policies (e.g. `comments_only`,
/// `tests_only`, `delimited`).
///
/// Supports language-agnostic syntax resolution via `project.toml` and
/// auto-detection.
///
/// # Errors
/// Returns an error if the agent attempts to modify forbidden content within a
/// policy-guarded path.
pub(crate) fn check_agent_path_policies(
    workspace_root: &Path,
    candidate: &Path,
    original: &str,
    new_content: &str,
    agent_name: Option<&str>,
    guards: &BTreeMap<String, AgentGuard>,
    project: Option<&tkg_common::ProjectConfig>,
) -> Result<(), String> {
    let guard = agent_name.and_then(|a| guards.get(a));
    tkg_common::check_agent_path_policies_direct(
        workspace_root,
        candidate,
        original,
        new_content,
        agent_name,
        guard,
        project,
    )
}

/// Verifies whether the workspace contains unresolved VCS conflict markers.
///
/// # Errors
/// Returns an error if conflict markers (e.g. `<<<<<<<`) are detected in the
/// repository.
pub(crate) fn check_vcs_conflicts(workspace_root: &Path) -> Result<(), String> {
    match tkg_common::VcsConflictCheck.execute(workspace_root) {
        tkg_common::GateCheckStatus::Passed => Ok(()),
        tkg_common::GateCheckStatus::Failed(reason) => Err(format!(
            "Security gate blocked mutation: Unresolved merge conflicts detected: {reason}"
        )),
        tkg_common::GateCheckStatus::Warning(warn) => Err(format!(
            "Security gate blocked mutation: Potential merge conflicts detected: {warn}"
        )),
    }
}

/// Verifies that unauthorized agents do not mutate protected build manifests.
///
/// Neutralizes procedural-macro / `build.rs` execution sandbox escape vectors.
///
/// Delegates to canonical
/// [`tkg_common::manifest_guard::check_manifest_protection`].
///
/// # Errors
/// Returns an error if the agent is not permitted to mutate protected build
/// files.
pub(crate) fn check_manifest_protection(
    target_path: &Path,
    agent_name: Option<&str>,
    guard: Option<&AgentGuard>,
) -> Result<(), String> {
    tkg_common::manifest_guard::check_manifest_protection(target_path, agent_name, guard)
}

const COMMON_ROOT_DIRS: &[&str] = &[
    "src",
    "lib",
    "pkg",
    "app",
    "cmd",
    "tests",
    "test",
    "spec",
    "benches",
    "examples",
    "docs",
    "doc",
    "include",
    "scripts",
    "bin",
    ".git",
    ".jj",
    ".tkg",
    ".opencode",
    ".claude",
];

/// Helper attempting to resolve a scoped path within declared monorepo subcrate
/// containers.
fn resolve_in_subcrates(
    workspace_root: &Path,
    clean: &str,
    scopes: &[&str],
    containers: &[String],
) -> Option<String> {
    if containers.is_empty() {
        return None;
    }

    // Case A: Path starts with "<container>/<crate_name>/..."
    for container in containers {
        let prefix = format!("{container}/");
        if let Some(sub) = clean.strip_prefix(&prefix)
            && let Some((crate_name, rest)) = sub.split_once('/')
        {
            for scope in scopes {
                let candidate = workspace_root
                    .join(container)
                    .join(crate_name)
                    .join(scope)
                    .join(rest);
                if candidate.exists() {
                    return Some(format!("{container}/{crate_name}/{scope}/{rest}"));
                }
            }
        }
    }

    // Case B: Path starts with "<crate_name>/<rest>" where <crate_name> exists
    // in a container
    if let Some((possible_crate, rest)) = clean.split_once('/') {
        for container in containers {
            let crate_dir = workspace_root.join(container).join(possible_crate);
            if crate_dir.is_dir() {
                for scope in scopes {
                    let candidate = crate_dir.join(scope).join(rest);
                    if candidate.exists() {
                        return Some(format!("{container}/{possible_crate}/{scope}/{rest}"));
                    }
                }
            }
        }
    }

    // Case C: Path doesn't mention crate name (e.g. "actions/file_read.rs" or
    // "guards.rs") Find unique match across all crates in all containers
    let mut unique_match: Option<String> = None;
    let mut match_count = 0_usize;

    for container in containers {
        let container_dir = workspace_root.join(container);
        let Ok(read_dir) = std::fs::read_dir(container_dir) else {
            continue;
        };

        for entry in read_dir.flatten() {
            let entry_path = entry.path();
            if !entry_path.is_dir() {
                continue;
            }
            let crate_name = entry.file_name().to_string_lossy().to_string();

            for scope in scopes {
                let candidate = entry_path.join(scope).join(clean);
                if candidate.exists() {
                    match_count += 1;
                    unique_match = Some(format!("{container}/{crate_name}/{scope}/{clean}"));
                }
            }
        }
    }

    if match_count == 1 { unique_match } else { None }
}

fn collect_candidate_scopes(
    tool: Option<&ToolDefinition>,
    agent_context: Option<(&str, &BTreeMap<String, AgentGuard>)>,
) -> Vec<String> {
    let mut scopes = Vec::new();

    if let Some(tool_def) = tool
        && let Some(ref enforce_dir) = tool_def.enforce_dir
    {
        let s = enforce_dir.trim_matches(['/', '\\']).to_owned();
        if !s.is_empty() && s != "*" && !scopes.contains(&s) {
            scopes.push(s);
        }
    }

    if let Some((agent_name, guards)) = agent_context
        && let Some(guard) = guards.get(agent_name)
    {
        for dir in &guard.allowed_dirs {
            let s = dir.trim_matches(['/', '\\']).to_owned();
            if !s.is_empty() && s != "*" && !scopes.contains(&s) {
                scopes.push(s);
            }
        }
    }

    if let Some(tool_def) = tool {
        for dir in &tool_def.allowed_directories {
            let s = dir.trim_matches(['/', '\\']).to_owned();
            if !s.is_empty() && s != "*" && !scopes.contains(&s) {
                scopes.push(s);
            }
        }
    }

    scopes
}

fn anchor_to_primary_scope(
    workspace_root: &Path,
    clean: &str,
    primary_scope: &str,
    containers: &[String],
) -> String {
    for container in containers {
        let prefix = format!("{container}/");
        if let Some(sub) = clean.strip_prefix(&prefix)
            && let Some((crate_name, rest)) = sub.split_once('/')
            && workspace_root.join(container).join(crate_name).is_dir()
        {
            return format!("{container}/{crate_name}/{primary_scope}/{rest}");
        }
    }

    if let Some((possible_crate, rest)) = clean.split_once('/') {
        for container in containers {
            if workspace_root.join(container).join(possible_crate).is_dir() {
                return format!("{container}/{possible_crate}/{primary_scope}/{rest}");
            }
        }
    }

    format!("{primary_scope}/{clean}")
}

/// Resolves a path string relative to agent and tool scopes.
///
/// If an agent or tool is confined to a specific directory (e.g., `src` or
/// `docs`), this resolves relative paths (e.g., `"auth/hasher.rs"` or
/// `"specs/rv32.md"`) to their scoped locations without forcing the caller to
/// supply the parent directory prefix.
///
/// Existing workspace paths, absolute paths, and explicit allowed files are
/// preserved transparently.
#[must_use]
pub(crate) fn resolve_scoped_path_str(
    workspace_root: &Path,
    rel_path_str: &str,
    tool: Option<&ToolDefinition>,
    agent_context: Option<(&str, &BTreeMap<String, AgentGuard>)>,
) -> String {
    let raw_norm = rel_path_str.trim().replace('\\', "/");
    let mut clean = raw_norm.as_str();
    while let Some(stripped) = clean.strip_prefix("./") {
        clean = stripped.trim();
    }

    if clean.is_empty() || Path::new(clean).is_absolute() {
        return clean.to_owned();
    }

    // 0. If tool has enforce_file_target, check if clean matches target or its
    //    filename
    if let Some(target) = tool.and_then(|t| t.enforce_file_target.as_deref()) {
        let clean_target = target.trim_matches(['/', '\\']).replace('\\', "/");
        if clean == clean_target {
            return clean_target;
        }
        if let Some(fname) = Path::new(&clean_target)
            .file_name()
            .and_then(|f| f.to_str())
            && clean == fname
        {
            return clean_target;
        }
    }

    // 1. If explicit allowed file for this agent, leave as-is
    if let Some((agent_name, guards)) = agent_context
        && let Some(guard) = guards.get(agent_name)
        && guard.allowed_files.iter().any(|f| f == clean)
    {
        return clean.to_owned();
    }

    // 2. Collect candidate scopes (ordered: tool enforce_dir, agent
    //    allowed_dirs, tool allowed_directories)
    let scopes = collect_candidate_scopes(tool, agent_context);
    if scopes.is_empty() {
        return clean.to_owned();
    }

    let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();

    let project = tkg_common::ProjectConfig::detect_or_load(workspace_root);
    let containers = project.resolved_containers(workspace_root);

    // 3. If the path ALREADY matches any candidate scope, don't double-prefix
    //    it!
    for scope in &scope_refs {
        if matches_directory_scope_with_containers(clean, scope, &containers) {
            return clean.to_owned();
        }
    }

    // 4. If the agent explicitly attempts to access one of its disallowed
    //    directories,
    // do NOT anchor it into an allowed scope. Leave it intact so security
    // checks will reject it.
    if let Some((agent_name, guards)) = agent_context
        && let Some(guard) = guards.get(agent_name)
        && guard
            .disallowed_dirs
            .iter()
            .any(|d| matches_directory_scope_with_containers(clean, d, &containers))
    {
        return clean.to_owned();
    }

    // 5. If the path already exists directly at workspace root, leave as-is
    //    (e.g. cross-directory read)
    if workspace_root.join(clean).exists() {
        return clean.to_owned();
    }

    // 6. Check if the path exists inside candidate scope directly (e.g.
    //    "src/auth/hasher.rs")
    for scope in &scope_refs {
        let candidate = workspace_root.join(scope).join(clean);
        if candidate.exists() {
            return format!("{scope}/{clean}");
        }
    }

    // 7. Check workspace multi-crate structure using declared containers
    if let Some(resolved) = resolve_in_subcrates(workspace_root, clean, &scope_refs, &containers) {
        return resolved;
    }

    // 8. If clean starts with a known top-level project directory that is NOT
    //    one of our scopes,
    // do NOT anchor it into our scope (e.g. "tests/test.rs" when scoped to
    // "src").
    if let Some((first_component, _)) = clean.split_once('/')
        && !containers.iter().any(|c| c == first_component)
    {
        let is_known_root = COMMON_ROOT_DIRS.contains(&first_component)
            || project.docs.paths.iter().any(|d| d == first_component)
            || workspace_root.join(first_component).is_dir();
        if is_known_root && !scope_refs.contains(&first_component) {
            return clean.to_owned();
        }
    }

    // 9. If file does not exist yet (e.g. writing a new file via file_write),
    //    anchor to primary scope
    let primary = tool
        .and_then(|t| t.enforce_dir.as_deref())
        .map(|s| s.trim_matches(['/', '\\']))
        .filter(|s| !s.is_empty() && *s != "*")
        .or_else(|| scope_refs.first().copied());

    if let Some(primary_scope) = primary {
        return anchor_to_primary_scope(workspace_root, clean, primary_scope, &containers);
    }

    clean.to_owned()
}

/// Truncates string output by lines and bytes safely on UTF-8 boundaries.
pub(crate) fn truncate_output(
    text: &str,
    max_lines: Option<usize>,
    max_bytes: Option<usize>,
    emit_notice: bool,
) -> String {
    let total_lines = text.lines().count();
    let total_bytes = text.len();

    let mut truncated = false;

    // 1. Line-based truncation
    let mut truncated_text = if let Some(limit_lines) = max_lines
        && total_lines > limit_lines
    {
        truncated = true;
        let lines: Vec<&str> = text.lines().take(limit_lines).collect();
        lines.join("\n")
    } else {
        text.to_owned()
    };

    // 2. Byte-based truncation with safe UTF-8 character boundary slicing
    if let Some(limit_bytes) = max_bytes
        && truncated_text.len() > limit_bytes
    {
        truncated = true;
        // Floor to nearest valid char boundary <= limit_bytes
        let mut boundary = limit_bytes;
        while boundary > 0 && !truncated_text.is_char_boundary(boundary) {
            boundary = boundary.saturating_sub(1);
        }

        if let Some(slice) = truncated_text.get(..boundary) {
            truncated_text = slice.to_owned();
        }
    }

    if truncated && emit_notice {
        let notice = format!(
            "\n\n[TRUNCATION NOTICE: Output truncated. Showing {} of {} lines, {} of {} bytes]",
            truncated_text.lines().count(),
            total_lines,
            truncated_text.len(),
            total_bytes
        );
        truncated_text.push_str(&notice);
    }

    truncated_text
}

/// Strips raw ANSI terminal styling sequences (e.g. colors, bold, cursor
/// codes).
pub(crate) fn strip_ansi(input: &str) -> String {
    anstream::adapter::strip_str(input).to_string()
}

/// Filters diagnostic command output using declarative `keep_regex`,
/// `drop_regex`, and `max_lines` rules from `[tools.<name>.output_filter]`.
pub(crate) fn filter_output(input: &str, filter: &tkg_common::tools::ToolOutputFilter) -> String {
    if filter.is_empty() {
        return input.to_owned();
    }

    let keep: Vec<regex::Regex> = filter
        .keep_regex
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect();
    let drop: Vec<regex::Regex> = filter
        .drop_regex
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect();

    let mut filtered: Vec<&str> = Vec::new();
    for line in input.lines() {
        if !keep.is_empty() && !keep.iter().any(|re| re.is_match(line)) {
            continue;
        }
        if drop.iter().any(|re| re.is_match(line)) {
            continue;
        }
        filtered.push(line);
    }

    if let Some(max_lines) = filter.max_lines
        && filtered.len() > max_lines
    {
        let omitted = filtered.len() - max_lines;
        let mut kept: Vec<&str> = filtered.into_iter().take(max_lines).collect();
        let notice = format!("[... {omitted} more line(s) pruned by output_filter ...]");
        kept.push(&notice);
        return kept.join("\n");
    }

    filtered.join("\n")
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "Unit test assertions")]
mod tests {
    use tkg_common::path_matches_pattern;

    use super::*;

    fn check_dangerous_files(path: &Path) -> Result<(), String> {
        check_dangerous_files_with_security(path, None)
    }

    #[test]
    fn test_dangerous_files_rejected() {
        assert!(check_dangerous_files(Path::new(".env")).is_err());
        assert!(check_dangerous_files(Path::new("config/.env.local")).is_err());
        assert!(check_dangerous_files(Path::new("keys/id_rsa")).is_err());
        assert!(check_dangerous_files(Path::new(".git/HEAD")).is_err());
        assert!(check_dangerous_files(Path::new(".jj/working_copy")).is_err());
        assert!(check_dangerous_files(Path::new("secrets/private.pem")).is_err());
        assert!(check_dangerous_files(Path::new("src/lib.rs")).is_ok());
        assert!(check_dangerous_files(Path::new("src/tokenizer.rs")).is_ok());
        assert!(check_dangerous_files(Path::new("src/tokens.rs")).is_ok());
    }

    #[test]
    fn test_path_traversal_rejected() {
        let root = std::env::temp_dir().join(format!(
            "tkg_traversal_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        _ = std::fs::create_dir_all(&root);
        assert!(resolve_safe_path(&root, "../etc/passwd", &[], true).is_err());
        assert!(resolve_safe_path(&root, "foo/../../etc", &[], true).is_err());
        assert!(resolve_safe_path(&root, "/etc/passwd", &[], true).is_err());
        assert!(resolve_safe_path(&root, "/nonexistent/outside/path", &[], true).is_err());
        assert!(resolve_safe_path(&root, "nested_new_dir/file.rs", &[], true).is_ok());
        _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_allowed_directories_enforced() {
        let root = std::env::temp_dir().join(format!(
            "tkg_allowed_dir_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        _ = std::fs::create_dir_all(&root);
        drop(std::fs::write(
            root.join("project.toml"),
            "[workspace]\ncontainers = [\"crates\"]\n",
        ));

        let allowed = vec!["tests".to_owned(), "benches".to_owned()];

        assert!(resolve_safe_path(&root, "tests/unit_test.rs", &allowed, false).is_ok());
        assert!(resolve_safe_path(&root, "crates/core/tests/test.rs", &allowed, false).is_ok());
        assert!(resolve_safe_path(&root, "src/main.rs", &allowed, false).is_err());
        assert!(
            resolve_safe_path(&root, "docs/fixtures/tests/sample.rs", &allowed, false).is_err()
        );

        _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_truncation() {
        let sample = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5";
        let truncated = truncate_output(sample, Some(2), None, true);
        assert!(truncated.contains("Line 1\nLine 2"));
        assert!(truncated.contains("[TRUNCATION NOTICE:"));
    }

    #[test]
    fn test_enforce_boundaries() {
        let root = std::env::temp_dir();

        // Enforce file target
        let good_file = root.join("ARCHITECTURE.md");
        let bad_file = root.join("src/lib.rs");
        assert!(check_enforce_boundaries(&root, &good_file, None, Some("ARCHITECTURE.md")).is_ok());
        assert!(check_enforce_boundaries(&root, &bad_file, None, Some("ARCHITECTURE.md")).is_err());

        // Enforce dir
        let good_dir = root.join("docs/specs/rv32.md");
        let bad_dir = root.join("src/main.rs");
        assert!(check_enforce_boundaries(&root, &good_dir, Some("docs"), None).is_ok());
        assert!(check_enforce_boundaries(&root, &bad_dir, Some("docs"), None).is_err());
    }

    #[test]
    fn test_check_agent_guard() {
        let root = std::env::temp_dir();
        let mut guards = BTreeMap::new();

        guards.insert(
            "worker_agent".to_owned(),
            AgentGuard {
                allowed_dirs: vec!["src".to_owned()],
                disallowed_dirs: vec!["tests".to_owned()],
                allowed_files: Vec::new(),
                disallowed_files: vec!["Cargo.toml".to_owned()],
                ..Default::default()
            },
        );

        guards.insert(
            "docs_agent".to_owned(),
            AgentGuard {
                allowed_dirs: vec!["docs".to_owned()],
                disallowed_dirs: vec!["src".to_owned()],
                allowed_files: vec!["spec.md".to_owned()],
                disallowed_files: Vec::new(),
                ..Default::default()
            },
        );

        // worker_agent tests
        assert!(
            check_agent_guard(
                &root,
                &root.join("src/lib.rs"),
                Some("worker_agent"),
                &guards
            )
            .is_ok()
        );
        assert!(
            check_agent_guard(
                &root,
                &root.join("tests/test.rs"),
                Some("worker_agent"),
                &guards
            )
            .is_err()
        );
        assert!(
            check_agent_guard(
                &root,
                &root.join("Cargo.toml"),
                Some("worker_agent"),
                &guards
            )
            .is_err()
        );

        // docs_agent tests
        assert!(
            check_agent_guard(&root, &root.join("spec.md"), Some("docs_agent"), &guards).is_ok()
        );
        assert!(
            check_agent_guard(
                &root,
                &root.join("docs/rfc.md"),
                Some("docs_agent"),
                &guards
            )
            .is_ok()
        );
        assert!(
            check_agent_guard(&root, &root.join("src/lib.rs"), Some("docs_agent"), &guards)
                .is_err()
        );

        // Unrestricted agent
        assert!(
            check_agent_guard(
                &root,
                &root.join("src/lib.rs"),
                Some("unrestricted_agent"),
                &guards
            )
            .is_ok()
        );
        assert!(check_agent_guard(&root, &root.join("tests/test.rs"), None, &guards).is_ok());
    }

    fn setup_test_guards() -> (PathBuf, BTreeMap<String, AgentGuard>) {
        let root = PathBuf::from("/workspace");
        let mut guards = BTreeMap::new();

        guards.insert(
            "comments_guarded_agent".to_owned(),
            AgentGuard {
                path_policies: vec![tkg_common::PathPolicy::comments_only("src")],
                ..Default::default()
            },
        );

        guards.insert(
            "tests_guarded_agent".to_owned(),
            AgentGuard {
                path_policies: vec![tkg_common::PathPolicy {
                    test_attributes: vec!["test".to_owned(), "cfg(test".to_owned()],
                    ..tkg_common::PathPolicy::tests_only("src")
                }],
                ..Default::default()
            },
        );

        (root, guards)
    }

    #[test]
    fn test_check_agent_path_policies_comments_only() {
        let (root, guards) = setup_test_guards();
        let orig = "pub fn foo() -> u32 { 1 }";
        let with_docs = "/// Doc comment\npub fn foo() -> u32 {\n    // Inline comment\n    1\n}";
        let with_code_tamper = "pub fn foo() -> u32 { 2 }";

        // comments_guarded_agent: comments allowed, code edits blocked in src/
        assert!(
            check_agent_path_policies(
                &root,
                &root.join("src/lib.rs"),
                orig,
                with_docs,
                Some("comments_guarded_agent"),
                &guards,
                None
            )
            .is_ok()
        );
        assert!(
            check_agent_path_policies(
                &root,
                &root.join("src/lib.rs"),
                orig,
                with_code_tamper,
                Some("comments_guarded_agent"),
                &guards,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn test_check_agent_path_policies_tests_only() {
        let (root, guards) = setup_test_guards();
        let orig = "pub fn foo() -> u32 { 1 }";
        let with_code_tamper = "pub fn foo() -> u32 { 2 }";

        // tests_guarded_agent: tests allowed in src/, production code edits
        // blocked
        let with_unit_test =
            "pub fn foo() -> u32 { 1 }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}";
        assert!(
            check_agent_path_policies(
                &root,
                &root.join("src/lib.rs"),
                orig,
                with_unit_test,
                Some("tests_guarded_agent"),
                &guards,
                None
            )
            .is_ok()
        );
        assert!(
            check_agent_path_policies(
                &root,
                &root.join("src/lib.rs"),
                orig,
                with_code_tamper,
                Some("tests_guarded_agent"),
                &guards,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn test_check_agent_path_policies_unconfigured_agent() {
        let (root, guards) = setup_test_guards();
        let orig = "pub fn foo() -> u32 { 1 }";
        let with_code_tamper = "pub fn foo() -> u32 { 2 }";

        // unconfigured agent: edits allowed
        assert!(
            check_agent_path_policies(
                &root,
                &root.join("src/lib.rs"),
                orig,
                with_code_tamper,
                Some("unconfigured_agent"),
                &guards,
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn test_check_agent_path_policies_declarative_project() {
        let (root, guards) = setup_test_guards();

        // Declarative project.toml custom path policy for Python
        let mut proj = tkg_common::ProjectConfig::default();
        proj.project.language = Some("python".to_owned());
        proj.policies.syntax = Some(tkg_common::SyntaxConfig {
            line_comments: vec!["#".to_owned()],
            block_comments: vec![tkg_common::BlockCommentDelimiters::new("\"\"\"", "\"\"\"")],
            ..Default::default()
        });
        proj.path_policies.push(tkg_common::PathPolicy {
            path: "app".to_owned(),
            filter: tkg_common::ContentFilterKind::CommentsOnly,
            ..Default::default()
        });

        let py_orig = "def add(a, b):\n    return a + b\n";
        let py_docs =
            "def add(a, b):\n    \"\"\"Docstring\"\"\"\n    # comment\n    return a + b\n";
        let py_tamper = "def add(a, b):\n    return a + b + 1\n";

        assert!(
            check_agent_path_policies(
                &root,
                &root.join("app/math.py"),
                py_orig,
                py_docs,
                Some("comments_guarded_agent"),
                &guards,
                Some(&proj)
            )
            .is_ok()
        );
        assert!(
            check_agent_path_policies(
                &root,
                &root.join("app/math.py"),
                py_orig,
                py_tamper,
                Some("comments_guarded_agent"),
                &guards,
                Some(&proj)
            )
            .is_err()
        );
    }

    #[test]
    fn test_path_matches_pattern_wildcards() {
        assert!(path_matches_pattern(
            "src/auth/password/hasher.rs",
            "src/auth/password/**"
        ));
        assert!(path_matches_pattern(
            "src/auth/password",
            "src/auth/password/**"
        ));
        assert!(path_matches_pattern("fixtures/keys/cert.pem", "*.pem"));
        assert!(path_matches_pattern(".env.test", ".env*"));
        assert!(path_matches_pattern("tests/secrets/token.json", "*"));
        assert!(path_matches_pattern(
            "deep/nested/path/firmware.bin",
            "firmware.bin"
        ));
        // Standard globs unlocked by globset
        assert!(path_matches_pattern(
            "src/controllers/user.rs",
            "src/**/*.rs"
        ));
        assert!(path_matches_pattern("secret.key", "*.{key,pem}"));
        assert!(path_matches_pattern("cert.pem", "*.{key,pem}"));
        assert!(!path_matches_pattern("doc.md", "*.{key,pem}"));
    }

    #[test]
    fn test_modular_security_credentials_and_allowlist() {
        let path = Path::new("src/auth/password/mod.rs");
        // By default, generic credential "password" is blocked
        assert!(check_dangerous_files_with_security(path, None).is_err());

        // With block_generic_credentials = false, it is allowed
        let mut sec_perm = ProjectSecurityConfig::default();
        sec_perm.set_category_blocked(tkg_common::SecurityCategory::GenericCredentials, false);
        assert!(check_dangerous_files_with_security(path, Some(&sec_perm)).is_ok());

        // With allowed_patterns, specific directory is allowed even if category
        // blocked
        let sec_allow = ProjectSecurityConfig {
            allowed_patterns: vec!["src/auth/password/**".into()],
            ..Default::default()
        };
        assert!(check_dangerous_files_with_security(path, Some(&sec_allow)).is_ok());

        // But unrelated credentials are still blocked
        assert!(
            check_dangerous_files_with_security(
                Path::new("src/secrets/token.txt"),
                Some(&sec_allow)
            )
            .is_err()
        );

        // With extra_banned_patterns, custom paths are rejected
        let sec_banned = ProjectSecurityConfig {
            extra_banned_patterns: vec!["internal_firmware.bin".into()],
            ..Default::default()
        };
        assert!(
            check_dangerous_files_with_security(
                Path::new("crates/core/internal_firmware.bin"),
                Some(&sec_banned)
            )
            .is_err()
        );
    }

    #[test]
    fn test_resolve_scoped_path_str_single_crate() {
        let root = std::env::temp_dir().join(format!("tkg_scope_test_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/auth")).expect("create src/auth");
        std::fs::create_dir_all(root.join("docs/specs")).expect("create docs/specs");
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n")
            .expect("write root Cargo.toml");
        std::fs::write(root.join("src/auth/hasher.rs"), "// hasher").expect("write hasher.rs");
        std::fs::write(root.join("docs/specs/rv32.md"), "# RV32").expect("write rv32.md");

        let mut guards = BTreeMap::new();
        guards.insert(
            "worker_agent".to_owned(),
            AgentGuard {
                allowed_dirs: vec!["src".to_owned()],
                disallowed_dirs: vec!["tests".to_owned()],
                allowed_files: Vec::new(),
                disallowed_files: vec!["Cargo.toml".to_owned()],
                ..Default::default()
            },
        );
        guards.insert(
            "docs_agent".to_owned(),
            AgentGuard {
                allowed_dirs: vec!["docs".to_owned()],
                disallowed_dirs: vec!["src".to_owned()],
                allowed_files: vec!["spec.md".to_owned()],
                disallowed_files: Vec::new(),
                ..Default::default()
            },
        );

        // 1. worker_agent scoped resolution
        assert_eq!(
            resolve_scoped_path_str(
                &root,
                "auth/hasher.rs",
                None,
                Some(("worker_agent", &guards))
            ),
            "src/auth/hasher.rs"
        );

        // 2. Already prefixed remains unchanged
        assert_eq!(
            resolve_scoped_path_str(
                &root,
                "src/auth/hasher.rs",
                None,
                Some(("worker_agent", &guards))
            ),
            "src/auth/hasher.rs"
        );

        // 3. Leading ./ stripped
        assert_eq!(
            resolve_scoped_path_str(
                &root,
                "./auth/hasher.rs",
                None,
                Some(("worker_agent", &guards))
            ),
            "src/auth/hasher.rs"
        );

        // 4. Absolute path preserved
        assert_eq!(
            resolve_scoped_path_str(
                &root,
                "/tmp/absolute.rs",
                None,
                Some(("worker_agent", &guards))
            ),
            "/tmp/absolute.rs"
        );

        // 5. Existing root file preserved for read/inspection
        assert_eq!(
            resolve_scoped_path_str(&root, "Cargo.toml", None, Some(("worker_agent", &guards))),
            "Cargo.toml"
        );

        // 6. Explicit allowed file for agent preserved
        assert_eq!(
            resolve_scoped_path_str(&root, "spec.md", None, Some(("docs_agent", &guards))),
            "spec.md"
        );

        // 7. Disallowed directory is NOT rewritten into allowed scope
        //    (preserves security rejection)
        assert_eq!(
            resolve_scoped_path_str(
                &root,
                "tests/unit_test.rs",
                None,
                Some(("worker_agent", &guards))
            ),
            "tests/unit_test.rs"
        );

        _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_resolve_scoped_path_str_tools_and_new_files() {
        let root = std::env::temp_dir().join(format!("tkg_scope_tools_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/auth")).expect("create src/auth");
        std::fs::create_dir_all(root.join("docs/specs")).expect("create docs/specs");
        std::fs::write(root.join("docs/specs/rv32.md"), "# RV32").expect("write rv32.md");

        let mut guards = BTreeMap::new();
        guards.insert(
            "worker_agent".to_owned(),
            AgentGuard {
                allowed_dirs: vec!["src".to_owned()],
                ..Default::default()
            },
        );

        // 8. Tool enforce_dir resolution
        let tool_docs = ToolDefinition {
            name: "read_spec".to_owned(),
            enforce_dir: Some("docs".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            resolve_scoped_path_str(&root, "specs/rv32.md", Some(&tool_docs), None),
            "docs/specs/rv32.md"
        );

        // 9. Tool enforce_file_target resolution
        let tool_arch = ToolDefinition {
            name: "update_arch".to_owned(),
            enforce_file_target: Some("docs/ARCHITECTURE.md".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            resolve_scoped_path_str(&root, "ARCHITECTURE.md", Some(&tool_arch), None),
            "docs/ARCHITECTURE.md"
        );

        // 10. New file creation in scope when file does not exist yet
        assert_eq!(
            resolve_scoped_path_str(&root, "auth/jwt.rs", None, Some(("worker_agent", &guards))),
            "src/auth/jwt.rs"
        );

        _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_resolve_scoped_path_str_multi_crate() {
        let root = std::env::temp_dir().join(format!("tkg_multi_crate_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("crates/core/src")).expect("create crates/core/src");
        std::fs::create_dir_all(root.join("crates/api/src")).expect("create crates/api/src");
        std::fs::write(
            root.join("project.toml"),
            "[workspace]\ncontainers = [\"crates\"]\n",
        )
        .expect("write project.toml");

        std::fs::write(root.join("crates/core/src/hasher.rs"), "// hasher")
            .expect("write hasher.rs");
        std::fs::write(root.join("crates/core/src/lib.rs"), "// core lib")
            .expect("write core lib.rs");
        std::fs::write(root.join("crates/api/src/lib.rs"), "// api lib").expect("write api lib.rs");

        let mut guards = BTreeMap::new();
        guards.insert(
            "worker_agent".to_owned(),
            AgentGuard {
                allowed_dirs: vec!["src".to_owned()],
                ..Default::default()
            },
        );

        // 1. Scoped path with crate prefix "core/hasher.rs" ->
        //    "crates/core/src/hasher.rs"
        assert_eq!(
            resolve_scoped_path_str(
                &root,
                "core/hasher.rs",
                None,
                Some(("worker_agent", &guards))
            ),
            "crates/core/src/hasher.rs"
        );

        // 2. Scoped path with "crates/core/hasher.rs" ->
        //    "crates/core/src/hasher.rs"
        assert_eq!(
            resolve_scoped_path_str(
                &root,
                "crates/core/hasher.rs",
                None,
                Some(("worker_agent", &guards))
            ),
            "crates/core/src/hasher.rs"
        );

        // 3. Unique file across crates ("hasher.rs") ->
        //    "crates/core/src/hasher.rs"
        assert_eq!(
            resolve_scoped_path_str(&root, "hasher.rs", None, Some(("worker_agent", &guards))),
            "crates/core/src/hasher.rs"
        );

        // 4. Ambiguous file ("lib.rs") in multiple crates falls back without
        //    guessing
        assert_eq!(
            resolve_scoped_path_str(&root, "lib.rs", None, Some(("worker_agent", &guards))),
            "src/lib.rs"
        );

        // 5. Creating new file in crate: "core/token.rs" ->
        //    "crates/core/src/token.rs"
        assert_eq!(
            resolve_scoped_path_str(
                &root,
                "core/token.rs",
                None,
                Some(("worker_agent", &guards))
            ),
            "crates/core/src/token.rs"
        );

        // 6. Creating new file with full crates prefix: "crates/core/token.rs"
        //    -> "crates/core/src/token.rs"
        assert_eq!(
            resolve_scoped_path_str(
                &root,
                "crates/core/token.rs",
                None,
                Some(("worker_agent", &guards))
            ),
            "crates/core/src/token.rs"
        );

        _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_strip_ansi() {
        let raw = "\u{1b}[2m\u{1b}[0m\u{1b}[38;2;84;142;200mcrate\u{1b}[0m::\u{1b}[38;2;84;142;200mserver\u{1b}[0m";
        assert_eq!(strip_ansi(raw), "crate::server");
    }

    #[test]
    fn test_check_manifest_protection() {
        let cargo_toml = Path::new("Cargo.toml");
        let src_file = Path::new("src/main.rs");

        // Normal source files are not protected
        assert!(check_manifest_protection(src_file, Some("reviewer"), None).is_ok());

        // Manifest file without guard: coder is allowed
        assert!(check_manifest_protection(cargo_toml, Some("coder"), None).is_ok());

        // Manifest file with unauthorized agent: blocked
        let guard = AgentGuard {
            allowed_files: vec!["README.md".to_owned()],
            ..Default::default()
        };
        let err = check_manifest_protection(cargo_toml, Some("reviewer"), Some(&guard));
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("Manifest protection violation"));

        // Manifest file with explicit wildcard or explicit allowance: allowed
        let allowed_guard = AgentGuard {
            allowed_files: vec!["Cargo.toml".to_owned()],
            ..Default::default()
        };
        assert!(
            check_manifest_protection(cargo_toml, Some("reviewer"), Some(&allowed_guard)).is_ok()
        );
    }

    #[test]
    fn test_output_filter_prunes_compiler_noise() {
        use std::fmt::Write as _;

        let mut log = String::new();
        for i in 0..998 {
            writeln!(log, "   Compiling crate_{i} v0.1.0").unwrap();
        }
        log.push_str("error[E0308]: mismatched types\n");
        log.push_str("error: aborting due to 1 previous error\n");

        let filter = tkg_common::tools::ToolOutputFilter {
            keep_regex: vec!["^error".to_owned()],
            drop_regex: vec!["^\\s*Compiling ".to_owned()],
            max_lines: Some(25),
        };

        let pruned = filter_output(&log, &filter);
        let lines: Vec<&str> = pruned.lines().collect();
        assert!(lines.len() < 30, "expected < 30 lines, got {}", lines.len());
        assert!(pruned.contains("error[E0308]: mismatched types"));
        assert!(!pruned.contains("Compiling crate_500"));
    }

    mod prop_tests {
        use proptest::prelude::*;

        use super::*;

        /// Independent lexical path scope oracle (no production helper reuse).
        fn independent_strictly_inside(raw: &str, scope_dir: &str) -> bool {
            if raw.is_empty() || raw.contains('\0') {
                return false;
            }
            if raw.starts_with('/') || raw.starts_with('\\') {
                return false;
            }
            let bytes = raw.as_bytes();
            if bytes.get(1) == Some(&b':') && bytes.first().is_some_and(u8::is_ascii_alphabetic) {
                return false;
            }
            let mut components = Vec::new();
            for part in raw.split(['/', '\\']) {
                match part {
                    "" | "." => {},
                    ".." => return false,
                    other => components.push(other),
                }
            }
            components.first().is_some_and(|first| *first == scope_dir)
        }

        /// Weighted 80/20 hostile path segment grammar (traversal, NUL, lookalike dots, secrets).
        fn hostile_segment() -> impl Strategy<Value = String> {
            prop_oneof![
                8 => prop_oneof![
                    Just(".".to_owned()),
                    Just("..".to_owned()),
                    Just("...".to_owned()),
                    Just(String::new()),
                    Just("~".to_owned()),
                    Just("%2e%2e".to_owned()),
                    Just("..\0ignored".to_owned()),
                    Just(".\u{2024}".to_owned()),
                    Just("foo/../../bar".to_owned()),
                    Just("src\\..\\outside".to_owned()),
                    Just(".env".to_owned()),
                    Just(".git".to_owned()),
                    Just("Cargo.toml".to_owned()),
                ],
                2 => "[a-z0-9_-]{1,10}",
            ]
        }

        /// Assembles hostile relative and rooted paths mixing `/` and `\` separators.
        fn hostile_path() -> impl Strategy<Value = String> {
            (
                any::<bool>(),
                any::<bool>(),
                prop::collection::vec(hostile_segment(), 1..10),
            )
                .prop_map(|(leading_slash, backslash_sep, parts)| {
                    let sep = if backslash_sep { "\\" } else { "/" };
                    let joined = parts.join(sep);
                    if leading_slash {
                        format!("{sep}{joined}")
                    } else {
                        joined
                    }
                })
        }

        #[cfg(unix)]
        fn hostile_raw_byte_vec() -> impl Strategy<Value = Vec<u8>> {
            prop::collection::vec(
                prop_oneof![
                    4 => prop_oneof![
                        Just(0x00u8),
                        Just(0xFFu8),
                        Just(0xFEu8),
                        Just(0x2Fu8), // '/'
                        Just(0x5Cu8), // '\'
                        Just(b'.'),
                    ],
                    3 => prop_oneof![Just(b's'), Just(b'r'), Just(b'c'), Just(b'e'), Just(b'n'), Just(b'v')],
                ],
                1..24,
            )
        }

        proptest! {
        #[test]
        fn prop_resolve_safe_path_rejects_escapes_and_stays_inside_root(
            subdir in "[a-z]{2,8}",
            file in "[a-z0-9_]{1,10}\\.rs",
            traversal in prop::collection::vec(prop_oneof![Just(".."), Just("."), Just("escape")], 1..5),
        ) {
            let dir = std::env::temp_dir().join(format!("tkg_mcp_prop_ws_{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("mkdir temp ws");
            let canonical_ws = dir.canonicalize().unwrap_or_else(|_| dir.clone());

            // Valid nested workspace-relative path resolves strictly inside workspace root
            let valid_rel = format!("{subdir}/{file}");
            let ok_path = resolve_safe_path(&dir, &valid_rel, std::slice::from_ref(&subdir), true);
            prop_assert!(ok_path.is_ok(), "valid path failed: {ok_path:?}");
            if let Ok(resolved) = ok_path {
                prop_assert!(resolved.starts_with(&dir) || resolved.starts_with(&canonical_ws));
            }

            // Explicit traversal or root escape must fail closed
            let escaped = format!("{subdir}/{}/{file}", traversal.join("/"));
            if traversal.contains(&"..") {
                prop_assert!(resolve_safe_path(&dir, &escaped, &[], true).is_err());
            }

            let root_escape = if cfg!(windows) {
                format!("C:\\Windows\\System32\\{file}")
            } else {
                format!("/etc/{file}")
            };
            prop_assert!(resolve_safe_path(&dir, &root_escape, &[], true).is_err());
        }

        #[test]
        fn prop_truncate_output_and_strip_ansi_bound_output(
            lines in prop::collection::vec("\\PC{0,40}", 0..40),
            max_lines in 0usize..=15,
            max_bytes in 1usize..=200,
        ) {
            let raw = lines.join("\n");
            let ansi_wrapped = format!("\u{1b}[31m{raw}\u{1b}[0m");
            let stripped = strip_ansi(&ansi_wrapped);
            let has_ansi = stripped.contains("\u{1b}[");
            prop_assert!(!has_ansi);

            let bounded = truncate_output(&raw, Some(max_lines), Some(max_bytes), false);
            prop_assert!(bounded.len() <= max_bytes);
            if max_lines == 0 && raw.lines().count() > 0 {
                prop_assert!(bounded.is_empty());
            } else {
                prop_assert!(bounded.lines().count() <= max_lines.max(1));
            }
        }

        #[test]
        fn prop_filter_output_never_exceeds_max_lines(
            lines in prop::collection::vec("[a-zA-Z0-9 :_-]{1,30}", 0..60),
            max_lines in 1usize..=20,
        ) {
            let raw = lines.join("\n");
            let filter = tkg_common::tools::ToolOutputFilter {
                keep_regex: vec!["error".to_owned()],
                drop_regex: vec!["Compiling".to_owned()],
                max_lines: Some(max_lines),
            };
            let out = filter_output(&raw, &filter);
            // filter_output may append one "... trimmed ..." summary line on overflow
            prop_assert!(out.lines().count() <= max_lines + 3);
        }

        #[test]
        fn prop_agent_guard_strictly_confines_arbitrary_paths(
            allowed_dir in "[a-z]{2,8}",
            denied_dir in "[a-z]{2,8}",
            arbitrary in prop_oneof![
                any::<String>(),
                "\\PC*",
            ],
            attack_variant in prop_oneof![
                Just(0u8), // raw arbitrary
                Just(1u8), // <allowed>/../<arbitrary>
                Just(2u8), // <allowed>\\..\\<arbitrary>
                Just(3u8), // <allowed>_extra/<arbitrary>
                Just(4u8), // <allowed>.bak/<arbitrary>
            ],
            sub_file in "[a-z0-9_]{1,10}\\.rs",
        ) {
            let root = Path::new("/tmp/tkg_guard_prop_root");
            let mut guards = BTreeMap::new();
            guards.insert(
                "scoped".to_owned(),
                AgentGuard {
                    allowed_dirs: vec![allowed_dir.clone()],
                    disallowed_dirs: vec![denied_dir.clone()],
                    ..Default::default()
                },
            );

            // 1. Prefix-lookalike (`<allowed>_extra/...`) must never pass allowed_dirs.
            let prefix_lookalike = format!("{allowed_dir}_extra/{sub_file}");
            let lookalike_res =
                check_agent_guard(root, &root.join(&prefix_lookalike), Some("scoped"), &guards);
            prop_assert!(lookalike_res.is_err());

            let candidate = match attack_variant {
                1 => format!("{allowed_dir}/../{arbitrary}"),
                2 => format!("{allowed_dir}\\..\\{arbitrary}"),
                3 => format!("{allowed_dir}_extra/{arbitrary}"),
                4 => format!("{allowed_dir}.bak/{arbitrary}"),
                _ => arbitrary,
            };

            // 2. Security invariant using independent lexical oracle:
            //    Anything NOT strictly inside `allowed_dir` (or inside `denied_dir`) MUST be rejected!
            let arb_res = check_agent_guard(root, &root.join(&candidate), Some("scoped"), &guards);
            let inside_allowed = independent_strictly_inside(&candidate, &allowed_dir);
            let inside_denied = independent_strictly_inside(&candidate, &denied_dir);
            if arb_res.is_ok() {
                prop_assert!(inside_allowed, "guard allowed unsafe/outside path: {candidate:?}");
                prop_assert!(!inside_denied, "guard allowed denied path: {candidate:?}");
            }
        }

        #[test]
        fn prop_enforce_boundaries_exact_on_arbitrary_strings(
            target_dir in "[a-z]{2,8}",
            target_file in "[a-z0-9_]{2,10}/[a-z0-9_]{2,10}\\.rs",
            arbitrary in any::<String>(),
            sub in "[a-z0-9_]{1,8}\\.rs",
        ) {
            let root = Path::new("/tmp/tkg_boundaries_prop_root");

            // Exact file lock: if check_enforce_boundaries passes, normalized path must equal target_file.
            let file_res =
                check_enforce_boundaries(root, &root.join(&arbitrary), None, Some(&target_file));
            if file_res.is_ok() {
                let norm = arbitrary.replace('\\', "/");
                let trimmed = norm.trim_start_matches("./");
                prop_assert_eq!(trimmed, target_file.as_str());
            }

            // Enforce dir rejects prefix-lookalike (`<target_dir>_leak/x.rs`) and accepts `<target_dir>/<sub>`.
            let lookalike = format!("{target_dir}_leak/{sub}");
            prop_assert!(check_enforce_boundaries(root, &root.join(&lookalike), Some(&target_dir), None).is_err());

            let inside = format!("{target_dir}/{sub}");
            prop_assert!(check_enforce_boundaries(root, &root.join(&inside), Some(&target_dir), None).is_ok());

            // Any path permitted by `enforce_dir` must satisfy the independent confinement oracle.
            let dir_res =
                check_enforce_boundaries(root, &root.join(&arbitrary), Some(&target_dir), None);
            if dir_res.is_ok() {
                prop_assert!(
                    independent_strictly_inside(&arbitrary, &target_dir),
                    "enforce_dir allowed outside/traversal path: {arbitrary:?}"
                );
            }
        }

        #[test]
        fn prop_dangerous_files_rejects_arbitrary_secret_variants(
            prefix in "\\PC{0,16}",
            suffix in "\\PC{0,16}",
        ) {
            let safe_prefix: String = prefix
                .chars()
                .filter(|c| *c != '/' && *c != '\\' && *c != '\0')
                .collect();
            let safe_suffix: String = suffix
                .chars()
                .filter(|c| *c != '/' && *c != '\\' && *c != '\0')
                .collect();
            let dir_part = if safe_prefix.is_empty() || safe_prefix == "." || safe_prefix == ".." {
                "sub".to_owned()
            } else {
                safe_prefix
            };
            let tail_part = if safe_suffix.is_empty() {
                "val".to_owned()
            } else {
                safe_suffix
            };

            for dangerous in [
                ".env".to_owned(),
                format!("{dir_part}/.env"),
                format!("{dir_part}/.env.{tail_part}"),
                format!(".git/{tail_part}"),
                format!(".jj/{tail_part}"),
                format!("secrets/{tail_part}.pem"),
                format!("{dir_part}/{tail_part}.key"),
                format!("{dir_part}/id_rsa"),
                format!("{dir_part}/id_ed25519"),
            ] {
                prop_assert!(
                    check_dangerous_files(Path::new(&dangerous)).is_err(),
                    "expected reject for {dangerous:?}"
                );
            }
        }

        #[test]
        fn prop_hostile_path_grammar_and_symlink_escape_contained(
            path in hostile_path(),
            allow_src_only in any::<bool>(),
            tail in "[a-z0-9_]{1,8}\\.txt",
        ) {
            let base = std::env::temp_dir().join(format!(
                "tkg_hostile_path_prop_{}",
                std::process::id()
            ));
            let ws = base.join("ws");
            let outside = base.join("outside_secret_dir");
            _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(ws.join("src")).expect("mkdir ws/src");
            std::fs::create_dir_all(&outside).expect("mkdir outside");
            std::fs::write(outside.join("secret.txt"), "TOP_SECRET").expect("write outside");

            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, ws.join("src/symlink_escape"))
                .expect("create symlink escape");

            let canonical_ws = ws.canonicalize().expect("canon ws");
            let allowed = if allow_src_only {
                vec!["src".to_owned()]
            } else {
                Vec::new()
            };

            // 1. Real Unix symlink inside `src/` pointing outside workspace MUST always fail closed.
            #[cfg(unix)]
            {
                prop_assert!(
                    resolve_safe_path(&ws, "src/symlink_escape/secret.txt", &allowed, true)
                        .is_err()
                );
                let nested_sym = format!("src/symlink_escape/{tail}");
                prop_assert!(resolve_safe_path(&ws, &nested_sym, &allowed, true).is_err());
            };

            // 2. Hostile path grammar: unconditional prefix containment & resolution idempotence.
            if let Ok(resolved) = resolve_safe_path(&ws, &path, &allowed, true) {
                let canon_check = resolved
                    .ancestors()
                    .find(|ancestor| ancestor.exists())
                    .and_then(|ancestor| ancestor.canonicalize().ok());
                prop_assert!(
                    canon_check.as_ref().is_some_and(|c| c.starts_with(&canonical_ws)),
                    "escaped canonical root: resolved={:?}, raw={:?}",
                    resolved,
                    path
                );
                let res_lossy = resolved.to_string_lossy();
                prop_assert!(!res_lossy.contains('\0'));

                if let Ok(rel_after) = resolved.strip_prefix(&ws) {
                    let rel_str = rel_after.to_string_lossy();
                    let rel_arg = if rel_str.is_empty() { "." } else { &rel_str };
                    let again = resolve_safe_path(&ws, rel_arg, &allowed, true);
                    prop_assert_eq!(again, Ok(resolved));
                }
            }

            _ = std::fs::remove_dir_all(&base);
        }

        #[cfg(unix)]
        #[test]
        fn prop_raw_non_utf8_osstring_paths_fail_closed(
            raw_bytes in hostile_raw_byte_vec(),
        ) {
            use std::os::unix::ffi::OsStringExt;

            let root = Path::new("/tmp/tkg_osstr_prop_root");
            let os_str = std::ffi::OsString::from_vec(raw_bytes.clone());
            let raw_path = PathBuf::from(&os_str);
            let is_valid_utf8 = std::str::from_utf8(&raw_bytes).is_ok();
            let has_nul = raw_bytes.contains(&0);

            if !is_valid_utf8 || has_nul {
                prop_assert!(check_dangerous_files_with_security(&raw_path, None).is_err());

                let guards = BTreeMap::from([(
                    "worker".to_owned(),
                    AgentGuard {
                        allowed_dirs: vec!["src".to_owned()],
                        allowed_files: vec!["src/\u{FFFD}".to_owned()],
                        ..Default::default()
                    },
                )]);
                prop_assert!(
                    check_agent_guard(root, &root.join(&raw_path), Some("worker"), &guards).is_err(),
                    "non-utf8 or NUL path passed check_agent_guard: {:?}",
                    raw_bytes
                );
            }
        }

        #[test]
        fn prop_agent_guard_monotonicity_of_restriction(
            path in hostile_path(),
            allow_pool in prop::collection::vec(prop_oneof![Just("src"), Just("docs"), Just("tests")], 1..4),
            extra_deny_dir in prop_oneof![Just("src"), Just("docs"), Just("secret")],
            extra_deny_file in prop_oneof![Just("src/lib.rs"), Just("Cargo.toml"), Just("docs/a.md")],
            drop_allow in any::<bool>(),
        ) {
            let root = Path::new("/tmp/tkg_mono_prop_root");
            let allowed_dirs: Vec<String> = allow_pool.iter().map(|s| (*s).to_owned()).collect();
            let base_guard = AgentGuard {
                allowed_dirs: allowed_dirs.clone(),
                disallowed_dirs: vec!["private".to_owned()],
                allowed_files: vec!["docs/a.md".to_owned()],
                disallowed_files: vec![".env".to_owned()],
                ..Default::default()
            };

            let mut stricter_dirs = allowed_dirs;
            if drop_allow && stricter_dirs.len() > 1 {
                _ = stricter_dirs.pop();
            }
            let stricter_guard = AgentGuard {
                allowed_dirs: stricter_dirs,
                disallowed_dirs: vec!["private".to_owned(), extra_deny_dir.to_owned()],
                allowed_files: if drop_allow {
                    Vec::new()
                } else {
                    vec!["docs/a.md".to_owned()]
                },
                disallowed_files: vec![".env".to_owned(), extra_deny_file.to_owned()],
                ..Default::default()
            };

            let base_map = BTreeMap::from([("agent".to_owned(), base_guard)]);
            let strict_map = BTreeMap::from([("agent".to_owned(), stricter_guard)]);
            let candidate = root.join(&path);

            let base_ok = check_agent_guard(root, &candidate, Some("agent"), &base_map).is_ok();
            let strict_ok = check_agent_guard(root, &candidate, Some("agent"), &strict_map).is_ok();

            // Monotonicity: restricting a guard can NEVER allow a path that base rejected.
            if strict_ok {
                prop_assert!(base_ok, "monotonicity violated on path {:?}", path);
            }
        }

        #[test]
        fn prop_filter_output_handles_pathological_regex_and_empty(
            payload_len in 50usize..400,
            pattern in prop_oneof![
                Just(String::new()),
                Just("(a+)+$".to_owned()),
                Just("((((a|b)+)+)+)+".to_owned()),
                Just("[unclosed".to_owned()),
            ],
            max_lines in 1usize..20,
        ) {
            let line = format!("{}!", "a".repeat(payload_len));
            let raw = format!("{line}\n{line}\nother");
            let filter = tkg_common::tools::ToolOutputFilter {
                keep_regex: vec![pattern.clone()],
                drop_regex: vec![pattern],
                max_lines: Some(max_lines),
            };
            let filtered = filter_output(&raw, &filter);
            prop_assert!(filtered.lines().count() <= max_lines + 3);
        }
        }
    }
}
