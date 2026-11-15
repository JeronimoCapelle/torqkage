//! Harness configuration injection, atomic JSON merging, and Antigravity
//! plugin bundle staging shared across CLI and TUI drivers.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    harness::{local_plugin_rel_dir, plugin_install_dir},
    limits::DEFAULT_TOOL_TIMEOUT_SECS,
    project::ProjectRunnerConfig,
    state::atomic_write,
    supervisor::SubprocessSupervisor,
};

/// Recursively copies a directory tree from `src` to `dst`.
///
/// # Errors
/// Returns an I/O error if reading directories or copying files fails.
pub fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
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

#[cfg(unix)]
fn ensure_binaries_executable(dest_bin: &Path) -> Result<(), String> {
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
                std::fs::set_permissions(entry.path(), perms).map_err(|err| {
                    format!(
                        "Failed to mark '{}' executable: {err}",
                        entry.path().display()
                    )
                })?;
            }
        }
    }
    Ok(())
}

/// Discovers the local plugin source bundle either in
/// `<workspace>/plugins/torqkage` or directly in `<workspace>`.
///
/// # Errors
/// Returns an error message when no plugin bundle directory or `plugin.json`
/// can be found.
pub fn resolve_source_plugin(workspace_root: &Path) -> Result<PathBuf, String> {
    let local_plugin_dir = workspace_root.join(local_plugin_rel_dir());
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

/// Installs the Antigravity plugin bundle into the user-global plugin root.
///
/// # Errors
/// Returns an error message if source resolution, file copying, or binary
/// staging fails.
pub fn install_plugin_interactive(
    workspace_root: &Path,
    runner: &ProjectRunnerConfig,
) -> Result<String, String> {
    let local_plugin = resolve_source_plugin(workspace_root)?;

    let dest = plugin_install_dir(runner)
        .ok_or_else(|| "HOME environment variable error: HOME is not set".to_owned())?;
    if let Some(target_dir) = dest.parent() {
        std::fs::create_dir_all(target_dir)
            .map_err(|e| format!("Failed to create {}: {e}", target_dir.display()))?;
    }

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

    copy_dir_all(&local_plugin, &dest).map_err(|e| {
        format!(
            "Failed to copy plugin files from {} to {}: {e}",
            local_plugin.display(),
            dest.display()
        )
    })?;

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
    ensure_binaries_executable(&dest_bin)?;

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

fn write_pretty_json_atomic(config_path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let serialized = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize {}: {e}", config_path.display()))?;
    atomic_write(config_path, serialized.as_bytes())
        .map_err(|e| format!("Failed to write {}: {e}", config_path.display()))
}

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
///
/// # Errors
/// Returns an error if an existing config file cannot be parsed as a JSON
/// object or written atomically.
pub fn register_opencode_config(workspace_root: &Path) -> Result<String, String> {
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
/// `.claude/settings.json` for Claude Code while preserving existing keys.
///
/// # Errors
/// Returns an error if any existing JSON file is invalid or writing fails.
pub fn register_claude_config(workspace_root: &Path) -> Result<String, String> {
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

/// Registers `tkg-mcp` in `.cursor/mcp.json` for Cursor IDE while preserving
/// existing servers.
///
/// # Errors
/// Returns an error if `.cursor/mcp.json` is corrupt or cannot be written.
pub fn register_cursor_config(workspace_root: &Path) -> Result<String, String> {
    let cursor_dir = workspace_root.join(".cursor");
    std::fs::create_dir_all(&cursor_dir)
        .map_err(|e| format!("Failed to create {}: {e}", cursor_dir.display()))?;
    let config_path = cursor_dir.join("mcp.json");
    let mut config = read_or_init_json_object(&config_path, serde_json::json!({}))?;
    upsert_torqkage_mcp_server(&mut config);
    write_pretty_json_atomic(&config_path, &config)?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' in {}",
        config_path.display()
    ))
}

/// Generates a standalone MCP server configuration in
/// `.tkg/state/mcp_servers.json` for generic harnesses.
///
/// # Errors
/// Returns an error if the state directory cannot be created or the config file
/// cannot be written.
pub fn register_custom_mcp_config(workspace_root: &Path) -> Result<String, String> {
    let state_dir = workspace_root.join(".tkg/state");
    std::fs::create_dir_all(&state_dir)
        .map_err(|e| format!("Failed to create {}: {e}", state_dir.display()))?;
    let config_path = state_dir.join("mcp_servers.json");
    let mut config = read_or_init_json_object(&config_path, serde_json::json!({}))?;
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

        _ = std::fs::create_dir_all(&temp);
        std::fs::write(temp.join("plugin.json"), "{}").expect("write plugin.json");
        let resolved = resolve_source_plugin(&temp).expect("resolve directly");
        assert_eq!(resolved, temp);

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

        register_opencode_config(&temp_ws).unwrap();
        let oc_content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(temp_ws.join("opencode.json")).unwrap())
                .unwrap();
        assert_eq!(
            oc_content
                .pointer("/mcpServers/torqkage/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-mcp")
        );

        register_claude_config(&temp_ws).unwrap();
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp_ws.join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            settings
                .pointer("/hooks/PreToolUse/0/hooks/0/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-hooks pre-tool-use")
        );

        register_cursor_config(&temp_ws).unwrap();
        let cursor_cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp_ws.join(".cursor/mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            cursor_cfg
                .pointer("/mcpServers/torqkage/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-mcp")
        );

        register_custom_mcp_config(&temp_ws).unwrap();
        let custom_cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp_ws.join(".tkg/state/mcp_servers.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            custom_cfg
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
        assert_eq!(pre_tool.len(), 2);

        std::fs::write(temp_ws.join(".mcp.json"), "{ invalid json").unwrap();
        let err = register_claude_config(&temp_ws).unwrap_err();
        assert!(err.contains("Failed to parse JSON"));

        _ = std::fs::remove_dir_all(&temp_ws);
    }
}
