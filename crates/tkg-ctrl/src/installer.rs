//! # installer
//!
//! In-deck plugin installation, directory copying, and host system prerequisite
//! diagnostics.

use std::{
    fmt::Write,
    path::{Path, PathBuf},
    process::Command,
};

use tkg_common::{DEFAULT_TOOL_TIMEOUT_SECS, FastHashSet, SubprocessSupervisor};

use crate::model::load_tools_from_file;

/// Prerequisite binary diagnostic report item.
#[derive(Debug, Clone)]
pub(crate) struct BinaryCheck {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) required: bool,
    pub(crate) installed: bool,
}

/// Full host system diagnostics report.
#[derive(Debug, Clone)]
pub(crate) struct DiagnosticsReport {
    pub(crate) plugin_installed: bool,
    pub(crate) plugin_path: PathBuf,
    pub(crate) is_symlink: bool,
    pub(crate) binary_checks: Vec<BinaryCheck>,
}

impl DiagnosticsReport {
    /// Formats the report into a human-readable multi-line summary string.
    #[must_use]
    pub(crate) fn summary(&self) -> String {
        let mut out = String::new();
        out.push_str("=== TORQKAGE SYSTEM & ENVIRONMENT DIAGNOSTICS ===\n\n");

        // Plugin status
        if self.plugin_installed {
            let link_note = if self.is_symlink {
                " (symlinked dev bundle - press 'i' to copy standalone bundle)"
            } else {
                " (standalone copy)"
            };
            _ = write!(
                out,
                "✔ Plugin Status: Installed at {}{link_note}\n\n",
                self.plugin_path.display()
            );
        } else {
            _ = write!(
                out,
                "✖ Plugin Status: NOT INSTALLED at {}\n  Press 'i' to copy and install now.\n\n",
                self.plugin_path.display()
            );
        }

        // Host binaries
        out.push_str("--- Host Binary Prerequisites ---\n");
        for check in &self.binary_checks {
            let icon = if check.installed {
                "✔"
            } else if check.required {
                "✖"
            } else {
                "○"
            };
            let req_tag = if check.required {
                "[Required]"
            } else {
                "[Optional]"
            };
            _ = writeln!(
                out,
                "{icon} {req_tag:<10} {:<16} — {}",
                check.name, check.description
            );
        }

        out
    }
}

/// Checks whether a binary is available in system `PATH`.
pub(crate) fn is_binary_available(name: &str) -> bool {
    which::which(name).is_ok()
}

/// Behavioural contract for installing, registering, and checking configuration
/// of an execution harness.
pub(crate) trait HarnessDriver: Send + Sync {
    /// Returns `(config_or_bundle_path, is_installed, is_symlink)` for
    /// diagnostics.
    fn config_status(
        &self,
        workspace_root: &Path,
        runner: &tkg_common::ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool);

    /// Installs or registers the harness configuration for the given workspace.
    fn register_config(
        &self,
        workspace_root: &Path,
        runner: &tkg_common::ProjectRunnerConfig,
    ) -> Result<String, String>;

    /// Creates the session discovery provider for this harness.
    fn session_provider(&self) -> Box<dyn crate::session::SessionProvider>;
}

/// Driver for Google Antigravity (`agy`).
pub(crate) struct AntigravityDriver;

impl HarnessDriver for AntigravityDriver {
    fn config_status(
        &self,
        _workspace_root: &Path,
        runner: &tkg_common::ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool) {
        let p = tkg_common::harness::plugin_install_dir(runner)
            .unwrap_or_else(|| PathBuf::from(".gemini/config/plugins/torqkage"));
        let exists = p.exists();
        let is_sym = p
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink());
        (p, exists, is_sym)
    }

    fn register_config(
        &self,
        workspace_root: &Path,
        runner: &tkg_common::ProjectRunnerConfig,
    ) -> Result<String, String> {
        install_plugin_interactive(workspace_root, runner)
    }

    fn session_provider(&self) -> Box<dyn crate::session::SessionProvider> {
        Box::new(crate::session::AntigravitySessionProvider)
    }
}

/// Driver for `OpenCode`.
pub(crate) struct OpenCodeDriver;

impl HarnessDriver for OpenCodeDriver {
    fn config_status(
        &self,
        workspace_root: &Path,
        _runner: &tkg_common::ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool) {
        let p = workspace_root.join("opencode.json");
        let exists = p.is_file();
        (p, exists, false)
    }

    fn register_config(
        &self,
        workspace_root: &Path,
        _runner: &tkg_common::ProjectRunnerConfig,
    ) -> Result<String, String> {
        register_opencode_config(workspace_root)
    }

    fn session_provider(&self) -> Box<dyn crate::session::SessionProvider> {
        Box::new(crate::session::OpenCodeSessionProvider)
    }
}

/// Driver for Claude Code.
pub(crate) struct ClaudeDriver;

impl HarnessDriver for ClaudeDriver {
    fn config_status(
        &self,
        workspace_root: &Path,
        _runner: &tkg_common::ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool) {
        let mcp_path = workspace_root.join(".mcp.json");
        let settings_path = workspace_root.join(".claude/settings.json");
        let exists = mcp_path.is_file() && settings_path.is_file();
        (mcp_path, exists, false)
    }

    fn register_config(
        &self,
        workspace_root: &Path,
        _runner: &tkg_common::ProjectRunnerConfig,
    ) -> Result<String, String> {
        register_claude_config(workspace_root)
    }

    fn session_provider(&self) -> Box<dyn crate::session::SessionProvider> {
        Box::new(crate::session::ClaudeSessionProvider)
    }
}

/// Driver for generic / custom CLI harnesses.
pub(crate) struct GenericDriver;

impl HarnessDriver for GenericDriver {
    fn config_status(
        &self,
        workspace_root: &Path,
        _runner: &tkg_common::ProjectRunnerConfig,
    ) -> (PathBuf, bool, bool) {
        let p = workspace_root.join(".tkg/state/mcp_servers.json");
        let exists = p.is_file();
        (p, exists, false)
    }

    fn register_config(
        &self,
        workspace_root: &Path,
        _runner: &tkg_common::ProjectRunnerConfig,
    ) -> Result<String, String> {
        register_custom_mcp_config(workspace_root)
    }

    fn session_provider(&self) -> Box<dyn crate::session::SessionProvider> {
        Box::new(crate::session::GenericSessionProvider)
    }
}

/// Resolves the [`HarnessDriver`] matching the configured runner.
#[must_use]
pub(crate) fn driver_for(runner: &tkg_common::ProjectRunnerConfig) -> Box<dyn HarnessDriver> {
    match runner.harness().id() {
        "agy" => Box::new(AntigravityDriver),
        "opencode" => Box::new(OpenCodeDriver),
        "claude" => Box::new(ClaudeDriver),
        _ => Box::new(GenericDriver),
    }
}

/// Runs a diagnostic scan of the host environment, plugin install path,
/// and dynamically validates all binaries declared across tools.toml and
/// project.toml.
#[must_use]
pub(crate) fn run_diagnostics(workspace_root: &Path) -> DiagnosticsReport {
    let project_cfg = tkg_common::project::ProjectConfig::detect_or_load(workspace_root);
    let runner = project_cfg.resolved_runner();
    let harness_binary = runner.resolved_binary();

    let (plugin_path, plugin_installed, is_symlink) =
        driver_for(&runner).config_status(workspace_root, &runner);

    let mut binary_checks = vec![
        BinaryCheck {
            name: harness_binary.clone(),
            description: format!("{} CLI harness for agent execution", runner.harness),
            required: true,
            installed: is_binary_available(&harness_binary),
        },
        BinaryCheck {
            name: "jj".to_owned(),
            description: "Jujutsu VCS for isolated subagent workspaces".to_owned(),
            required: false,
            installed: is_binary_available("jj"),
        },
        BinaryCheck {
            name: "git".to_owned(),
            description: "Git version control system".to_owned(),
            required: false,
            installed: is_binary_available("git"),
        },
        BinaryCheck {
            name: "bwrap".to_owned(),
            description: "Bubblewrap unprivileged Linux container sandbox".to_owned(),
            required: false,
            installed: is_binary_available("bwrap"),
        },
    ];

    let mut checked_names = FastHashSet::default();
    checked_names.insert(harness_binary);
    checked_names.insert("jj".to_owned());
    checked_names.insert("git".to_owned());
    checked_names.insert("bwrap".to_owned());

    // Dynamically check tools declared in tools.toml
    if let Some(tools_path) = tkg_common::harness::find_tools_toml(workspace_root)
        && let Ok(tools) = load_tools_from_file(&tools_path)
    {
        for tool in &tools {
            if let Some(cmd) = &tool.command
                && let Some(bin) = cmd.first()
                && !bin.is_empty()
                && checked_names.insert(bin.clone())
            {
                let installed = is_binary_available(bin);
                binary_checks.push(BinaryCheck {
                    name: bin.clone(),
                    description: format!("Declared tool '{}' in tools.toml", tool.name),
                    required: false,
                    installed,
                });
            }
        }
    }

    // Dynamically check quality gate commands configured in project.toml
    let project = tkg_common::project::ProjectConfig::detect_or_load(workspace_root);
    for (gate_name, gate) in &project.gates {
        if let Some(bin) = gate.command.first()
            && !bin.is_empty()
            && checked_names.insert(bin.clone())
        {
            let installed = is_binary_available(bin);
            binary_checks.push(BinaryCheck {
                name: bin.clone(),
                description: format!("Project quality gate '[gates.{gate_name}]'"),
                required: true,
                installed,
            });
        }
    }

    DiagnosticsReport {
        plugin_installed,
        plugin_path,
        is_symlink,
        binary_checks,
    }
}

/// Recursively copies all directories and files from `src` to `dst`.
pub(crate) fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_child = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &dest_child)?;
        } else if path.is_file() {
            std::fs::copy(&path, &dest_child)?;
        }
    }
    Ok(())
}

/// Ensures binary executables in `dest_bin` have executable bits set on Unix.
#[cfg(unix)]
fn ensure_binaries_executable(dest_bin: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if dest_bin.is_dir()
        && let Ok(entries) = std::fs::read_dir(dest_bin)
    {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                if let Err(err) = std::fs::set_permissions(entry.path(), perms) {
                    eprintln!(
                        "tkg: warning: failed to mark '{}' executable: {err}",
                        entry.path().display()
                    );
                }
            }
        }
    }
}

/// Discovers the local plugin source bundle either in
/// `<workspace>/plugins/torqkage` or directly in `<workspace>` if executed
/// inside an extracted plugin release bundle.
fn resolve_source_plugin(workspace_root: &Path) -> Result<PathBuf, String> {
    let local_plugin_dir = workspace_root.join(tkg_common::harness::local_plugin_rel_dir());
    if local_plugin_dir.join("plugin.json").is_file() {
        Ok(local_plugin_dir)
    } else if workspace_root.join("plugin.json").is_file() {
        Ok(workspace_root.to_path_buf())
    } else if local_plugin_dir.is_dir() {
        Ok(local_plugin_dir)
    } else {
        Err(format!(
            "Source plugin bundle not found at '{}' or '{}'",
            local_plugin_dir.display(),
            workspace_root.display()
        ))
    }
}

/// Installs the plugin bundle by copying all declarative files, agents, and
/// binaries into the user-global plugin directory of the configured harness
/// (see [`tkg_common::harness::plugin_install_dir`]).
///
/// # Errors
/// Returns an error string if source resolution, directory creation, or file
/// copying fails.
pub(crate) fn install_plugin_interactive(
    workspace_root: &Path,
    runner: &tkg_common::ProjectRunnerConfig,
) -> Result<String, String> {
    let local_plugin = resolve_source_plugin(workspace_root)?;

    let dest = tkg_common::harness::plugin_install_dir(runner)
        .ok_or_else(|| "HOME environment variable error: HOME is not set".to_owned())?;
    if let Some(target_dir) = dest.parent() {
        std::fs::create_dir_all(target_dir)
            .map_err(|e| format!("Failed to create {}: {e}", target_dir.display()))?;
    }

    // Remove existing target whether it's an old symlink or directory
    if let Ok(meta) = dest.symlink_metadata() {
        if meta.file_type().is_symlink() || !dest.is_dir() {
            std::fs::remove_file(&dest).map_err(|e| {
                format!(
                    "Failed to remove existing file or symlink {}: {e}",
                    dest.display()
                )
            })?;
        } else {
            std::fs::remove_dir_all(&dest).map_err(|e| {
                format!(
                    "Failed to remove old plugin directory {}: {e}",
                    dest.display()
                )
            })?;
        }
    }

    // Recursively copy all plugin files and subdirectories directly
    copy_dir_all(&local_plugin, &dest).map_err(|e| {
        format!(
            "Failed to copy plugin files from {} to {}: {e}",
            local_plugin.display(),
            dest.display()
        )
    })?;

    // Stage fresh compiled binaries if they exist in workspace target
    // directories
    let dest_bin = dest.join("bin");
    std::fs::create_dir_all(&dest_bin).map_err(|err| {
        format!(
            "Failed to create plugin binary directory '{}': {err}",
            dest_bin.display()
        )
    })?;
    for profile in ["release", "fast", "debug"] {
        let target_profile = workspace_root.join("target").join(profile);
        for bin_name in ["tkg-mcp", "tkg-hooks", "tkg"] {
            let candidate_bin = target_profile.join(bin_name);
            let target_file = dest_bin.join(bin_name);
            if candidate_bin.is_file() && !target_file.is_file() {
                std::fs::copy(&candidate_bin, &target_file).map_err(|err| {
                    format!(
                        "Failed to copy '{}' to '{}': {err}",
                        candidate_bin.display(),
                        target_file.display()
                    )
                })?;
            }
        }
    }

    #[cfg(unix)]
    ensure_binaries_executable(&dest_bin);

    // Run harness plugin validation command if supported by the harness
    let val_status = if let Some(val_argv) = runner.harness().validate_plugin_command(&dest)
        && let Some((prog, args)) = val_argv.split_first()
    {
        let mut validate_cmd = Command::new(prog);
        validate_cmd.args(args);
        validate_cmd.current_dir(workspace_root);
        match SubprocessSupervisor::run_supervised(validate_cmd, DEFAULT_TOOL_TIMEOUT_SECS) {
            Ok(out) if out.status.success() => "✔ Validation passed",
            Ok(_) => "○ Installed (plugin validation returned warnings)",
            Err(_) => "○ Installed (harness CLI not found in PATH for pre-validation)",
        }
    } else {
        "✔ Installed"
    };

    Ok(format!(
        "✔ Plugin bundle successfully copied to: {}\n  Source: {}\n  Status: {val_status}",
        dest.display(),
        local_plugin.display()
    ))
}

/// Registers or installs the `TorqKage` configuration for the selected
/// execution harness.
pub(crate) fn register_harness_config(
    workspace_root: &Path,
    runner: &tkg_common::ProjectRunnerConfig,
) -> Result<String, String> {
    driver_for(runner).register_config(workspace_root, runner)
}

/// Reads an existing JSON file as a JSON object, or initializes a new object
/// from `default_json` if the file does not exist or is empty. Fails closed if
/// an existing file has invalid JSON.
fn read_or_init_json_object(
    config_path: &Path,
    default_json: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if !config_path.is_file() {
        return Ok(default_json);
    }
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| format!("Failed to read {}: {e}", config_path.display()))?;
    if content.trim().is_empty() {
        return Ok(default_json);
    }
    let parsed: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse JSON in {}: {e}", config_path.display()))?;
    if !parsed.is_object() {
        return Err(format!(
            "Expected a top-level JSON object in {}",
            config_path.display()
        ));
    }
    Ok(parsed)
}

/// Serializes `value` as pretty JSON and writes it atomically to `config_path`.
fn write_pretty_json_atomic(config_path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let serialized = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize {}: {e}", config_path.display()))?;
    tkg_common::atomic_write(config_path, serialized.as_bytes())
        .map_err(|e| format!("Failed to write {}: {e}", config_path.display()))
}

/// Inserts or updates the `torqkage` server entry under `mcpServers` without
/// disturbing other keys.
fn upsert_torqkage_mcp_server(config: &mut serde_json::Value) {
    if let Some(obj) = config.as_object_mut() {
        let servers = obj
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(s_obj) = servers.as_object_mut() {
            s_obj.insert(
                "torqkage".to_owned(),
                serde_json::json!({
                    "command": "tkg-mcp",
                    "args": []
                }),
            );
        }
    }
}

/// Registers `tkg-mcp` in `opencode.json` for `OpenCode` integration.
pub(crate) fn register_opencode_config(workspace_root: &Path) -> Result<String, String> {
    let config_path = workspace_root.join("opencode.json");
    let mut config = read_or_init_json_object(
        &config_path,
        serde_json::json!({ "$schema": "https://opencode.ai/schema.json" }),
    )?;
    upsert_torqkage_mcp_server(&mut config);
    write_pretty_json_atomic(&config_path, &config)?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' in opencode.json at {}",
        config_path.display()
    ))
}

/// Upserts a Claude Code hook entry under `.claude/settings.json`'s
/// `hooks.<event>` array while preserving existing unrelated matchers and
/// commands.
fn upsert_claude_hook_event(
    hooks_obj: &mut serde_json::Map<String, serde_json::Value>,
    event: &str,
    matcher: Option<&str>,
    subcommand: &str,
    timeout_secs: u64,
) {
    let entry_list = hooks_obj
        .entry(event)
        .or_insert_with(|| serde_json::json!([]));
    let Some(arr) = entry_list.as_array_mut() else {
        return;
    };

    // Strip any previously installed TorqKage hook entries for this subcommand
    // to stay idempotent
    arr.retain(|item| {
        let has_tkg_cmd = item
            .get("hooks")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|hs| {
                hs.iter().any(|h| {
                    h.get("command")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|cmd| cmd.contains(&format!("tkg-hooks {subcommand}")))
                })
            });
        !has_tkg_cmd
    });

    let cmd_entry = serde_json::json!({
        "type": "command",
        "command": format!("tkg-hooks {subcommand}"),
        "timeout": timeout_secs
    });
    let wrapper = matcher.map_or_else(
        || serde_json::json!({ "hooks": [cmd_entry] }),
        |m| serde_json::json!({ "matcher": m, "hooks": [cmd_entry] }),
    );
    arr.push(wrapper);
}

/// Registers `tkg-mcp` in `.mcp.json` and `tkg-hooks` in
/// `.claude/settings.json` for Claude Code, preserving existing user settings
/// and unrelated hooks/servers.
pub(crate) fn register_claude_config(workspace_root: &Path) -> Result<String, String> {
    let mcp_path = workspace_root.join(".mcp.json");
    let mut mcp_config = read_or_init_json_object(&mcp_path, serde_json::json!({}))?;
    upsert_torqkage_mcp_server(&mut mcp_config);
    write_pretty_json_atomic(&mcp_path, &mcp_config)?;

    let claude_dir = workspace_root.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| format!("Failed to create {}: {e}", claude_dir.display()))?;
    let settings_path = claude_dir.join("settings.json");
    let mut settings = read_or_init_json_object(&settings_path, serde_json::json!({}))?;
    if let Some(root_obj) = settings.as_object_mut() {
        let hooks = root_obj
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(hooks_obj) = hooks.as_object_mut() {
            upsert_claude_hook_event(hooks_obj, "PreToolUse", Some("*"), "pre-tool-use", 15);
            upsert_claude_hook_event(hooks_obj, "SessionStart", None, "inject-wake", 5);
            upsert_claude_hook_event(hooks_obj, "Stop", None, "record-handoff", 10);
        }
    }
    write_pretty_json_atomic(&settings_path, &settings)?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' in {} and hooks in {}",
        mcp_path.display(),
        settings_path.display()
    ))
}

/// Generates a standalone MCP server configuration for custom harnesses.
pub(crate) fn register_custom_mcp_config(workspace_root: &Path) -> Result<String, String> {
    let state_dir = workspace_root.join(".tkg/state");
    std::fs::create_dir_all(&state_dir)
        .map_err(|e| format!("Failed to create {}: {e}", state_dir.display()))?;
    let config_path = state_dir.join("mcp_servers.json");
    let mut config = serde_json::json!({});
    upsert_torqkage_mcp_server(&mut config);
    write_pretty_json_atomic(&config_path, &config)?;

    Ok(format!(
        "✔ Generated MCP server config at {}",
        config_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_copy_dir_all_recursive() {
        let temp_base = std::env::temp_dir().join(format!("tkg_copy_test_{}", std::process::id()));
        let src = temp_base.join("src");
        let dst = temp_base.join("dst");
        _ = std::fs::remove_dir_all(&temp_base);

        _ = std::fs::create_dir_all(src.join("sub"));
        std::fs::write(src.join("a.txt"), "hello a").expect("write a");
        std::fs::write(src.join("sub/b.txt"), "hello b").expect("write b");

        copy_dir_all(&src, &dst).expect("copy_dir_all");

        assert!(dst.join("a.txt").is_file());
        assert!(dst.join("sub/b.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(dst.join("sub/b.txt")).unwrap(),
            "hello b"
        );

        _ = std::fs::remove_dir_all(&temp_base);
    }

    #[test]
    fn test_resolve_source_plugin() {
        let temp = std::env::temp_dir().join(format!("tkg_src_test_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&temp);

        // Case 1: unzipped plugin root containing plugin.json directly
        _ = std::fs::create_dir_all(&temp);
        std::fs::write(temp.join("plugin.json"), "{}").expect("write plugin.json");
        let resolved = resolve_source_plugin(&temp).expect("resolve directly");
        assert_eq!(resolved, temp);

        // Case 2: repo workspace with plugins/torqkage/plugin.json
        let temp_ws = std::env::temp_dir().join(format!("tkg_ws_test_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&temp_ws);
        _ = std::fs::create_dir_all(temp_ws.join("plugins/torqkage"));
        std::fs::write(temp_ws.join("plugins/torqkage/plugin.json"), "{}").expect("write");
        let resolved_ws = resolve_source_plugin(&temp_ws).expect("resolve in workspace");
        assert_eq!(resolved_ws, temp_ws.join("plugins/torqkage"));

        _ = std::fs::remove_dir_all(&temp);
        _ = std::fs::remove_dir_all(&temp_ws);
    }

    #[test]
    fn test_register_harness_configs() {
        let temp_ws = std::env::temp_dir().join(format!("tkg_reg_test_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&temp_ws);
        _ = std::fs::create_dir_all(&temp_ws);

        // 1. OpenCode
        let opencode_res = register_opencode_config(&temp_ws);
        opencode_res.unwrap();
        let oc_file = temp_ws.join("opencode.json");
        assert!(oc_file.is_file());
        let oc_content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&oc_file).unwrap()).unwrap();
        assert_eq!(
            oc_content
                .pointer("/mcpServers/torqkage/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-mcp")
        );

        // 2. Claude (.mcp.json + .claude/settings.json)
        let claude_res = register_claude_config(&temp_ws);
        claude_res.unwrap();
        let cl_file = temp_ws.join(".mcp.json");
        assert!(cl_file.is_file());
        let cl_content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cl_file).unwrap()).unwrap();
        assert_eq!(
            cl_content
                .pointer("/mcpServers/torqkage/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-mcp")
        );
        let settings_file = temp_ws.join(".claude/settings.json");
        assert!(settings_file.is_file());
        let settings_content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_file).unwrap()).unwrap();
        assert_eq!(
            settings_content
                .pointer("/hooks/PreToolUse/0/hooks/0/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-hooks pre-tool-use")
        );
        assert_eq!(
            settings_content
                .pointer("/hooks/SessionStart/0/hooks/0/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-hooks inject-wake")
        );
        assert_eq!(
            settings_content
                .pointer("/hooks/Stop/0/hooks/0/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-hooks record-handoff")
        );

        // 3. Custom MCP
        let custom_res = register_custom_mcp_config(&temp_ws);
        custom_res.unwrap();
        let custom_file = temp_ws.join(".tkg/state/mcp_servers.json");
        assert!(custom_file.is_file());
        let custom_content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&custom_file).unwrap()).unwrap();
        assert_eq!(
            custom_content
                .pointer("/mcpServers/torqkage/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-mcp")
        );

        _ = std::fs::remove_dir_all(&temp_ws);
    }

    #[test]
    fn test_register_claude_preserves_existing_keys_and_is_idempotent() {
        let temp_ws =
            std::env::temp_dir().join(format!("tkg_claude_merge_test_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&temp_ws);
        std::fs::create_dir_all(temp_ws.join(".claude")).unwrap();

        std::fs::write(
            temp_ws.join(".mcp.json"),
            r#"{"mcpServers":{"other":{"command":"other-mcp"}},"extraFlag":true}"#,
        )
        .unwrap();
        std::fs::write(
            temp_ws.join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Read"]},"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"my-audit"}]}]}}"#,
        )
        .unwrap();

        register_claude_config(&temp_ws).unwrap();
        // Run twice to ensure idempotency (no duplicate tkg-hooks entries)
        register_claude_config(&temp_ws).unwrap();

        let mcp_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(temp_ws.join(".mcp.json")).unwrap())
                .unwrap();
        assert_eq!(
            mcp_json
                .pointer("/mcpServers/other/command")
                .and_then(serde_json::Value::as_str),
            Some("other-mcp")
        );
        assert_eq!(
            mcp_json
                .pointer("/mcpServers/torqkage/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-mcp")
        );
        assert_eq!(
            mcp_json
                .get("extraFlag")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );

        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp_ws.join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            settings
                .pointer("/permissions/allow/0")
                .and_then(serde_json::Value::as_str),
            Some("Read")
        );
        let pre_tool = settings
            .pointer("/hooks/PreToolUse")
            .and_then(serde_json::Value::as_array)
            .unwrap();
        assert_eq!(
            pre_tool.len(),
            2,
            "must retain user hook + exactly one tkg-hooks entry: {pre_tool:?}"
        );

        // Corrupt JSON must fail closed rather than clobbering user config
        std::fs::write(temp_ws.join(".mcp.json"), "{ invalid json").unwrap();
        let err = register_claude_config(&temp_ws).unwrap_err();
        assert!(err.contains("Failed to parse JSON"));

        _ = std::fs::remove_dir_all(&temp_ws);
    }
}
