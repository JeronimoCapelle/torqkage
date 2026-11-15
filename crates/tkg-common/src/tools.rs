//! Declarative tool data types, safety policies, and inheritance merge engine.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Action type determining the execution engine for a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolAction {
    FileRead,
    FileWrite,
    MpatchEdit,
    FileSearch,
    #[default]
    StaticCommand,
    ParameterizedCommand,
    CliManual,
    ProjectState,
    WorkspaceEnsure,
    WorkspaceInspect,
    WorkspaceMerge,
    SetActiveAgent,
    RecordSessionHandoff,
    DetectWip,
    #[serde(untagged)]
    Unknown(String),
}

impl ToolAction {
    /// Returns the canonical `snake_case` identifier for this tool action.
    ///
    /// # Examples
    /// ```
    /// use tkg_common::tools::ToolAction;
    ///
    /// assert_eq!(ToolAction::FileRead.as_str(), "file_read");
    /// assert_eq!(ToolAction::MpatchEdit.as_str(), "mpatch_edit");
    /// ```
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::FileRead => "file_read",
            Self::FileWrite => "file_write",
            Self::MpatchEdit => "mpatch_edit",
            Self::FileSearch => "file_search",
            Self::StaticCommand => "static_command",
            Self::ParameterizedCommand => "parameterized_command",
            Self::CliManual => "cli_manual",
            Self::ProjectState => "project_state",
            Self::WorkspaceEnsure => "workspace_ensure",
            Self::WorkspaceInspect => "workspace_inspect",
            Self::WorkspaceMerge => "workspace_merge",
            Self::SetActiveAgent => "set_active_agent",
            Self::RecordSessionHandoff => "record_session_handoff",
            Self::DetectWip => "detect_wip",
            Self::Unknown(s) => s.as_str(),
        }
    }
}

/// Capture strategy for standard process streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputCapture {
    #[default]
    Stdout,
    Stderr,
    #[serde(alias = "merged")]
    Both,
    ExitCodeOnly,
    JsonSpans,
}

/// Declared argument for parameterized commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolArg {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub flag: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub switch: bool,
    #[serde(default, rename = "type", alias = "arg_type")]
    pub arg_type: Option<String>,
    #[serde(default, rename = "enum", alias = "values")]
    pub enum_values: Vec<String>,
    #[serde(default)]
    pub conflicts_with: Vec<String>,
}

/// Security policies governing tool mutability and permission gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolSafetyPolicy {
    #[serde(default)]
    pub mutating: bool,
    #[serde(default)]
    pub tracks_circuit_breaker: bool,
    #[serde(default)]
    pub require_permission_gate: bool,
}

/// Sandbox runtime backend declared via `[tools.<name>.sandbox]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxEngine {
    #[default]
    Bwrap,
    None,
}

const fn default_true() -> bool {
    true
}

/// Granular sandbox capabilities (`[tools.<name>.sandbox]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxProfile {
    #[serde(default)]
    pub engine: SandboxEngine,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default = "default_true")]
    pub unshare_pid: bool,
    #[serde(default)]
    pub read_only_binds: Vec<String>,
    #[serde(default)]
    pub writable_binds: Vec<String>,
    #[serde(default)]
    pub env_passthrough: Vec<String>,
}

impl Default for SandboxProfile {
    fn default() -> Self {
        Self {
            engine: SandboxEngine::Bwrap,
            allow_network: false,
            unshare_pid: true,
            read_only_binds: Vec::new(),
            writable_binds: Vec::new(),
            env_passthrough: Vec::new(),
        }
    }
}

/// Raw optional sandbox profile table before inheritance resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RawSandboxProfile {
    pub engine: Option<SandboxEngine>,
    pub enforce_sandbox: Option<bool>,
    pub allow_host_fallback: Option<bool>,
    pub allow_network: Option<bool>,
    pub unshare_pid: Option<bool>,
    pub read_only_binds: Option<Vec<String>>,
    pub writable_binds: Option<Vec<String>>,
    pub env_passthrough: Option<Vec<String>>,
}

/// Accepts either legacy boolean (`sandbox = true`) or a granular table
/// (`[tools.<name>.sandbox]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SandboxDecl {
    Bool(bool),
    Profile(Box<RawSandboxProfile>),
}

/// Sandbox and filesystem confinement boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolSandboxPolicy {
    #[serde(default)]
    pub enforce_sandbox: bool,
    /// When `true`, a tool with `enforce_sandbox` may run directly on the host
    /// if `bwrap` is unavailable or cannot create namespaces *and* an outer
    /// sandbox/container is detected. Defaults to `false` (fail closed).
    #[serde(default)]
    pub allow_host_fallback: bool,
    #[serde(default)]
    pub enforce_workspace_bounds: bool,
    #[serde(default)]
    pub ban_dangerous_files: bool,
    #[serde(default)]
    pub profile: SandboxProfile,
}

impl ToolSandboxPolicy {
    /// Returns `true` when sandboxing is enforced and `engine != "none"`.
    #[must_use]
    pub const fn effective_enforce_sandbox(&self) -> bool {
        self.enforce_sandbox && !matches!(self.profile.engine, SandboxEngine::None)
    }
}

/// Preconditions required before tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolPreconditions {
    #[serde(default)]
    pub require_gates: Vec<String>,
}

/// Output truncation and formatting directives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolFormatting {
    #[serde(default)]
    pub emit_truncation_notice: bool,
}

const fn default_timeout_seconds() -> u64 {
    15
}

/// Raw representation of a tool entry parsed from TOML before inheritance
/// resolution.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawTool {
    pub inherit: Option<String>,
    pub description: Option<String>,
    pub action: Option<ToolAction>,
    pub category: Option<String>,
    pub mutating: Option<bool>,
    pub tracks_circuit_breaker: Option<bool>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<ToolArg>>,
    pub output_capture: Option<OutputCapture>,
    pub allowed_directories: Option<Vec<String>>,
    pub enforce_dir: Option<String>,
    pub enforce_file_target: Option<String>,
    pub enforce_workspace_bounds: Option<bool>,
    pub ban_dangerous_files: Option<bool>,
    pub sanitize_input: Option<String>,
    pub truncate_lines: Option<usize>,
    pub truncate_bytes: Option<usize>,
    pub emit_truncation_notice: Option<bool>,
    pub timeout_seconds: Option<u64>,
    pub require_permission_gate: Option<bool>,
    pub enforce_sandbox: Option<bool>,
    pub allow_host_fallback: Option<bool>,
    pub sandbox: Option<SandboxDecl>,
    pub require_gates: Option<Vec<String>>,
    pub skip_gates: Option<Vec<String>>,
    pub error_fallback: Option<String>,
    pub empty_fallback: Option<String>,
    pub success_exit_codes: Option<Vec<i32>>,
    pub output_filter: Option<ToolOutputFilter>,
}

/// Declarative line filtering & diagnostic pruning rules
/// (`[tools.<name>.output_filter]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolOutputFilter {
    #[serde(default)]
    pub keep_regex: Vec<String>,
    #[serde(default)]
    pub drop_regex: Vec<String>,
    #[serde(default)]
    pub max_lines: Option<usize>,
}

impl ToolOutputFilter {
    /// Returns `true` when no keep, drop, or max-line pruning rules are
    /// configured.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.keep_regex.is_empty() && self.drop_regex.is_empty() && self.max_lines.is_none()
    }
}

/// Configuration container holding base tools, category defaults, and resolved
/// tools.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawToolsConfig {
    #[serde(default)]
    pub base_tools: BTreeMap<String, RawTool>,
    #[serde(default)]
    pub categories: BTreeMap<String, RawTool>,
    #[serde(default)]
    pub tools: BTreeMap<String, RawTool>,
}

/// Fully-resolved declarative tool definition ready for JSON Schema generation
/// and execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub action: ToolAction,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(flatten)]
    pub safety: ToolSafetyPolicy,
    #[serde(flatten)]
    pub sandbox: ToolSandboxPolicy,
    #[serde(flatten)]
    pub preconditions: ToolPreconditions,
    #[serde(flatten)]
    pub formatting: ToolFormatting,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<ToolArg>,
    #[serde(default)]
    pub output_capture: OutputCapture,
    #[serde(default)]
    pub allowed_directories: Vec<String>,
    #[serde(default)]
    pub enforce_dir: Option<String>,
    #[serde(default)]
    pub enforce_file_target: Option<String>,
    #[serde(default)]
    pub sanitize_input: Option<String>,
    #[serde(default)]
    pub truncate_lines: Option<usize>,
    #[serde(default)]
    pub truncate_bytes: Option<usize>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub error_fallback: Option<String>,
    #[serde(default)]
    pub empty_fallback: Option<String>,
    #[serde(default)]
    pub success_exit_codes: Vec<i32>,
    #[serde(default)]
    pub output_filter: Option<ToolOutputFilter>,
}

impl ToolDefinition {
    #[must_use]
    pub const fn mutating(&self) -> bool {
        self.safety.mutating
    }

    #[must_use]
    pub const fn tracks_circuit_breaker(&self) -> bool {
        self.safety.tracks_circuit_breaker
    }

    #[must_use]
    pub const fn require_permission_gate(&self) -> bool {
        self.safety.require_permission_gate
    }

    #[must_use]
    pub const fn enforce_sandbox(&self) -> bool {
        self.sandbox.effective_enforce_sandbox()
    }

    #[must_use]
    pub const fn allow_host_fallback(&self) -> bool {
        self.sandbox.allow_host_fallback
    }

    #[must_use]
    pub const fn enforce_workspace_bounds(&self) -> bool {
        self.sandbox.enforce_workspace_bounds
    }

    #[must_use]
    pub const fn ban_dangerous_files(&self) -> bool {
        self.sandbox.ban_dangerous_files
    }

    /// Resolves the list of quality gates required prior to tool execution.
    #[must_use]
    pub fn required_gates(&self) -> &[String] {
        &self.preconditions.require_gates
    }

    #[must_use]
    pub const fn emit_truncation_notice(&self) -> bool {
        self.formatting.emit_truncation_notice
    }

    /// Returns `true` when this tool serves project documentation (via explicit
    /// `[docs] tool_name` or a case-insensitive `"documentation"`/`"docs"`
    /// category).
    #[must_use]
    pub fn is_documentation_tool(&self, project: &crate::project::ProjectConfig) -> bool {
        project.docs.tool_name.as_deref() == Some(&self.name)
            || self.category.as_deref().is_some_and(|c| {
                let trimmed = c.trim();
                trimmed.eq_ignore_ascii_case("documentation")
                    || trimmed.eq_ignore_ascii_case("docs")
            })
    }

    #[must_use]
    pub fn primary_binary(&self) -> Option<&str> {
        self.command.first().map(String::as_str)
    }

    /// Evaluates whether this resolved tool requires the specified gate
    /// (case-insensitive).
    #[must_use]
    pub fn requires_gate(&self, gate_name: &str) -> bool {
        self.preconditions
            .require_gates
            .iter()
            .any(|g| g.eq_ignore_ascii_case(gate_name))
    }
}

fn resolve_fallback<T: Clone>(
    raw: Option<T>,
    base: Option<&RawTool>,
    category: Option<&RawTool>,
    selector: impl Fn(&RawTool) -> Option<T>,
) -> Option<T> {
    raw.or_else(|| selector(base?))
        .or_else(|| selector(category?))
}

fn resolve_safety(
    raw: &RawTool,
    base: Option<&RawTool>,
    category: Option<&RawTool>,
) -> ToolSafetyPolicy {
    ToolSafetyPolicy {
        mutating: resolve_fallback(raw.mutating, base, category, |t| t.mutating).unwrap_or(false),
        tracks_circuit_breaker: resolve_fallback(raw.tracks_circuit_breaker, base, category, |t| {
            t.tracks_circuit_breaker
        })
        .unwrap_or(false),
        require_permission_gate: resolve_fallback(
            raw.require_permission_gate,
            base,
            category,
            |t| t.require_permission_gate,
        )
        .unwrap_or(false),
    }
}

fn push_unique_strings(dest: &mut Vec<String>, items: &[String]) {
    for item in items {
        if !dest.contains(item) {
            dest.push(item.clone());
        }
    }
}

fn resolve_sandbox(
    raw: &RawTool,
    base: Option<&RawTool>,
    category: Option<&RawTool>,
) -> ToolSandboxPolicy {
    let mut enforce_sandbox = false;
    let mut allow_host_fallback = false;
    let mut profile = SandboxProfile::default();

    for layer in [category, base, Some(raw)].into_iter().flatten() {
        if let Some(ref decl) = layer.sandbox {
            match decl {
                SandboxDecl::Bool(enabled) => {
                    enforce_sandbox = *enabled;
                    profile.engine = if *enabled {
                        SandboxEngine::Bwrap
                    } else {
                        SandboxEngine::None
                    };
                },
                SandboxDecl::Profile(p) => {
                    if let Some(engine) = p.engine {
                        profile.engine = engine;
                    } else if profile.engine == SandboxEngine::None
                        && p.enforce_sandbox != Some(false)
                    {
                        profile.engine = SandboxEngine::Bwrap;
                    }

                    if let Some(explicit) = p.enforce_sandbox {
                        enforce_sandbox = explicit;
                    } else {
                        enforce_sandbox = profile.engine != SandboxEngine::None;
                    }

                    if let Some(fallback) = p.allow_host_fallback {
                        allow_host_fallback = fallback;
                    }
                    if let Some(net) = p.allow_network {
                        profile.allow_network = net;
                    }
                    if let Some(pid) = p.unshare_pid {
                        profile.unshare_pid = pid;
                    }
                    if let Some(ref binds) = p.read_only_binds {
                        push_unique_strings(&mut profile.read_only_binds, binds);
                    }
                    if let Some(ref binds) = p.writable_binds {
                        push_unique_strings(&mut profile.writable_binds, binds);
                    }
                    if let Some(ref vars) = p.env_passthrough {
                        push_unique_strings(&mut profile.env_passthrough, vars);
                    }
                },
            }
        }
        if let Some(explicit) = layer.enforce_sandbox {
            enforce_sandbox = explicit;
            if explicit && profile.engine == SandboxEngine::None {
                profile.engine = SandboxEngine::Bwrap;
            } else if !explicit {
                profile.engine = SandboxEngine::None;
            }
        }
        if let Some(fallback) = layer.allow_host_fallback {
            allow_host_fallback = fallback;
        }
    }

    ToolSandboxPolicy {
        enforce_sandbox,
        allow_host_fallback,
        enforce_workspace_bounds: resolve_fallback(
            raw.enforce_workspace_bounds,
            base,
            category,
            |t| t.enforce_workspace_bounds,
        )
        .unwrap_or(false),
        ban_dangerous_files: resolve_fallback(raw.ban_dangerous_files, base, category, |t| {
            t.ban_dangerous_files
        })
        .unwrap_or(false),
        profile,
    }
}

fn resolve_preconditions(
    raw: &RawTool,
    base: Option<&RawTool>,
    category: Option<&RawTool>,
) -> ToolPreconditions {
    let mut gates = Vec::new();
    let sources = [category, base, Some(raw)];
    for source in sources.into_iter().flatten() {
        if let Some(ref source_gates) = source.require_gates {
            for g in source_gates {
                if !gates
                    .iter()
                    .any(|existing: &String| existing.eq_ignore_ascii_case(g))
                {
                    gates.push(g.clone());
                }
            }
        }
    }

    let mut skip_set = Vec::new();
    for source in sources.into_iter().flatten() {
        if let Some(ref skips) = source.skip_gates {
            skip_set.extend(skips.clone());
        }
    }

    if skip_set
        .iter()
        .any(|s| s == "*" || s.eq_ignore_ascii_case("all"))
    {
        gates.clear();
    } else {
        gates.retain(|g| !skip_set.iter().any(|s| s.eq_ignore_ascii_case(g)));
    }

    ToolPreconditions {
        require_gates: gates,
    }
}

fn resolve_output_filter(
    raw: &RawTool,
    base: Option<&RawTool>,
    category: Option<&RawTool>,
) -> Option<ToolOutputFilter> {
    let mut merged = ToolOutputFilter::default();
    for source in [category, base, Some(raw)].into_iter().flatten() {
        if let Some(ref f) = source.output_filter {
            if !f.keep_regex.is_empty() {
                merged.keep_regex.clone_from(&f.keep_regex);
            }
            if !f.drop_regex.is_empty() {
                merged.drop_regex.clone_from(&f.drop_regex);
            }
            if f.max_lines.is_some() {
                merged.max_lines = f.max_lines;
            }
        }
    }
    (!merged.is_empty()).then_some(merged)
}

/// Merges a raw tool definition with its optional category defaults and base
/// definition.
#[must_use]
pub fn merge_tool_with_category(
    name: String,
    category: Option<&RawTool>,
    base: Option<&RawTool>,
    raw: &RawTool,
) -> ToolDefinition {
    let safety = resolve_safety(raw, base, category);
    let sandbox = resolve_sandbox(raw, base, category);
    let preconditions = resolve_preconditions(raw, base, category);
    let formatting = ToolFormatting {
        emit_truncation_notice: resolve_fallback(raw.emit_truncation_notice, base, category, |t| {
            t.emit_truncation_notice
        })
        .unwrap_or(false),
    };

    ToolDefinition {
        name,
        description: resolve_fallback(raw.description.clone(), base, category, |t| {
            t.description.clone()
        })
        .unwrap_or_default(),
        action: resolve_fallback(raw.action.clone(), base, category, |t| t.action.clone())
            .unwrap_or_default(),
        category: raw.category.clone().or_else(|| base?.category.clone()),
        safety,
        sandbox,
        preconditions,
        formatting,
        command: resolve_fallback(raw.command.clone(), base, category, |t| t.command.clone())
            .unwrap_or_default(),
        args: resolve_fallback(raw.args.clone(), base, category, |t| t.args.clone())
            .unwrap_or_default(),
        output_capture: resolve_fallback(raw.output_capture, base, category, |t| t.output_capture)
            .unwrap_or_default(),
        allowed_directories: resolve_fallback(
            raw.allowed_directories.clone(),
            base,
            category,
            |t| t.allowed_directories.clone(),
        )
        .unwrap_or_default(),
        enforce_dir: resolve_fallback(raw.enforce_dir.clone(), base, category, |t| {
            t.enforce_dir.clone()
        }),
        enforce_file_target: resolve_fallback(
            raw.enforce_file_target.clone(),
            base,
            category,
            |t| t.enforce_file_target.clone(),
        ),
        sanitize_input: resolve_fallback(raw.sanitize_input.clone(), base, category, |t| {
            t.sanitize_input.clone()
        }),
        truncate_lines: resolve_fallback(raw.truncate_lines, base, category, |t| t.truncate_lines),
        truncate_bytes: resolve_fallback(raw.truncate_bytes, base, category, |t| t.truncate_bytes),
        timeout_seconds: resolve_fallback(raw.timeout_seconds, base, category, |t| {
            t.timeout_seconds
        })
        .unwrap_or(15),
        error_fallback: resolve_fallback(raw.error_fallback.clone(), base, category, |t| {
            t.error_fallback.clone()
        }),
        empty_fallback: resolve_fallback(raw.empty_fallback.clone(), base, category, |t| {
            t.empty_fallback.clone()
        }),
        success_exit_codes: resolve_fallback(raw.success_exit_codes.clone(), base, category, |t| {
            t.success_exit_codes.clone()
        })
        .unwrap_or_default(),
        output_filter: resolve_output_filter(raw, base, category),
    }
}

/// Merges a raw tool definition with its optional base definition.
#[must_use]
pub fn merge_tool(name: String, base: Option<&RawTool>, raw: &RawTool) -> ToolDefinition {
    merge_tool_with_category(name, None, base, raw)
}

/// Resolves inheritance for all tools defined in `RawToolsConfig`, merging
/// category defaults, base tool templates, and concrete tool declarations.
#[must_use]
pub fn resolve(config: &RawToolsConfig) -> Vec<ToolDefinition> {
    let mut resolved = Vec::with_capacity(config.tools.len());
    for (name, raw_tool) in &config.tools {
        let base = raw_tool.inherit.as_deref().and_then(|key| {
            let clean = key.strip_prefix("base_tools.").unwrap_or(key);
            config.base_tools.get(clean)
        });
        let category_name = raw_tool
            .category
            .as_deref()
            .or_else(|| base?.category.as_deref())
            .map(|c| c.strip_prefix("categories.").unwrap_or(c));
        let category = category_name.and_then(|cat| config.categories.get(cat));
        resolved.push(merge_tool_with_category(
            name.clone(),
            category,
            base,
            raw_tool,
        ));
    }
    resolved
}

impl RawToolsConfig {
    /// Parses a `tools.toml` content string into a `RawToolsConfig`,
    /// attaching `path` context and line/column caret diagnostics on error.
    ///
    /// # Errors
    /// Returns [`crate::error::TkgError::ConfigParse`] if the content fails
    /// TOML deserialization.
    pub fn parse_with_path(
        path: Option<&std::path::Path>,
        content: &str,
    ) -> Result<Self, crate::error::TkgError> {
        toml::from_str(content)
            .map_err(|err| crate::error::TkgError::from_toml(path, content, &err))
    }

    /// Parses a `tools.toml` content string into a `RawToolsConfig`.
    ///
    /// # Errors
    /// Returns [`crate::error::TkgError::ConfigParse`] if the content fails
    /// TOML deserialization.
    pub fn parse(content: &str) -> Result<Self, crate::error::TkgError> {
        Self::parse_with_path(None, content)
    }

    /// Parses a `tools.toml` content string into a `RawToolsConfig`, returning
    /// an empty config on parse error.
    #[must_use]
    pub fn from_toml_str(content: &str) -> Self {
        Self::parse(content).unwrap_or_default()
    }

    /// Loads and merges `tools.toml` configurations across all candidate
    /// directories for `root`, failing closed if any existing file cannot be
    /// read or parsed.
    ///
    /// # Errors
    /// Returns [`crate::error::TkgError::Io`] or
    /// [`crate::error::TkgError::ConfigParse`] when an on-disk `tools.toml`
    /// fails to load or contains syntax errors.
    pub fn try_load_merged(root: &std::path::Path) -> crate::Result<Self> {
        let ws_root =
            crate::WorkspaceResolver::find_root(Some(root)).unwrap_or_else(|_| root.to_path_buf());
        let runner = crate::harness::runner_for(root);
        let mut candidates = crate::harness::config_file_candidates(root, &runner, "tools.toml");
        for extra in crate::harness::config_file_candidates(&ws_root, &runner, "tools.toml") {
            if !candidates.contains(&extra) {
                candidates.push(extra);
            }
        }

        let mut loader = crate::cascade::CascadedConfigLoader::new();
        // Load in reverse order so higher-priority candidates deep-merge over
        // lower-priority ones without clobbering unrelated sibling keys.
        for path in candidates.into_iter().rev() {
            if path.is_file() {
                let content = std::fs::read_to_string(&path)?;
                let _ = Self::parse_with_path(Some(&path), &content)?;
                loader.layer_str(Some(&path), &content)?;
            }
        }
        loader.deserialize()
    }

    /// Loads and merges `tools.toml` configurations across all candidate
    /// directories for `root`.
    #[must_use]
    pub fn load_merged(root: &std::path::Path) -> Self {
        Self::try_load_merged(root).unwrap_or_else(|err| {
            tracing::error!("Failed to load tools.toml: {err}");
            Self::default()
        })
    }

    /// Loads, merges, and resolves all tool definitions indexed by tool name,
    /// failing closed if any existing `tools.toml` cannot be read or parsed.
    ///
    /// # Errors
    /// Returns an error if reading or parsing any candidate `tools.toml` fails.
    pub fn try_load_resolved_map(
        root: &std::path::Path,
    ) -> crate::Result<crate::FastHashMap<String, ToolDefinition>> {
        let raw = Self::try_load_merged(root)?;
        Ok(resolve(&raw)
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect())
    }

    /// Loads, merges, and resolves all tool definitions indexed by tool name.
    #[must_use]
    pub fn load_resolved_map(root: &std::path::Path) -> crate::FastHashMap<String, ToolDefinition> {
        let raw = Self::load_merged(root);
        resolve(&raw)
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_tool_inheritance() {
        let base = RawTool {
            description: Some("Base description".to_owned()),
            mutating: Some(true),
            enforce_sandbox: Some(true),
            ..Default::default()
        };

        let raw = RawTool {
            description: Some("Override description".to_owned()),
            ..Default::default()
        };

        let merged = merge_tool("test_tool".to_owned(), Some(&base), &raw);
        assert_eq!(merged.name, "test_tool");
        assert_eq!(merged.description, "Override description");
        assert!(merged.mutating());
        assert!(merged.enforce_sandbox());
        assert_eq!(merged.required_gates(), <&[String]>::default());
    }

    #[test]
    fn test_merge_category_inheritance_and_gates() {
        let mut categories = BTreeMap::new();
        categories.insert(
            "compilation".to_owned(),
            RawTool {
                tracks_circuit_breaker: Some(true),
                require_gates: Some(vec!["vcs_conflicts".to_owned()]),
                timeout_seconds: Some(60),
                ..Default::default()
            },
        );

        let mut tools = BTreeMap::new();
        // Tool 1: inherits category gates and adds extra gate
        tools.insert(
            "cargo_clippy".to_owned(),
            RawTool {
                category: Some("compilation".to_owned()),
                require_gates: Some(vec!["compilation".to_owned()]),
                ..Default::default()
            },
        );
        // Tool 2: inherits category gates but opts out via skip_gates
        tools.insert(
            "run_special_check".to_owned(),
            RawTool {
                category: Some("compilation".to_owned()),
                skip_gates: Some(vec!["vcs_conflicts".to_owned()]),
                ..Default::default()
            },
        );

        let config = RawToolsConfig {
            base_tools: BTreeMap::new(),
            categories,
            tools,
        };

        let resolved = resolve(&config);
        assert_eq!(resolved.len(), 2);

        let clippy = resolved.iter().find(|t| t.name == "cargo_clippy").unwrap();
        assert!(clippy.tracks_circuit_breaker());
        assert_eq!(clippy.timeout_seconds, 60);
        // Has both category gate and tool-added gate
        assert_eq!(
            clippy.preconditions.require_gates,
            vec!["vcs_conflicts".to_owned(), "compilation".to_owned()]
        );

        let special = resolved
            .iter()
            .find(|t| t.name == "run_special_check")
            .unwrap();
        assert!(special.tracks_circuit_breaker());
        // vcs_conflicts was skipped!
        assert_eq!(special.preconditions.require_gates, Vec::<String>::new());
    }

    #[test]
    fn test_raw_tools_config_parse_syntax_error() {
        let malformed = "[tools.demo]\ndescription = \"ok\"\ncap = [[";
        let err = RawToolsConfig::parse(malformed).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("at line 3, column"));
        assert!(rendered.contains("3 | cap = [["));
        assert!(rendered.contains('^'));
        assert!(matches!(err, crate::error::TkgError::ConfigParse { .. }));
    }

    #[test]
    fn test_try_load_merged_fails_closed_on_malformed_tools_toml() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tkg_tools_fail_closed_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cfg_dir = temp_dir.join(".tkg");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let corrupt = cfg_dir.join("tools.toml");
        std::fs::write(&corrupt, "[tools.bad]\nbroken = [[\n").unwrap();

        let err = RawToolsConfig::try_load_merged(&temp_dir).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("tools.toml"));
        assert!(rendered.contains("at line 2, column"));

        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_output_filter_parsing_and_category_inheritance() {
        let toml = r#"
            [categories.command]
            [categories.command.output_filter]
            drop_regex = ["^\\s*Compiling "]
            max_lines = 40

            [tools.cargo_build]
            description = "Build workspace"
            category = "command"
            action = "static_command"
            command = ["cargo", "build"]

            [tools.cargo_build.output_filter]
            keep_regex = ["^error"]
            max_lines = 15
        "#;

        let raw = RawToolsConfig::parse(toml).unwrap();
        let resolved = resolve(&raw);
        let build = resolved.iter().find(|t| t.name == "cargo_build").unwrap();
        let filter = build
            .output_filter
            .as_ref()
            .expect("output_filter resolved");
        assert_eq!(filter.keep_regex, vec!["^error"]);
        assert_eq!(filter.drop_regex, vec!["^\\s*Compiling "]);
        assert_eq!(filter.max_lines, Some(15));
    }

    #[test]
    fn test_sandbox_profile_parsing_and_inheritance() {
        let toml = r#"
            [categories.security]
            [categories.security.sandbox]
            engine = "bwrap"
            allow_network = false
            unshare_pid = true
            read_only_binds = ["/usr", "/lib"]
            env_passthrough = ["RUST_LOG"]

            [tools.cargo_audit]
            description = "Audit Cargo dependencies"
            category = "security"
            action = "static_command"
            command = ["cargo", "audit"]

            [tools.cargo_audit.sandbox]
            allow_network = true
            read_only_binds = ["/lib", "/etc/ssl/certs"]
            writable_binds = ["{cwd}", "{home}/.cargo/registry"]
            env_passthrough = ["CARGO_HOME"]

            [tools.legacy_bool_on]
            description = "Legacy bool sandbox"
            action = "static_command"
            sandbox = true
            command = ["echo", "ok"]

            [tools.engine_none_override]
            description = "Opt-out of category sandbox"
            category = "security"
            action = "static_command"
            command = ["echo", "host"]

            [tools.engine_none_override.sandbox]
            engine = "none"
        "#;

        let raw = RawToolsConfig::parse(toml).unwrap();
        let resolved = resolve(&raw);

        let audit = resolved.iter().find(|t| t.name == "cargo_audit").unwrap();
        assert!(audit.enforce_sandbox());
        assert_eq!(audit.sandbox.profile.engine, SandboxEngine::Bwrap);
        assert!(audit.sandbox.profile.allow_network);
        assert!(audit.sandbox.profile.unshare_pid);
        assert_eq!(
            audit.sandbox.profile.read_only_binds,
            vec!["/usr", "/lib", "/etc/ssl/certs"]
        );
        assert_eq!(
            audit.sandbox.profile.writable_binds,
            vec!["{cwd}", "{home}/.cargo/registry"]
        );
        assert_eq!(
            audit.sandbox.profile.env_passthrough,
            vec!["RUST_LOG", "CARGO_HOME"]
        );

        let legacy = resolved
            .iter()
            .find(|t| t.name == "legacy_bool_on")
            .unwrap();
        assert!(legacy.enforce_sandbox());
        assert!(!legacy.sandbox.profile.allow_network);
        assert!(legacy.sandbox.profile.unshare_pid);

        let disabled = resolved
            .iter()
            .find(|t| t.name == "engine_none_override")
            .unwrap();
        assert!(!disabled.enforce_sandbox());
        assert_eq!(disabled.sandbox.profile.engine, SandboxEngine::None);
    }
}
