//! Data models, YAML frontmatter parser/serializer, and tool catalog
//! definitions.

use std::{
    fs,
    path::{Path, PathBuf},
};

use tkg_common::{
    AgentFrontmatter,
    tools::{
        RawToolsConfig, ToolDefinition, ToolFormatting, ToolPreconditions, ToolSafetyPolicy,
        ToolSandboxPolicy, resolve,
    },
};

/// Fallback category label when a tool omits `category` in `tools.toml`.
pub(crate) const DEFAULT_TOOL_CATEGORY: &str = "Tools";

/// In-memory representation of an MCP tool loaded from `tools.toml`.
#[derive(Debug, Clone)]
pub(crate) struct ToolItem {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) action: String,
    pub(crate) category: String,
    pub(crate) safety: ToolSafetyPolicy,
    pub(crate) sandbox: ToolSandboxPolicy,
    pub(crate) preconditions: ToolPreconditions,
    pub(crate) formatting: ToolFormatting,
    pub(crate) command: Option<Vec<String>>,
    pub(crate) allowed_directories: Option<Vec<String>>,
    pub(crate) timeout_seconds: Option<u64>,
    pub(crate) output_capture: Option<String>,
    pub(crate) modified: bool,
}

impl ToolItem {
    #[must_use]
    pub(crate) fn from_definition(def: ToolDefinition) -> Self {
        let category = def
            .category
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map_or_else(|| DEFAULT_TOOL_CATEGORY.to_owned(), ToOwned::to_owned);

        Self {
            name: def.name,
            description: def.description,
            action: def.action.as_str().to_owned(),
            category,
            safety: def.safety,
            sandbox: def.sandbox,
            preconditions: def.preconditions,
            formatting: def.formatting,
            command: if def.command.is_empty() {
                None
            } else {
                Some(def.command)
            },
            allowed_directories: if def.allowed_directories.is_empty() {
                None
            } else {
                Some(def.allowed_directories)
            },
            timeout_seconds: Some(def.timeout_seconds),
            output_capture: Some(format!("{:?}", def.output_capture).to_lowercase()),
            modified: false,
        }
    }

    #[must_use]
    pub(crate) const fn mutating(&self) -> bool {
        self.safety.mutating
    }

    #[must_use]
    pub(crate) const fn tracks_circuit_breaker(&self) -> bool {
        self.safety.tracks_circuit_breaker
    }

    #[must_use]
    pub(crate) const fn require_permission_gate(&self) -> bool {
        self.safety.require_permission_gate
    }

    #[must_use]
    pub(crate) const fn enforce_sandbox(&self) -> bool {
        self.sandbox.effective_enforce_sandbox()
    }

    #[must_use]
    pub(crate) const fn enforce_workspace_bounds(&self) -> bool {
        self.sandbox.enforce_workspace_bounds
    }

    #[must_use]
    pub(crate) const fn ban_dangerous_files(&self) -> bool {
        self.sandbox.ban_dangerous_files
    }

    #[must_use]
    pub(crate) fn required_gates(&self) -> &[String] {
        &self.preconditions.require_gates
    }

    #[must_use]
    pub(crate) const fn emit_truncation_notice(&self) -> bool {
        self.formatting.emit_truncation_notice
    }
}

/// Discovers the path to `tools.toml` across workspace overrides, plugin
/// bundles, and user home configurations.
#[must_use]
pub(crate) fn find_tools_config_path(workspace_root: &Path) -> Option<PathBuf> {
    tkg_common::harness::find_tools_toml(workspace_root)
}

/// Loads all tools from a `tools.toml` file path.
///
/// # Errors
/// Returns an error if reading or parsing the file fails.
pub(crate) fn load_tools_from_file(path: &Path) -> Result<Vec<ToolItem>, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let raw: RawToolsConfig =
        toml::from_str(&content).map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
    let defs = resolve(&raw);
    let mut items: Vec<ToolItem> = defs.into_iter().map(ToolItem::from_definition).collect();
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(items)
}

/// Updates or removes `require_permission_gate = true` for the specified tool
/// in a `tools.toml` file.
///
/// # Errors
/// Returns an error if reading, parsing, or writing the file fails.
pub(crate) fn update_tool_permission_gate_in_file(
    file_path: &Path,
    tool_name: &str,
    require_gate: bool,
) -> Result<(), String> {
    if !file_path.is_file() {
        return Ok(());
    }
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read {}: {e}", file_path.display()))?;

    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("Failed to parse {}: {e}", file_path.display()))?;

    if let Some(tools) = doc
        .get_mut("tools")
        .and_then(toml_edit::Item::as_table_like_mut)
        && let Some(tool_item) = tools
            .get_mut(tool_name)
            .and_then(toml_edit::Item::as_table_like_mut)
    {
        if require_gate {
            tool_item.insert("require_permission_gate", toml_edit::value(true));
        } else {
            tool_item.remove("require_permission_gate");
        }
    }

    fs::write(file_path, doc.to_string())
        .map_err(|e| format!("Failed to write {}: {e}", file_path.display()))?;

    Ok(())
}

/// Classification flags defining whether an agent is a root agent or a
/// subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AgentRoleFlags {
    pub(crate) main_agent: bool,
    pub(crate) subagent: bool,
}

/// Agent manifest parsed from an agent Markdown file with YAML frontmatter.
#[derive(Debug, Clone)]
pub(crate) struct AgentManifest {
    pub(crate) file_path: PathBuf,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) effort: String,
    pub(crate) permission_mode: String,
    pub(crate) command_execution_policy: String,
    pub(crate) role_flags: AgentRoleFlags,
    pub(crate) inherit_mcp: bool,
    pub(crate) native_tools: Vec<String>,
    pub(crate) mcp_tools: Vec<String>,
    pub(crate) allowed_subagents: Vec<String>,
    pub(crate) pre_launch_checks: Vec<String>,
    pub(crate) allowed_dirs: Vec<String>,
    pub(crate) disallowed_dirs: Vec<String>,
    pub(crate) allowed_files: Vec<String>,
    pub(crate) disallowed_files: Vec<String>,
    pub(crate) path_policies: Vec<tkg_common::PathPolicy>,
    pub(crate) directives: String,
    pub(crate) modified: bool,
}

impl AgentManifest {
    /// Constructs an `AgentManifest` from a file path and parsed
    /// `AgentFrontmatter`.
    #[must_use]
    pub(crate) fn from_frontmatter(file_path: PathBuf, mut parsed: AgentFrontmatter) -> Self {
        if parsed.name.is_empty()
            && let Some(stem) = file_path.file_stem().and_then(std::ffi::OsStr::to_str)
        {
            stem.clone_into(&mut parsed.name);
        }

        Self {
            file_path,
            name: parsed.name,
            description: parsed.description,
            effort: parsed.effort,
            permission_mode: parsed.permission_mode,
            command_execution_policy: parsed.command_execution_policy,
            role_flags: AgentRoleFlags {
                main_agent: parsed.main_agent,
                subagent: parsed.subagent,
            },
            inherit_mcp: parsed.inherit_mcp,
            native_tools: parsed.native_tools,
            mcp_tools: parsed.mcp_tools,
            allowed_subagents: parsed.allowed_subagents,
            pre_launch_checks: parsed.pre_launch_checks,
            allowed_dirs: parsed.allowed_dirs,
            disallowed_dirs: parsed.disallowed_dirs,
            allowed_files: parsed.allowed_files,
            disallowed_files: parsed.disallowed_files,
            path_policies: parsed.path_policies,
            directives: parsed.directives,
            modified: false,
        }
    }

    /// Converts this `AgentManifest` back into a canonical `AgentFrontmatter`.
    #[must_use]
    pub(crate) fn to_frontmatter(&self) -> AgentFrontmatter {
        AgentFrontmatter {
            name: self.name.clone(),
            description: self.description.clone(),
            effort: self.effort.clone(),
            permission_mode: self.permission_mode.clone(),
            command_execution_policy: self.command_execution_policy.clone(),
            main_agent: self.role_flags.main_agent,
            subagent: self.role_flags.subagent,
            inherit_mcp: self.inherit_mcp,
            allowed_subagents: self.allowed_subagents.clone(),
            pre_launch_checks: self.pre_launch_checks.clone(),
            native_tools: self.native_tools.clone(),
            mcp_tools: self.mcp_tools.clone(),
            allowed_dirs: self.allowed_dirs.clone(),
            disallowed_dirs: self.disallowed_dirs.clone(),
            allowed_files: self.allowed_files.clone(),
            disallowed_files: self.disallowed_files.clone(),
            path_policies: self.path_policies.clone(),
            directives: self.directives.clone(),
        }
    }

    /// Checks whether an MCP tool is allowed for this agent.
    #[must_use]
    pub(crate) fn allows_mcp_tool(&self, tool_name: &str) -> bool {
        self.mcp_tools.iter().any(|t| t == tool_name)
    }

    /// Toggles an MCP tool on or off in this agent's allowlist.
    pub(crate) fn toggle_mcp_tool(&mut self, tool_name: &str) {
        if let Some(pos) = self.mcp_tools.iter().position(|t| t == tool_name) {
            self.mcp_tools.remove(pos);
        } else {
            self.mcp_tools.push(tool_name.to_owned());
            self.mcp_tools.sort();
        }
        self.modified = true;
    }

    /// Cycles the agent's permission mode and command execution policy:
    /// Autonomous (acceptEdits / off) -> Supervised (acceptEdits / ask) ->
    /// Planning (plan / off).
    pub(crate) fn cycle_permission_policy(&mut self) -> &'static str {
        let desc = match (
            self.permission_mode.as_str(),
            self.command_execution_policy.as_str(),
        ) {
            ("acceptEdits", "off") => {
                "acceptEdits".clone_into(&mut self.permission_mode);
                "ask".clone_into(&mut self.command_execution_policy);
                "Supervised (ASK before commands)"
            },
            ("acceptEdits", "ask" | "prompt") => {
                "plan".clone_into(&mut self.permission_mode);
                "off".clone_into(&mut self.command_execution_policy);
                "Planning (Read-only PLAN mode)"
            },
            _ => {
                "acceptEdits".clone_into(&mut self.permission_mode);
                "off".clone_into(&mut self.command_execution_policy);
                "Autonomous (ALLOW all commands)"
            },
        };
        self.modified = true;
        desc
    }

    /// Serializes this agent manifest to YAML frontmatter + Markdown directives
    /// using the default (`Antigravity`) dialect.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn serialize(&self) -> String {
        self.serialize_for_harness(&tkg_common::harness::Antigravity)
    }

    /// Serializes this agent manifest to YAML frontmatter + Markdown directives
    /// tailored for `harness`.
    #[must_use]
    pub(crate) fn serialize_for_harness(&self, harness: &dyn tkg_common::Harness) -> String {
        self.to_frontmatter().serialize_for_harness(harness)
    }
}

/// Parses an agent Markdown content string.
#[cfg(test)]
pub(crate) fn parse_agent_markdown(
    file_path: PathBuf,
    content: &str,
) -> Result<AgentManifest, String> {
    let parsed = AgentFrontmatter::parse(content)?;
    Ok(AgentManifest::from_frontmatter(file_path, parsed))
}

/// Discovers and loads all agent manifests within the plugin or repository.
pub(crate) fn discover_agents(workspace_root: &Path) -> Vec<AgentManifest> {
    tkg_common::discover_agent_frontmatters(workspace_root)
        .into_iter()
        .map(|(path, fm)| AgentManifest::from_frontmatter(path, fm))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_declarative_tool_category_is_language_agnostic() {
        let make_tool = |name: &str, category: Option<&str>| {
            ToolItem::from_definition(ToolDefinition {
                name: name.to_owned(),
                category: category.map(ToOwned::to_owned),
                ..Default::default()
            })
        };

        assert_eq!(make_tool("pytest", Some("Test")).category, "Test");
        assert_eq!(make_tool("ruff", Some("Linter")).category, "Linter");
        assert_eq!(
            make_tool("golangci_lint", Some("  Audit  ")).category,
            "Audit"
        );
        assert_eq!(make_tool("mypy", Some("types")).category, "types");
        // Omitted or blank categories fall back cleanly to "Tools" without Rust heuristics
        assert_eq!(make_tool("cargo_clippy", None).category, "Tools");
        assert_eq!(make_tool("custom_tool", Some("   ")).category, "Tools");
    }

    #[test]
    fn test_parse_and_serialize_agent_markdown() {
        let sample = "\
---
name: test-agent
description: Test Agent for verification
effort: high
permissionMode: acceptEdits
commandExecutionPolicy: off
mainAgent: false
subagent: true
inheritMcp: false
allowed_subagents:
  - helper_agent
pre_launch_checks:
  - gate_a
  - gate_b
tools: []
mcp_tools:
  - tool_a
  - tool_b
mcpServers:
  - name: tkg-tools
    command: tkg-mcp
    args:
      - --agent
      - test-agent
---

# Instructions

You are a test agent. Always pass tests.
";
        let Ok(manifest) = parse_agent_markdown(PathBuf::from("test.md"), sample) else {
            return;
        };
        assert_eq!(manifest.name, "test-agent");
        assert_eq!(manifest.effort, "high");
        assert!(!manifest.inherit_mcp);
        assert_eq!(manifest.mcp_tools, vec!["tool_a", "tool_b"]);
        assert_eq!(manifest.allowed_subagents, vec!["helper_agent"]);
        assert_eq!(manifest.pre_launch_checks, vec!["gate_a", "gate_b"]);
        assert!(manifest.directives.contains("You are a test agent."));

        let serialized = manifest.serialize();
        assert!(serialized.contains("name: test-agent"));
        assert!(serialized.contains("pre_launch_checks:"));
        assert!(serialized.contains("- gate_a"));
        assert!(serialized.contains("mcp_tools:"));
        assert!(serialized.contains("- tool_a"));
        assert!(serialized.contains("inheritMcp: false"));
        assert!(serialized.contains("You are a test agent."));
    }

    #[test]
    fn test_update_tool_permission_gate_in_file() {
        let temp_dir = std::env::temp_dir().join(format!("tkg_test_gate_{}", std::process::id()));
        _ = fs::create_dir_all(&temp_dir);
        let toml_file = temp_dir.join("tools.toml");

        let initial = "# Initial tools configuration\n[tools.run_tests]\n# Important comment to preserve\ndescription = \"Run tests\"\ncommand = [\"cargo\", \"test\"]\n";
        fs::write(&toml_file, initial).unwrap();

        // 1. Enable gate
        update_tool_permission_gate_in_file(&toml_file, "run_tests", true).unwrap();
        let content_enabled = fs::read_to_string(&toml_file).unwrap();
        assert!(content_enabled.contains("require_permission_gate = true"));
        assert!(content_enabled.contains("# Important comment to preserve"));

        // 2. Disable gate
        update_tool_permission_gate_in_file(&toml_file, "run_tests", false).unwrap();
        let content_disabled = fs::read_to_string(&toml_file).unwrap();
        assert!(!content_disabled.contains("require_permission_gate = true"));
        assert!(content_disabled.contains("# Important comment to preserve"));

        _ = fs::remove_dir_all(&temp_dir);
    }
}
