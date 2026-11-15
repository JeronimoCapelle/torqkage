//! # harness
//!
//! The [`Harness`] trait is the single capability-driven seam between
//! `TorqKage` and an agent execution harness (Antigravity, `OpenCode`, Claude
//! Code, Cursor, or any custom CLI). Every harness concern — static flags and
//! directory layout, [`HarnessCapabilities`], hook wire dialect
//! ([`HookDialect`]), session discovery ([`SessionMeta`]), spawn command
//! assembly, and config injection — lives in an implementation of this trait.
//!
//! Adding a new harness means: implement [`Harness`] and register it in
//! [`resolve`] (and [`builtin_harnesses`]).
//!
//! This module also hosts harness-neutral configuration discovery used by every
//! binary.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    error::TkgError,
    harness_install,
    hook_dialect::{CLAUDE_CODE_DIALECT, HookDialect, PROTO_JSON_DIALECT},
    project::{ProjectConfig, ProjectRunnerConfig},
    sessions::{
        SessionMeta, discover_claude_sessions_in, discover_generic_state_sessions,
        discover_opencode_sessions,
    },
};

bitflags::bitflags! {
    /// Capabilities advertised by an execution harness.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct HarnessCapabilities: u32 {
        /// Harness supports external lifecycle hooks (`PreToolUse`, `Stop`, `SessionStart`).
        const NATIVE_HOOKS   = 1 << 0;
        /// Harness persists and can resume prior conversation sessions.
        const SESSION_RESUME = 1 << 1;
        /// Harness supports headless / non-interactive CLI spawning.
        const HEADLESS_SPAWN = 1 << 2;
        /// Harness supports workspace MCP server registration.
        const NATIVE_MCP     = 1 << 3;
    }
}

/// Plugin bundle directory name used under every harness plugin root.
pub const BUNDLE_NAME: &str = "torqkage";

/// Workspace-relative directory containing the bundled plugin assets.
#[must_use]
pub fn local_plugin_rel_dir() -> PathBuf {
    PathBuf::from("plugins").join(BUNDLE_NAME)
}

/// Infers an execution harness identifier from environment variables or
/// harness-specific workspace directories (`.claude`, `.opencode`, `.gemini`,
/// or `.cursor/mcp.json`).
#[must_use]
pub fn detect_workspace_harness(workspace_root: &Path) -> Option<&'static str> {
    if std::env::var_os("CLAUDE_PROJECT_DIR").is_some() || workspace_root.join(".claude").is_dir() {
        return Some("claude");
    }
    if workspace_root.join(".opencode").is_dir() {
        return Some("opencode");
    }
    if workspace_root.join(".gemini").is_dir() {
        return Some("agy");
    }
    if workspace_root.join(".cursor").join("mcp.json").is_file() {
        return Some("cursor");
    }
    None
}

/// Unified capability-driven contract for an agent execution harness.
pub trait Harness: Send + Sync + std::fmt::Debug {
    /// Canonical lowercase identifier (`agy`, `opencode`, `claude`, `cursor`,
    /// or the custom slug).
    fn id(&self) -> &str;

    /// Human-readable display label shown in status diagnostics.
    fn display_name(&self) -> &str {
        self.id()
    }

    /// Advertised capability bitset controlling hooks, session discovery,
    /// spawning, and config injection.
    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::HEADLESS_SPAWN | HarnessCapabilities::NATIVE_MCP
    }

    /// Default executable name.
    fn default_binary(&self) -> String {
        self.id().to_owned()
    }

    /// Default CLI flag used to select an agent persona.
    fn agent_flag(&self) -> Option<&'static str> {
        Some("--agent")
    }

    /// Default CLI flag used to enable the harness' own sandbox.
    fn sandbox_flag(&self) -> Option<&'static str> {
        None
    }

    /// Default CLI flag used to resume a prior session.
    fn resume_flag(&self) -> Option<&'static str> {
        Some("--resume")
    }

    /// Harness-specific workspace directory name (e.g. `.claude`).
    fn workspace_dir(&self) -> Option<String>;

    /// User-global plugin root that contains the `torqkage` bundle directory.
    fn plugin_root(&self, home: &Path) -> Option<PathBuf>;

    /// Workspace-relative directories searched for agent manifests.
    fn workspace_agent_dirs(&self, workspace_root: &Path) -> Vec<PathBuf> {
        self.workspace_dir().map_or_default(|d| {
            vec![
                workspace_root.join(&d).join("agents"),
                workspace_root.join(d),
            ]
        })
    }

    /// User-global directories searched for agent manifests.
    fn home_agent_dirs(&self, home: &Path) -> Vec<PathBuf>;

    /// Environment variables through which this harness exposes the active
    /// agent name.
    fn agent_env_vars(&self) -> &'static [&'static str] {
        &[]
    }

    /// Environment variables set when the harness already runs inside its own
    /// sandbox.
    fn outer_sandbox_env_vars(&self) -> &'static [&'static str] {
        &[]
    }

    /// Path fragments identifying a plugin installation directory of this
    /// harness.
    fn plugin_path_markers(&self) -> Vec<String> {
        Vec::new()
    }

    /// Optional CLI command (argv) to validate a plugin bundle directory for
    /// this harness.
    fn validate_plugin_command(&self, _bundle_dir: &Path) -> Option<Vec<String>> {
        None
    }

    /// Whether agent manifests should emit Antigravity-specific YAML
    /// frontmatter keys (`permissionMode`, `commandExecutionPolicy`,
    /// `mainAgent`, `subagent`, `inheritMcp`, inline `mcpServers`).
    fn uses_agy_frontmatter(&self) -> bool {
        false
    }

    /// Whether this harness provides native lifecycle hook interception
    /// (`PreToolUse`, `Stop`, `SessionStart`). Harnesses returning `false` rely
    /// on `TorqKage`'s universal in-band MCP seatbelt.
    fn supports_hooks(&self) -> bool {
        self.capabilities()
            .contains(HarnessCapabilities::NATIVE_HOOKS)
    }

    /// Hook wire-protocol dialect used for parsing `stdin` and formatting
    /// `stdout` payloads.
    fn hook_dialect(&self) -> &'static dyn HookDialect {
        &PROTO_JSON_DIALECT
    }

    /// Discovers prior sessions for `agent_name` in `workspace_root`.
    ///
    /// # Errors
    /// Returns [`TkgError`] if backend session lookup encounters an
    /// unrecoverable failure.
    fn discover_sessions(
        &self,
        agent_name: &str,
        workspace_root: &Path,
    ) -> Result<Vec<SessionMeta>, TkgError> {
        Ok(discover_generic_state_sessions(agent_name, workspace_root))
    }

    /// Convenience helper returning the newest resumable session for
    /// `agent_name` in `workspace_root`.
    fn latest_session(&self, agent_name: &str, workspace_root: &Path) -> Option<SessionMeta> {
        self.discover_sessions(agent_name, workspace_root)
            .ok()
            .and_then(|sessions| sessions.into_iter().next())
    }

    /// Returns `(config_or_bundle_path, is_installed, is_symlink)` for TUI/CLI
    /// installation diagnostics.
    fn config_status(
        &self,
        workspace_root: &Path,
        _runner: &ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool) {
        let p = workspace_root.join(".tkg/state/mcp_servers.json");
        let exists = p.is_file();
        (p, exists, false)
    }

    /// Installs or injects MCP server and hook configuration for this harness.
    ///
    /// # Errors
    /// Returns [`TkgError`] if configuration files cannot be parsed or written.
    fn inject_configs(
        &self,
        workspace_root: &Path,
        _runner: &ProjectRunnerConfig,
    ) -> Result<String, TkgError> {
        harness_install::register_custom_mcp_config(workspace_root).map_err(TkgError::Workspace)
    }

    /// Constructs the spawn [`Command`] for launching this harness with
    /// workspace environment, optional agent selection, prompt, resume ID, and
    /// sandbox flags.
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "Command template placeholders like {agent}, {conversation}, and {prompt} are not Rust format strings"
    )]
    fn build_spawn_cmd(
        &self,
        _workspace_root: &Path,
        runner: &ProjectRunnerConfig,
        agent_name: Option<&str>,
        prompt: Option<&str>,
        resume_session_id: Option<&str>,
        sandbox: bool,
        env_vars: &BTreeMap<String, String>,
    ) -> Command {
        let binary = runner.resolved_binary();
        let mut cmd = Command::new(&binary);

        for (k, v) in env_vars {
            cmd.env(k, v);
        }

        if let Some(custom_cmd) = &runner.command
            && let Some(sub_args) = custom_cmd.get(1..)
        {
            for arg in sub_args {
                if arg == "{agent}"
                    && let Some(agent) = agent_name
                {
                    cmd.arg(agent);
                } else if (arg == "{conversation}" || arg == "{session}")
                    && let Some(cid) = resume_session_id
                {
                    cmd.arg(cid);
                } else if arg == "{prompt}"
                    && let Some(p) = prompt
                {
                    cmd.arg(p);
                } else {
                    cmd.arg(arg);
                }
            }
        } else {
            if sandbox && let Some(flag) = runner.resolved_sandbox_flag() {
                cmd.arg(flag);
            }
            if let Some(conv_id) = resume_session_id {
                if let Some(flag) = runner.resolved_resume_flag() {
                    cmd.arg(flag).arg(conv_id);
                }
            } else if let Some(agent) = agent_name
                && let Some(flag) = runner.resolved_agent_flag()
            {
                cmd.arg(flag).arg(agent);
            }
            if let Some(p) = prompt {
                cmd.arg(p);
            }
        }

        for extra in &runner.extra_args {
            cmd.arg(extra);
        }

        cmd
    }
}

/// Google Antigravity (`agy`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Antigravity;

/// Alias for [`Antigravity`] matching the unified `*Harness` naming convention.
pub type AgyHarness = Antigravity;

impl Harness for Antigravity {
    fn id(&self) -> &'static str {
        "agy"
    }

    fn display_name(&self) -> &'static str {
        "Antigravity"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::NATIVE_HOOKS
            | HarnessCapabilities::SESSION_RESUME
            | HarnessCapabilities::HEADLESS_SPAWN
            | HarnessCapabilities::NATIVE_MCP
    }

    fn sandbox_flag(&self) -> Option<&'static str> {
        Some("--sandbox")
    }

    fn resume_flag(&self) -> Option<&'static str> {
        Some("--conversation")
    }

    fn workspace_dir(&self) -> Option<String> {
        Some(".gemini".to_owned())
    }

    fn plugin_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".gemini/config/plugins"))
    }

    fn workspace_agent_dirs(&self, workspace_root: &Path) -> Vec<PathBuf> {
        vec![workspace_root.join(".gemini/agents")]
    }

    fn home_agent_dirs(&self, home: &Path) -> Vec<PathBuf> {
        vec![
            home.join(".gemini/config/plugins")
                .join(BUNDLE_NAME)
                .join("agents"),
            home.join(".gemini/agents"),
        ]
    }

    fn agent_env_vars(&self) -> &'static [&'static str] {
        &["AGY_AGENT"]
    }

    fn outer_sandbox_env_vars(&self) -> &'static [&'static str] {
        &["AGY_OUTER_SANDBOX"]
    }

    fn plugin_path_markers(&self) -> Vec<String> {
        vec![
            "/.gemini/config/plugins/".to_owned(),
            "/.antigravity/plugins/".to_owned(),
            "/.gemini/plugins/".to_owned(),
        ]
    }

    fn validate_plugin_command(&self, bundle_dir: &Path) -> Option<Vec<String>> {
        Some(vec![
            "agy".to_owned(),
            "plugin".to_owned(),
            "validate".to_owned(),
            bundle_dir.to_string_lossy().into_owned(),
        ])
    }

    fn uses_agy_frontmatter(&self) -> bool {
        true
    }

    #[cfg(feature = "sqlite")]
    fn discover_sessions(
        &self,
        agent_name: &str,
        workspace_root: &Path,
    ) -> Result<Vec<SessionMeta>, TkgError> {
        let Some(home_path) = home_dir() else {
            return Ok(Vec::new());
        };
        let db_path = home_path.join(".gemini/antigravity-cli/conversation_summaries.db");
        let convs_dir = home_path.join(".gemini/antigravity-cli/conversations");
        let history_path = home_path.join(".gemini/antigravity-cli/history.jsonl");
        Ok(crate::sessions::query_session_from_sources(
            &db_path,
            &convs_dir,
            &history_path,
            agent_name,
            workspace_root,
        ))
    }

    #[cfg(not(feature = "sqlite"))]
    fn discover_sessions(
        &self,
        agent_name: &str,
        workspace_root: &Path,
    ) -> Result<Vec<SessionMeta>, TkgError> {
        Ok(discover_generic_state_sessions(agent_name, workspace_root))
    }

    fn config_status(
        &self,
        _workspace_root: &Path,
        runner: &ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool) {
        let p = plugin_install_dir(runner)
            .unwrap_or_else(|| PathBuf::from(".gemini/config/plugins/torqkage"));
        let exists = p.exists();
        let is_sym = p
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink());
        (p, exists, is_sym)
    }

    fn inject_configs(
        &self,
        workspace_root: &Path,
        runner: &ProjectRunnerConfig,
    ) -> Result<String, TkgError> {
        harness_install::install_plugin_interactive(workspace_root, runner)
            .map_err(TkgError::Workspace)
    }
}

/// `OpenCode`.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenCode;

/// Alias for [`OpenCode`] matching the unified `*Harness` naming convention.
pub type OpenCodeHarness = OpenCode;

impl Harness for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn display_name(&self) -> &'static str {
        "OpenCode"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::SESSION_RESUME
            | HarnessCapabilities::HEADLESS_SPAWN
            | HarnessCapabilities::NATIVE_MCP
    }

    fn resume_flag(&self) -> Option<&'static str> {
        Some("--session")
    }

    fn workspace_dir(&self) -> Option<String> {
        Some(".opencode".to_owned())
    }

    fn plugin_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".local/share/opencode/plugins"))
    }

    fn home_agent_dirs(&self, home: &Path) -> Vec<PathBuf> {
        vec![
            home.join(".local/share/opencode/agents"),
            home.join(".opencode/agents"),
        ]
    }

    fn plugin_path_markers(&self) -> Vec<String> {
        vec![
            "/.opencode/plugins/".to_owned(),
            "/.local/share/opencode/plugins/".to_owned(),
        ]
    }

    fn discover_sessions(
        &self,
        agent_name: &str,
        workspace_root: &Path,
    ) -> Result<Vec<SessionMeta>, TkgError> {
        Ok(discover_opencode_sessions(agent_name, workspace_root))
    }

    fn config_status(
        &self,
        workspace_root: &Path,
        _runner: &ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool) {
        let p = workspace_root.join("opencode.json");
        let exists = p.is_file();
        (p, exists, false)
    }

    fn inject_configs(
        &self,
        workspace_root: &Path,
        _runner: &ProjectRunnerConfig,
    ) -> Result<String, TkgError> {
        harness_install::register_opencode_config(workspace_root).map_err(TkgError::Workspace)
    }
}

/// Claude Code.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCode;

/// Alias for [`ClaudeCode`] matching the unified `*Harness` naming convention.
pub type ClaudeHarness = ClaudeCode;

impl Harness for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::NATIVE_HOOKS
            | HarnessCapabilities::SESSION_RESUME
            | HarnessCapabilities::HEADLESS_SPAWN
            | HarnessCapabilities::NATIVE_MCP
    }

    fn workspace_dir(&self) -> Option<String> {
        Some(".claude".to_owned())
    }

    fn plugin_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".claude/plugins"))
    }

    fn home_agent_dirs(&self, home: &Path) -> Vec<PathBuf> {
        vec![home.join(".claude/agents")]
    }

    fn plugin_path_markers(&self) -> Vec<String> {
        vec!["/.claude/plugins/".to_owned()]
    }

    fn hook_dialect(&self) -> &'static dyn HookDialect {
        &CLAUDE_CODE_DIALECT
    }

    fn discover_sessions(
        &self,
        agent_name: &str,
        workspace_root: &Path,
    ) -> Result<Vec<SessionMeta>, TkgError> {
        if let Some(home_path) = home_dir() {
            let projects_dir = home_path.join(".claude").join("projects");
            let sessions = discover_claude_sessions_in(&projects_dir, agent_name, workspace_root);
            if !sessions.is_empty() {
                return Ok(sessions);
            }
        }
        Ok(discover_generic_state_sessions(agent_name, workspace_root))
    }

    fn config_status(
        &self,
        workspace_root: &Path,
        _runner: &ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool) {
        let mcp_path = workspace_root.join(".mcp.json");
        let settings_path = workspace_root.join(".claude/settings.json");
        let exists = mcp_path.is_file() && settings_path.is_file();
        (mcp_path, exists, false)
    }

    fn inject_configs(
        &self,
        workspace_root: &Path,
        _runner: &ProjectRunnerConfig,
    ) -> Result<String, TkgError> {
        harness_install::register_claude_config(workspace_root).map_err(TkgError::Workspace)
    }
}

/// Cursor IDE / CLI (MCP-native; uses in-band MCP circuit breaker seatbelt).
#[derive(Debug, Default, Clone, Copy)]
pub struct Cursor;

/// Alias for [`Cursor`] matching the unified `*Harness` naming convention.
pub type CursorHarness = Cursor;

impl Harness for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn display_name(&self) -> &'static str {
        "Cursor"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::NATIVE_MCP
    }

    fn workspace_dir(&self) -> Option<String> {
        Some(".cursor".to_owned())
    }

    fn plugin_root(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".cursor/plugins"))
    }

    fn home_agent_dirs(&self, home: &Path) -> Vec<PathBuf> {
        vec![home.join(".cursor/agents")]
    }

    fn config_status(
        &self,
        workspace_root: &Path,
        _runner: &ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool) {
        let p = workspace_root.join(".cursor/mcp.json");
        let exists = p.is_file();
        (p, exists, false)
    }

    fn inject_configs(
        &self,
        workspace_root: &Path,
        _runner: &ProjectRunnerConfig,
    ) -> Result<String, TkgError> {
        harness_install::register_cursor_config(workspace_root).map_err(TkgError::Workspace)
    }
}

/// Any other CLI harness, addressed purely by convention (`.<slug>/agents`,
/// `~/.<slug>/plugins`).
#[derive(Debug, Clone)]
pub struct Generic {
    slug: String,
}

impl Generic {
    /// Builds a generic harness from a user-provided name (sanitized to a
    /// filesystem-safe slug).
    #[must_use]
    pub fn new(name: &str) -> Self {
        let slug: String = name
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        Self {
            slug: if slug.is_empty() {
                "custom".to_owned()
            } else {
                slug
            },
        }
    }

    fn is_placeholder(&self) -> bool {
        self.slug == "custom"
    }
}

impl Harness for Generic {
    fn id(&self) -> &str {
        &self.slug
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::HEADLESS_SPAWN | HarnessCapabilities::NATIVE_MCP
    }

    fn workspace_dir(&self) -> Option<String> {
        (!self.is_placeholder()).then(|| format!(".{}", self.slug))
    }

    fn plugin_root(&self, home: &Path) -> Option<PathBuf> {
        Some(if self.is_placeholder() {
            home.join(".config/tkg/plugins")
        } else {
            home.join(format!(".{}/plugins", self.slug))
        })
    }

    fn home_agent_dirs(&self, home: &Path) -> Vec<PathBuf> {
        if self.is_placeholder() {
            Vec::new()
        } else {
            vec![home.join(format!(".{}/agents", self.slug))]
        }
    }
}

/// Resolves a harness implementation from its configured name (`[runner]
/// harness = "..."`).
///
/// # Adding a New Harness
/// 1. Implement [`Harness`] for the new harness struct in this module.
/// 2. Add a match arm here and in [`builtin_harnesses`].
#[must_use]
pub fn resolve(name: &str) -> Box<dyn Harness> {
    match name.trim().to_ascii_lowercase().as_str() {
        "agy" | "antigravity" => Box::new(Antigravity),
        "opencode" => Box::new(OpenCode),
        "claude" | "claude-code" => Box::new(ClaudeCode),
        "cursor" => Box::new(Cursor),
        other => Box::new(Generic::new(other)),
    }
}

/// Normalizes a free-form MCP `clientInfo.name` or process cmdline token into
/// the matching [`Harness`] implementation.
#[must_use]
pub fn resolve_client_name(raw_name: &str) -> Box<dyn Harness> {
    let lower = raw_name.trim().to_ascii_lowercase();
    if lower.contains("claude") {
        Box::new(ClaudeCode)
    } else if lower.contains("antigravity") || lower == "agy" || lower.starts_with("agy-") {
        Box::new(Antigravity)
    } else if lower.contains("opencode") {
        Box::new(OpenCode)
    } else if lower.contains("cursor") {
        Box::new(Cursor)
    } else if lower.contains("windsurf") {
        Box::new(Generic::new("windsurf"))
    } else {
        resolve(raw_name)
    }
}

/// Best-effort detection of the parent host harness via `/proc/<ppid>/cmdline`
/// on Linux.
#[cfg(target_os = "linux")]
#[must_use]
pub fn detect_parent_process_harness() -> Option<String> {
    let ppid = std::os::unix::process::parent_id();
    let bytes = std::fs::read(format!("/proc/{ppid}/cmdline")).ok()?;
    let cmdline = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    for needle in [
        "claude",
        "cursor",
        "opencode",
        "windsurf",
        "antigravity",
        "agy",
    ] {
        if cmdline.contains(needle) {
            return Some(resolve_client_name(needle).id().to_owned());
        }
    }
    None
}

/// Non-Linux stub returning `None`.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn detect_parent_process_harness() -> Option<String> {
    None
}

/// All built-in harnesses (used for cross-harness fallbacks and marker
/// detection).
#[must_use]
pub fn builtin_harnesses() -> Vec<Box<dyn Harness>> {
    vec![
        Box::new(Antigravity),
        Box::new(OpenCode),
        Box::new(ClaudeCode),
        Box::new(Cursor),
    ]
}

/// Returns the user-global install directory of the plugin bundle for the
/// configured harness.
#[must_use]
pub fn plugin_install_dir(runner: &ProjectRunnerConfig) -> Option<PathBuf> {
    let home = home_dir()?;
    runner
        .harness()
        .plugin_root(&home)
        .map(|root| root.join(BUNDLE_NAME))
}

/// Ordered candidate paths for a declarative config file (highest priority
/// first).
///
/// 1. `<root>/.tkg/<name>`
/// 2. `<root>/<name>`
/// 3. `<root>/plugins/torqkage/<name>`
/// 4. `<root>/<harness dir>/<name>`
/// 5. `~/.tkg/<name>` and `~/.config/tkg/<name>`
/// 6. configured harness plugin bundle, then every built-in harness bundle
#[must_use]
pub fn config_file_candidates(
    workspace_root: &Path,
    runner: &ProjectRunnerConfig,
    name: &str,
) -> Vec<PathBuf> {
    let harness = runner.harness();
    let mut out = vec![
        workspace_root.join(".tkg").join(name),
        workspace_root.join(name),
        workspace_root.join("plugins").join(BUNDLE_NAME).join(name),
    ];
    if let Some(dir) = harness.workspace_dir() {
        out.push(workspace_root.join(dir).join(name));
    }

    if let Some(home) = home_dir() {
        out.push(home.join(".tkg").join(name));
        out.push(home.join(".config/tkg").join(name));

        let builtins = builtin_harnesses();
        let roots = std::iter::once(harness.plugin_root(&home))
            .chain(builtins.iter().map(|h| h.plugin_root(&home)))
            .flatten();
        for root in roots {
            let p = root.join(BUNDLE_NAME).join(name);
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Loads the effective runner configuration of a workspace.
#[must_use]
pub fn runner_for(workspace_root: &Path) -> ProjectRunnerConfig {
    ProjectConfig::detect_or_load(workspace_root).resolved_runner()
}

/// Finds the first existing candidate for a declarative config file.
#[must_use]
pub fn find_config_file(workspace_root: &Path, name: &str) -> Option<PathBuf> {
    let runner = runner_for(workspace_root);
    config_file_candidates(workspace_root, &runner, name)
        .into_iter()
        .find(|p| p.is_file())
}

/// Finds the effective `tools.toml`.
#[must_use]
pub fn find_tools_toml(workspace_root: &Path) -> Option<PathBuf> {
    find_config_file(workspace_root, "tools.toml")
}

/// Finds the effective `hooks.toml`.
#[must_use]
pub fn find_hooks_toml(workspace_root: &Path) -> Option<PathBuf> {
    find_config_file(workspace_root, "hooks.toml")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner(h: &str) -> ProjectRunnerConfig {
        ProjectRunnerConfig {
            harness: h.to_owned(),
            ..ProjectRunnerConfig::default()
        }
    }

    #[test]
    fn test_resolve_and_defaults() {
        assert_eq!(resolve("Antigravity").id(), "agy");
        assert_eq!(resolve("claude-code").id(), "claude");
        assert_eq!(resolve("cursor").id(), "cursor");
        assert_eq!(resolve("cursor").default_binary(), "cursor");
        assert_eq!(resolve("agy").sandbox_flag(), Some("--sandbox"));
        assert_eq!(resolve("opencode").resume_flag(), Some("--session"));
        assert_eq!(resolve("whatever").resume_flag(), Some("--resume"));
    }

    #[test]
    fn test_workspace_harness_dir() {
        assert_eq!(
            runner("claude").harness().workspace_dir().as_deref(),
            Some(".claude")
        );
        assert_eq!(
            runner("agy").harness().workspace_dir().as_deref(),
            Some(".gemini")
        );
        assert_eq!(
            runner("cursor").harness().workspace_dir().as_deref(),
            Some(".cursor")
        );
        assert_eq!(runner("custom").harness().workspace_dir(), None);
    }

    #[test]
    fn test_candidates_priority_order() {
        let root = Path::new("/ws");
        let c = config_file_candidates(root, &runner("opencode"), "tools.toml");
        let expected = [
            "/ws/.tkg/tools.toml",
            "/ws/tools.toml",
            "/ws/plugins/torqkage/tools.toml",
            "/ws/.opencode/tools.toml",
        ];
        for (got, want) in c.iter().zip(expected) {
            assert_eq!(got, &PathBuf::from(want));
        }
    }

    #[test]
    fn test_detect_workspace_harness() {
        let base = std::env::temp_dir().join(format!(
            "tkg_test_detect_harness_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let claude_ws = base.join("claude_ws");
        std::fs::create_dir_all(claude_ws.join(".claude")).ok();
        assert_eq!(detect_workspace_harness(&claude_ws), Some("claude"));

        let opencode_ws = base.join("opencode_ws");
        std::fs::create_dir_all(opencode_ws.join(".opencode")).ok();
        assert_eq!(detect_workspace_harness(&opencode_ws), Some("opencode"));

        let empty_ws = base.join("empty_ws");
        std::fs::create_dir_all(&empty_ws).ok();
        assert_eq!(detect_workspace_harness(&empty_ws), None);

        _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_resolve_client_name_and_hook_support() {
        let cursor = resolve_client_name("Cursor");
        assert_eq!(cursor.id(), "cursor");
        assert!(!cursor.supports_hooks());

        let claude = resolve_client_name("Claude Code");
        assert_eq!(claude.id(), "claude");
        assert!(claude.supports_hooks());

        let agy = resolve_client_name("antigravity-cli");
        assert_eq!(agy.id(), "agy");
        assert!(agy.supports_hooks());

        let unknown = resolve_client_name("zed-editor");
        assert_eq!(unknown.id(), "zed-editor");
        assert!(!unknown.supports_hooks());
    }

    #[test]
    fn test_unified_harness_capabilities_and_spawn_cmd() {
        assert!(
            ClaudeHarness::default()
                .capabilities()
                .contains(HarnessCapabilities::NATIVE_HOOKS | HarnessCapabilities::SESSION_RESUME)
        );
        assert!(
            AgyHarness::default()
                .capabilities()
                .contains(HarnessCapabilities::NATIVE_HOOKS | HarnessCapabilities::HEADLESS_SPAWN)
        );
        assert_eq!(
            CursorHarness::default().capabilities(),
            HarnessCapabilities::NATIVE_MCP
        );
        assert!(
            OpenCodeHarness::default()
                .capabilities()
                .contains(HarnessCapabilities::SESSION_RESUME)
        );

        let r = runner("opencode");
        let cmd = r.harness().build_spawn_cmd(
            Path::new("/ws"),
            &r,
            Some("coder"),
            Some("fix bugs"),
            Some("ses-99"),
            false,
            &BTreeMap::new(),
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["--session", "ses-99", "fix bugs"]);
    }
}
