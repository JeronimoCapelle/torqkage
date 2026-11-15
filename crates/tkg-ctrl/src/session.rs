//! Project-specific conversation session discovery delegated to the unified
//! [`Harness`](tkg_common::harness::Harness) trait.

use std::path::Path;

pub(crate) type SessionSummary = tkg_common::sessions::SessionMeta;

/// Discovers the latest resumable session for a given agent in the given
/// workspace using explicit runner settings.
#[must_use]
pub(crate) fn find_agent_session_with_runner(
    agent_name: &str,
    workspace_root: &Path,
    runner_cfg: &tkg_common::ProjectRunnerConfig,
) -> Option<SessionSummary> {
    runner_cfg
        .harness()
        .latest_session(agent_name, workspace_root)
}

/// Discovers the latest resumable session for a given agent in the given
/// workspace by resolving `project.toml`.
#[must_use]
pub(crate) fn find_agent_session_for_workspace(
    agent_name: &str,
    workspace_root: &Path,
) -> Option<SessionSummary> {
    let project_cfg = tkg_common::ProjectConfig::detect_or_load(workspace_root);
    let runner = project_cfg.resolved_runner();
    find_agent_session_with_runner(agent_name, workspace_root, &runner)
}
