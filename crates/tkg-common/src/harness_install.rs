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

fn upsert_mcp_server_entry(
    config: &mut serde_json::Value,
    servers_key: &str,
    entry: serde_json::Value,
) {
    if let Some(obj) = config.as_object_mut() {
        let servers = obj
            .entry(servers_key)
            .or_insert_with(|| serde_json::json!({}));
        if let Some(s_obj) = servers.as_object_mut() {
            s_obj.insert("torqkage".to_owned(), entry);
        }
    }
}

fn upsert_torqkage_mcp_server(config: &mut serde_json::Value) {
    upsert_mcp_server_entry(
        config,
        "mcpServers",
        serde_json::json!({
            "command": "tkg-mcp",
            "args": []
        }),
    );
}

fn register_mcp_json_file(
    config_path: &Path,
    servers_key: &str,
    entry: serde_json::Value,
    default_json: serde_json::Value,
) -> Result<(), String> {
    if let Some(parent) = config_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    let mut config = read_or_init_json_object(config_path, default_json)?;
    upsert_mcp_server_entry(&mut config, servers_key, entry);
    write_pretty_json_atomic(config_path, &config)
}

/// Registers `tkg-mcp` in `opencode.json` for `OpenCode` integration.
///
/// # Errors
/// Returns an error if an existing config file cannot be parsed as a JSON
/// object or written atomically.
pub fn register_opencode_config(workspace_root: &Path) -> Result<String, String> {
    let config_path = workspace_root.join("opencode.json");
    register_mcp_json_file(
        &config_path,
        "mcpServers",
        serde_json::json!({ "command": "tkg-mcp", "args": [] }),
        serde_json::json!({ "$schema": "https://opencode.ai/schema.json" }),
    )?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' in opencode.json at {}",
        config_path.display()
    ))
}

fn upsert_command_hook_event(
    hooks_obj: &mut serde_json::Map<String, serde_json::Value>,
    event: &str,
    matcher: Option<&str>,
    subcommand: &str,
    timeout: Option<u64>,
    hook_name: Option<&str>,
    nested_hooks: bool,
) {
    let entry_list = hooks_obj
        .entry(event)
        .or_insert_with(|| serde_json::json!([]));
    let Some(arr) = entry_list.as_array_mut() else {
        return;
    };

    let needle = format!("tkg-hooks {subcommand}");
    arr.retain(|item| {
        let flat_match = item
            .get("command")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|cmd| cmd.contains(&needle));
        let nested_match = item
            .get("hooks")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|hs| {
                hs.iter().any(|h| {
                    h.get("command")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|cmd| cmd.contains(&needle))
                })
            });
        !flat_match && !nested_match
    });

    let mut cmd_map = serde_json::Map::new();
    if let Some(name) = hook_name {
        cmd_map.insert("name".to_owned(), serde_json::json!(name));
    }
    if nested_hooks {
        cmd_map.insert("type".to_owned(), serde_json::json!("command"));
    }
    cmd_map.insert("command".to_owned(), serde_json::json!(needle));
    if let Some(t) = timeout {
        cmd_map.insert("timeout".to_owned(), serde_json::json!(t));
    }

    if nested_hooks {
        let mut wrapper = serde_json::Map::new();
        if let Some(m) = matcher {
            wrapper.insert("matcher".to_owned(), serde_json::json!(m));
        }
        wrapper.insert(
            "hooks".to_owned(),
            serde_json::json!([serde_json::Value::Object(cmd_map)]),
        );
        arr.push(serde_json::Value::Object(wrapper));
    } else {
        if let Some(m) = matcher {
            cmd_map.insert("matcher".to_owned(), serde_json::json!(m));
        }
        arr.push(serde_json::Value::Object(cmd_map));
    }
}

/// Registers `tkg-mcp` in `.mcp.json` and `tkg-hooks` in
/// `.claude/settings.json` for Claude Code while preserving existing keys.
///
/// # Errors
/// Returns an error if any existing JSON file is invalid or writing fails.
pub fn register_claude_config(workspace_root: &Path) -> Result<String, String> {
    let mcp_path = workspace_root.join(".mcp.json");
    register_mcp_json_file(
        &mcp_path,
        "mcpServers",
        serde_json::json!({ "command": "tkg-mcp", "args": [] }),
        serde_json::json!({}),
    )?;

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
            upsert_command_hook_event(
                hooks_obj,
                "PreToolUse",
                Some("*"),
                "pre-tool-use",
                Some(15),
                None,
                true,
            );
            upsert_command_hook_event(
                hooks_obj,
                "SessionStart",
                None,
                "inject-wake",
                Some(5),
                None,
                true,
            );
            upsert_command_hook_event(
                hooks_obj,
                "Stop",
                None,
                "record-handoff",
                Some(10),
                None,
                true,
            );
        }
    }
    write_pretty_json_atomic(&settings_path, &settings)?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' in {} and hooks in {}",
        mcp_path.display(),
        settings_path.display()
    ))
}

/// Registers `tkg-mcp` in `.cursor/mcp.json` and native Cursor agent hooks in
/// `.cursor/hooks.json` while preserving existing entries.
///
/// # Errors
/// Returns an error if `.cursor/mcp.json` or `.cursor/hooks.json` is corrupt
/// or cannot be written.
pub fn register_cursor_config(workspace_root: &Path) -> Result<String, String> {
    let cursor_dir = workspace_root.join(".cursor");
    std::fs::create_dir_all(&cursor_dir)
        .map_err(|e| format!("Failed to create {}: {e}", cursor_dir.display()))?;
    let mcp_path = cursor_dir.join("mcp.json");
    register_mcp_json_file(
        &mcp_path,
        "mcpServers",
        serde_json::json!({ "command": "tkg-mcp", "args": [] }),
        serde_json::json!({}),
    )?;

    let hooks_path = cursor_dir.join("hooks.json");
    let mut hooks_cfg = read_or_init_json_object(&hooks_path, serde_json::json!({ "version": 1 }))?;
    if let Some(root_obj) = hooks_cfg.as_object_mut() {
        root_obj
            .entry("version")
            .or_insert_with(|| serde_json::json!(1));
        let hooks = root_obj
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(hooks_obj) = hooks.as_object_mut() {
            upsert_command_hook_event(
                hooks_obj,
                "preToolUse",
                Some(".*"),
                "pre-tool-use",
                Some(15),
                None,
                false,
            );
            upsert_command_hook_event(
                hooks_obj,
                "beforeShellExecution",
                None,
                "pre-tool-use",
                Some(15),
                None,
                false,
            );
            upsert_command_hook_event(
                hooks_obj,
                "beforeMCPExecution",
                None,
                "pre-tool-use",
                Some(15),
                None,
                false,
            );
            upsert_command_hook_event(
                hooks_obj,
                "sessionStart",
                None,
                "inject-wake",
                Some(5),
                None,
                false,
            );
            upsert_command_hook_event(
                hooks_obj,
                "stop",
                None,
                "record-handoff",
                Some(10),
                None,
                false,
            );
        }
    }
    write_pretty_json_atomic(&hooks_path, &hooks_cfg)?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' in {} and hooks in {}",
        mcp_path.display(),
        hooks_path.display()
    ))
}

/// Registers `tkg-mcp` in `.codex/config.toml` (`[mcp_servers.torqkage]`) and
/// lifecycle hooks in `.codex/hooks.json` for `OpenAI` Codex CLI.
///
/// # Errors
/// Returns an error if existing config files are invalid TOML/JSON or writing
/// fails.
pub fn register_codex_config(workspace_root: &Path) -> Result<String, String> {
    let codex_dir = workspace_root.join(".codex");
    std::fs::create_dir_all(&codex_dir)
        .map_err(|e| format!("Failed to create {}: {e}", codex_dir.display()))?;

    let toml_path = codex_dir.join("config.toml");
    let mut table: toml::Table = if toml_path.is_file() {
        let raw = std::fs::read_to_string(&toml_path)
            .map_err(|e| format!("Failed to read {}: {e}", toml_path.display()))?;
        if raw.trim().is_empty() {
            toml::Table::new()
        } else {
            toml::from_str(&raw)
                .map_err(|e| format!("Failed to parse TOML in {}: {e}", toml_path.display()))?
        }
    } else {
        toml::Table::new()
    };

    let mcp_entry = table
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let mcp_table = mcp_entry.as_table_mut().ok_or_else(|| {
        format!(
            "Expected [mcp_servers] to be a TOML table in {}",
            toml_path.display()
        )
    })?;
    let mut server_tbl = toml::Table::new();
    server_tbl.insert(
        "command".to_owned(),
        toml::Value::String("tkg-mcp".to_owned()),
    );
    server_tbl.insert("args".to_owned(), toml::Value::Array(Vec::new()));
    mcp_table.insert("torqkage".to_owned(), toml::Value::Table(server_tbl));

    let serialized_toml = toml::to_string_pretty(&table)
        .map_err(|e| format!("Failed to serialize {}: {e}", toml_path.display()))?;
    atomic_write(&toml_path, serialized_toml.as_bytes())
        .map_err(|e| format!("Failed to write {}: {e}", toml_path.display()))?;

    let hooks_path = codex_dir.join("hooks.json");
    let mut hooks_root = read_or_init_json_object(&hooks_path, serde_json::json!({}))?;
    if let Some(hooks_obj) = hooks_root.as_object_mut() {
        upsert_command_hook_event(
            hooks_obj,
            "PreToolUse",
            Some(".*"),
            "pre-tool-use",
            Some(15),
            None,
            true,
        );
        upsert_command_hook_event(
            hooks_obj,
            "SessionStart",
            None,
            "inject-wake",
            Some(5),
            None,
            true,
        );
    }
    write_pretty_json_atomic(&hooks_path, &hooks_root)?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' in {} and hooks in {}",
        toml_path.display(),
        hooks_path.display()
    ))
}

/// Registers `tkg-mcp` and lifecycle hooks (`BeforeTool`, `SessionStart`) in
/// `.gemini/settings.json` for Google Gemini CLI.
///
/// # Errors
/// Returns an error if `.gemini/settings.json` is corrupt or cannot be written.
pub fn register_gemini_config(workspace_root: &Path) -> Result<String, String> {
    let settings_path = workspace_root.join(".gemini/settings.json");
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    let mut settings = read_or_init_json_object(&settings_path, serde_json::json!({}))?;
    upsert_torqkage_mcp_server(&mut settings);
    if let Some(root_obj) = settings.as_object_mut() {
        let hooks = root_obj
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(hooks_obj) = hooks.as_object_mut() {
            upsert_command_hook_event(
                hooks_obj,
                "BeforeTool",
                Some(".*"),
                "pre-tool-use",
                Some(15_000),
                Some("torqkage-pre-tool-use"),
                true,
            );
            upsert_command_hook_event(
                hooks_obj,
                "SessionStart",
                Some("*"),
                "inject-wake",
                Some(5_000),
                Some("torqkage-session-start"),
                true,
            );
        }
    }
    write_pretty_json_atomic(&settings_path, &settings)?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' and hooks in {}",
        settings_path.display()
    ))
}

/// Registers `tkg-mcp` in `.vscode/mcp.json` using VS Code Agent Mode's
/// `"servers"` schema (`type = "stdio"`).
///
/// # Errors
/// Returns an error if `.vscode/mcp.json` is corrupt or cannot be written.
pub fn register_vscode_config(workspace_root: &Path) -> Result<String, String> {
    let mcp_path = workspace_root.join(".vscode/mcp.json");
    register_mcp_json_file(
        &mcp_path,
        "servers",
        serde_json::json!({
            "type": "stdio",
            "command": "tkg-mcp",
            "args": []
        }),
        serde_json::json!({}),
    )?;

    Ok(format!("✔ Registered 'tkg-mcp' in {}", mcp_path.display()))
}

/// Registers `tkg-mcp` in `.vscode/mcp.json` and lifecycle hooks in
/// `.github/hooks/torqkage.json` for GitHub Copilot CLI / Coding Agent.
///
/// # Errors
/// Returns an error if existing JSON files are corrupt or cannot be written.
pub fn register_github_copilot_config(workspace_root: &Path) -> Result<String, String> {
    drop(register_vscode_config(workspace_root)?);
    let hooks_dir = workspace_root.join(".github/hooks");
    std::fs::create_dir_all(&hooks_dir)
        .map_err(|e| format!("Failed to create {}: {e}", hooks_dir.display()))?;
    let hooks_path = hooks_dir.join("torqkage.json");
    let mut hooks_cfg = read_or_init_json_object(&hooks_path, serde_json::json!({ "version": 1 }))?;
    if let Some(root_obj) = hooks_cfg.as_object_mut() {
        root_obj
            .entry("version")
            .or_insert_with(|| serde_json::json!(1));
        let hooks = root_obj
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(hooks_obj) = hooks.as_object_mut() {
            upsert_command_hook_event(
                hooks_obj,
                "preToolUse",
                None,
                "pre-tool-use",
                Some(15),
                None,
                false,
            );
            upsert_command_hook_event(
                hooks_obj,
                "sessionStart",
                None,
                "inject-wake",
                Some(5),
                None,
                false,
            );
        }
    }
    write_pretty_json_atomic(&hooks_path, &hooks_cfg)?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' in {} and hooks in {}",
        workspace_root.join(".vscode/mcp.json").display(),
        hooks_path.display()
    ))
}

/// Registers `tkg-mcp` in `.roo/mcp.json` for Roo Code.
///
/// # Errors
/// Returns an error if `.roo/mcp.json` is corrupt or cannot be written.
pub fn register_roo_config(workspace_root: &Path) -> Result<String, String> {
    let config_path = workspace_root.join(".roo/mcp.json");
    register_mcp_json_file(
        &config_path,
        "mcpServers",
        serde_json::json!({ "command": "tkg-mcp", "args": [] }),
        serde_json::json!({}),
    )?;

    Ok(format!(
        "✔ Registered 'tkg-mcp' in {}",
        config_path.display()
    ))
}

/// Resolves the target Windsurf MCP config path (`~/.codeium/windsurf/mcp_config.json`
/// when `HOME` is set, otherwise `<ws>/.windsurf/mcp_config.json`).
#[must_use]
pub fn windsurf_mcp_config_path(workspace_root: &Path) -> PathBuf {
    crate::harness::home_dir().map_or_else(
        || workspace_root.join(".windsurf/mcp_config.json"),
        |h| h.join(".codeium/windsurf/mcp_config.json"),
    )
}

/// Registers `tkg-mcp` at an explicit Windsurf MCP config file path.
///
/// # Errors
/// Returns an error if the config file is corrupt or cannot be written.
pub fn register_windsurf_config_at(config_path: &Path) -> Result<String, String> {
    register_mcp_json_file(
        config_path,
        "mcpServers",
        serde_json::json!({ "command": "tkg-mcp", "args": [] }),
        serde_json::json!({}),
    )?;
    Ok(format!(
        "✔ Registered 'tkg-mcp' for Windsurf at {}",
        config_path.display()
    ))
}

/// Registers `tkg-mcp` for Windsurf (`~/.codeium/windsurf/mcp_config.json`).
///
/// # Errors
/// Returns an error if the config file is corrupt or cannot be written.
pub fn register_windsurf_config(workspace_root: &Path) -> Result<String, String> {
    register_windsurf_config_at(&windsurf_mcp_config_path(workspace_root))
}

/// Resolves the active Cline MCP settings path across VS Code globalStorage
/// directories and Cline CLI (`~/.cline/data/settings/cline_mcp_settings.json`).
#[must_use]
pub fn cline_mcp_config_path(workspace_root: &Path) -> PathBuf {
    let mut candidates = Vec::new();
    #[cfg(target_os = "windows")]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        candidates.push(PathBuf::from(appdata).join(
            "Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json",
        ));
    }
    if let Some(home) = crate::harness::home_dir() {
        #[cfg(target_os = "macos")]
        candidates.push(
            home.join("Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"),
        );
        #[cfg(all(unix, not(target_os = "macos")))]
        candidates.push(
            home.join(".config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"),
        );
        candidates.push(home.join(".cline/data/settings/cline_mcp_settings.json"));
    }

    for c in &candidates {
        if c.is_file() || c.parent().is_some_and(Path::is_dir) {
            return c.clone();
        }
    }
    candidates
        .into_iter()
        .last()
        .unwrap_or_else(|| workspace_root.join(".cline/cline_mcp_settings.json"))
}

/// Registers `tkg-mcp` at an explicit Cline MCP settings path.
///
/// # Errors
/// Returns an error if the config file is corrupt or cannot be written.
pub fn register_cline_config_at(config_path: &Path) -> Result<String, String> {
    register_mcp_json_file(
        config_path,
        "mcpServers",
        serde_json::json!({ "command": "tkg-mcp", "args": [] }),
        serde_json::json!({}),
    )?;
    Ok(format!(
        "✔ Registered 'tkg-mcp' for Cline at {}",
        config_path.display()
    ))
}

/// Registers `tkg-mcp` for Cline IDE / CLI.
///
/// # Errors
/// Returns an error if the config file is corrupt or cannot be written.
pub fn register_cline_config(workspace_root: &Path) -> Result<String, String> {
    register_cline_config_at(&cline_mcp_config_path(workspace_root))
}

/// Generates a standalone MCP server configuration in
/// `.tkg/state/mcp_servers.json` for generic harnesses.
///
/// # Errors
/// Returns an error if the state directory cannot be created or the config file
/// cannot be written.
pub fn register_custom_mcp_config(workspace_root: &Path) -> Result<String, String> {
    let config_path = workspace_root.join(".tkg/state/mcp_servers.json");
    register_mcp_json_file(
        &config_path,
        "mcpServers",
        serde_json::json!({ "command": "tkg-mcp", "args": [] }),
        serde_json::json!({}),
    )?;

    Ok(format!(
        "✔ Generated MCP server config at {}",
        config_path.display()
    ))
}

/// Removes empty directories starting at `start_dir` up to (not including)
/// `stop_at`. Ignores non-empty or missing directories.
pub fn prune_empty_ancestors(start_dir: &Path, stop_at: &Path) {
    let mut current = start_dir;
    while current != stop_at {
        if !current.starts_with(stop_at) {
            break;
        }
        if std::fs::remove_dir(current).is_err() {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
}

fn remove_file_and_prune(file_path: &Path, stop_at: &Path) -> Result<(), String> {
    match std::fs::remove_file(file_path) {
        Ok(()) => {},
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("Failed to remove {}: {err}", file_path.display())),
    }
    if let Some(parent) = file_path.parent() {
        prune_empty_ancestors(parent, stop_at);
    }
    Ok(())
}

fn is_tkg_hook_item(item: &serde_json::Value) -> bool {
    item.get("command")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|cmd| cmd.contains("tkg-hooks "))
}

fn strip_tkg_hooks_from_events_map(
    events_obj: &mut serde_json::Map<String, serde_json::Value>,
) -> bool {
    let mut changed = false;
    let mut empty_events = Vec::new();

    for (event_name, entry_list) in events_obj.iter_mut() {
        let Some(arr) = entry_list.as_array_mut() else {
            continue;
        };
        let before_len = arr.len();
        arr.retain_mut(|item| {
            if is_tkg_hook_item(item) {
                return false;
            }
            if let Some(nested) = item
                .get_mut("hooks")
                .and_then(serde_json::Value::as_array_mut)
            {
                let inner_before = nested.len();
                nested.retain(|h| !is_tkg_hook_item(h));
                if nested.len() != inner_before {
                    changed = true;
                }
                return !nested.is_empty();
            }
            true
        });
        if arr.len() != before_len {
            changed = true;
        }
        if arr.is_empty() {
            empty_events.push(event_name.clone());
        }
    }

    for event_name in empty_events {
        events_obj.remove(&event_name);
    }
    changed
}

fn remove_mcp_server_from_obj(
    root_obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> bool {
    let Some(servers_val) = root_obj.get_mut(key) else {
        return false;
    };
    let Some(servers_obj) = servers_val.as_object_mut() else {
        return false;
    };
    let removed = servers_obj.remove("torqkage").is_some();
    if servers_obj.is_empty() {
        root_obj.remove(key);
    }
    removed
}

fn json_only_contains_boilerplate(
    root_obj: &serde_json::Map<String, serde_json::Value>,
    boilerplate: &[(&str, &str)],
) -> bool {
    root_obj.is_empty()
        || root_obj.iter().all(|(k, v)| {
            boilerplate.iter().any(|(bk, bv)| {
                k == bk
                    && match v {
                        serde_json::Value::String(s) => s == bv,
                        serde_json::Value::Number(n) => n.to_string() == *bv,
                        _ => false,
                    }
            })
        })
}

fn write_or_delete_json(
    config_path: &Path,
    config: &serde_json::Value,
    boilerplate: &[(&str, &str)],
    stop_prune_at: &Path,
) -> Result<(), String> {
    if let Some(obj) = config.as_object()
        && json_only_contains_boilerplate(obj, boilerplate)
    {
        return remove_file_and_prune(config_path, stop_prune_at);
    }
    write_pretty_json_atomic(config_path, config)
}

fn eject_mcp_json_file(
    config_path: &Path,
    servers_key: &str,
    boilerplate: &[(&str, &str)],
    stop_prune_at: &Path,
) -> Result<bool, String> {
    if !config_path.is_file() {
        return Ok(false);
    }
    let mut config = read_or_init_json_object(config_path, serde_json::json!({}))?;
    let changed = config
        .as_object_mut()
        .is_some_and(|obj| remove_mcp_server_from_obj(obj, servers_key));
    if !changed {
        return Ok(false);
    }
    write_or_delete_json(config_path, &config, boilerplate, stop_prune_at)?;
    Ok(true)
}

fn eject_hooks_json_file(
    hooks_path: &Path,
    nested_hooks_key: Option<&str>,
    boilerplate: &[(&str, &str)],
    stop_prune_at: &Path,
) -> Result<bool, String> {
    if !hooks_path.is_file() {
        return Ok(false);
    }
    let mut cfg = read_or_init_json_object(hooks_path, serde_json::json!({}))?;
    let changed = cfg.as_object_mut().is_some_and(|root_obj| {
        if let Some(container_key) = nested_hooks_key {
            let Some(container) = root_obj
                .get_mut(container_key)
                .and_then(serde_json::Value::as_object_mut)
            else {
                return false;
            };
            let modified = strip_tkg_hooks_from_events_map(container);
            if container.is_empty() {
                root_obj.remove(container_key);
            }
            modified
        } else {
            strip_tkg_hooks_from_events_map(root_obj)
        }
    });
    if !changed {
        return Ok(false);
    }
    write_or_delete_json(hooks_path, &cfg, boilerplate, stop_prune_at)?;
    Ok(true)
}

fn format_ejected_paths(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| {
            if p.is_file() {
                format!("✔ Removed TorqKage entries from {}", p.display())
            } else {
                format!("✔ Removed empty TorqKage config {}", p.display())
            }
        })
        .collect()
}

/// Removes `TorqKage` MCP registration from `opencode.json`.
///
/// # Errors
/// Returns an error if an existing `opencode.json` is invalid JSON or cannot be
/// updated/removed.
pub fn eject_opencode_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    let config_path = workspace_root.join("opencode.json");
    let mut touched = Vec::new();
    if eject_mcp_json_file(
        &config_path,
        "mcpServers",
        &[("$schema", "https://opencode.ai/schema.json")],
        workspace_root,
    )? {
        touched.push(config_path);
    }
    Ok(format_ejected_paths(touched))
}

/// Removes `TorqKage` entries from `.mcp.json` and `.claude/settings.json`.
///
/// # Errors
/// Returns an error if any existing config is invalid JSON or cannot be
/// updated/removed.
pub fn eject_claude_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    let mut touched = Vec::new();
    let mcp_path = workspace_root.join(".mcp.json");
    if eject_mcp_json_file(&mcp_path, "mcpServers", &[], workspace_root)? {
        touched.push(mcp_path);
    }
    let settings_path = workspace_root.join(".claude/settings.json");
    if eject_hooks_json_file(&settings_path, Some("hooks"), &[], workspace_root)? {
        touched.push(settings_path);
    }
    Ok(format_ejected_paths(touched))
}

/// Removes `TorqKage` entries from `.cursor/mcp.json` and `.cursor/hooks.json`.
///
/// # Errors
/// Returns an error if any existing config is invalid JSON or cannot be
/// updated/removed.
pub fn eject_cursor_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    let mut touched = Vec::new();
    let mcp_path = workspace_root.join(".cursor/mcp.json");
    if eject_mcp_json_file(&mcp_path, "mcpServers", &[], workspace_root)? {
        touched.push(mcp_path);
    }
    let hooks_path = workspace_root.join(".cursor/hooks.json");
    if eject_hooks_json_file(
        &hooks_path,
        Some("hooks"),
        &[("version", "1")],
        workspace_root,
    )? {
        touched.push(hooks_path);
    }
    Ok(format_ejected_paths(touched))
}

/// Removes `TorqKage` entries from `.codex/config.toml` and `.codex/hooks.json`.
///
/// # Errors
/// Returns an error if existing files are invalid TOML/JSON or cannot be
/// updated/removed.
pub fn eject_codex_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    let mut touched = Vec::new();
    let toml_path = workspace_root.join(".codex/config.toml");
    if toml_path.is_file() {
        let raw = std::fs::read_to_string(&toml_path)
            .map_err(|e| format!("Failed to read {}: {e}", toml_path.display()))?;
        if !raw.trim().is_empty() {
            let mut table: toml::Table = toml::from_str(&raw)
                .map_err(|e| format!("Failed to parse TOML in {}: {e}", toml_path.display()))?;
            let removed = if let Some(mcp_table) = table
                .get_mut("mcp_servers")
                .and_then(toml::Value::as_table_mut)
            {
                let did_remove = mcp_table.remove("torqkage").is_some();
                if mcp_table.is_empty() {
                    table.remove("mcp_servers");
                }
                did_remove
            } else {
                false
            };
            if removed {
                if table.is_empty() {
                    remove_file_and_prune(&toml_path, workspace_root)?;
                } else {
                    let serialized = toml::to_string_pretty(&table)
                        .map_err(|e| format!("Failed to serialize {}: {e}", toml_path.display()))?;
                    atomic_write(&toml_path, serialized.as_bytes())
                        .map_err(|e| format!("Failed to write {}: {e}", toml_path.display()))?;
                }
                touched.push(toml_path);
            }
        }
    }

    let hooks_path = workspace_root.join(".codex/hooks.json");
    if eject_hooks_json_file(&hooks_path, None, &[], workspace_root)? {
        touched.push(hooks_path);
    }
    Ok(format_ejected_paths(touched))
}

/// Removes `TorqKage` MCP and hook entries from `.gemini/settings.json`.
///
/// # Errors
/// Returns an error if `.gemini/settings.json` is invalid JSON or cannot be
/// updated/removed.
pub fn eject_gemini_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    let settings_path = workspace_root.join(".gemini/settings.json");
    if !settings_path.is_file() {
        return Ok(Vec::new());
    }
    let mut settings = read_or_init_json_object(&settings_path, serde_json::json!({}))?;
    let mut changed = false;
    if let Some(root_obj) = settings.as_object_mut() {
        if remove_mcp_server_from_obj(root_obj, "mcpServers") {
            changed = true;
        }
        if let Some(hooks_obj) = root_obj
            .get_mut("hooks")
            .and_then(serde_json::Value::as_object_mut)
        {
            if strip_tkg_hooks_from_events_map(hooks_obj) {
                changed = true;
            }
            if hooks_obj.is_empty() {
                root_obj.remove("hooks");
            }
        }
    }
    if !changed {
        return Ok(Vec::new());
    }
    write_or_delete_json(&settings_path, &settings, &[], workspace_root)?;
    Ok(format_ejected_paths(vec![settings_path]))
}

/// Removes `TorqKage` MCP registration from `.vscode/mcp.json`.
///
/// # Errors
/// Returns an error if `.vscode/mcp.json` is invalid JSON or cannot be
/// updated/removed.
pub fn eject_vscode_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    let mcp_path = workspace_root.join(".vscode/mcp.json");
    let mut touched = Vec::new();
    if eject_mcp_json_file(&mcp_path, "servers", &[], workspace_root)? {
        touched.push(mcp_path);
    }
    Ok(format_ejected_paths(touched))
}

/// Removes `TorqKage` MCP registration from `.vscode/mcp.json` and hooks from
/// `.github/hooks/torqkage.json`.
///
/// # Errors
/// Returns an error if any existing config is invalid JSON or cannot be
/// updated/removed.
pub fn eject_github_copilot_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    let mut touched = Vec::new();
    let mcp_path = workspace_root.join(".vscode/mcp.json");
    if eject_mcp_json_file(&mcp_path, "servers", &[], workspace_root)? {
        touched.push(mcp_path);
    }
    let hooks_path = workspace_root.join(".github/hooks/torqkage.json");
    if eject_hooks_json_file(
        &hooks_path,
        Some("hooks"),
        &[("version", "1")],
        workspace_root,
    )? {
        touched.push(hooks_path);
    }
    Ok(format_ejected_paths(touched))
}

/// Removes `TorqKage` MCP registration from `.roo/mcp.json`.
///
/// # Errors
/// Returns an error if `.roo/mcp.json` is invalid JSON or cannot be
/// updated/removed.
pub fn eject_roo_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    let config_path = workspace_root.join(".roo/mcp.json");
    let mut touched = Vec::new();
    if eject_mcp_json_file(&config_path, "mcpServers", &[], workspace_root)? {
        touched.push(config_path);
    }
    Ok(format_ejected_paths(touched))
}

/// Removes `TorqKage` MCP registration from an explicit Windsurf config file.
///
/// # Errors
/// Returns an error if the config file is invalid JSON or cannot be
/// updated/removed.
pub fn eject_windsurf_config_at(config_path: &Path) -> Result<Vec<String>, String> {
    let stop_at = config_path.parent().unwrap_or_else(|| Path::new("/"));
    let mut touched = Vec::new();
    if eject_mcp_json_file(config_path, "mcpServers", &[], stop_at)? {
        touched.push(config_path.to_path_buf());
    }
    Ok(format_ejected_paths(touched))
}

/// Removes `TorqKage` MCP registration from the resolved Windsurf config path.
///
/// # Errors
/// Returns an error if the config file is invalid JSON or cannot be
/// updated/removed.
pub fn eject_windsurf_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    eject_windsurf_config_at(&windsurf_mcp_config_path(workspace_root))
}

/// Removes `TorqKage` MCP registration from an explicit Cline settings file.
///
/// # Errors
/// Returns an error if the settings file is invalid JSON or cannot be
/// updated/removed.
pub fn eject_cline_config_at(config_path: &Path) -> Result<Vec<String>, String> {
    let stop_at = config_path.parent().unwrap_or_else(|| Path::new("/"));
    let mut touched = Vec::new();
    if eject_mcp_json_file(config_path, "mcpServers", &[], stop_at)? {
        touched.push(config_path.to_path_buf());
    }
    Ok(format_ejected_paths(touched))
}

/// Removes `TorqKage` MCP registration from the resolved Cline settings path.
///
/// # Errors
/// Returns an error if the settings file is invalid JSON or cannot be
/// updated/removed.
pub fn eject_cline_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    eject_cline_config_at(&cline_mcp_config_path(workspace_root))
}

/// Removes the generated standalone `.tkg/state/mcp_servers.json`.
///
/// # Errors
/// Returns an error if `.tkg/state/mcp_servers.json` is invalid JSON or cannot
/// be removed.
pub fn eject_custom_mcp_config(workspace_root: &Path) -> Result<Vec<String>, String> {
    let config_path = workspace_root.join(".tkg/state/mcp_servers.json");
    let mut touched = Vec::new();
    if eject_mcp_json_file(&config_path, "mcpServers", &[], workspace_root)? {
        touched.push(config_path);
    }
    Ok(format_ejected_paths(touched))
}

/// Removes the installed plugin bundle directory/symlink for `runner` if
/// present (e.g. `~/.gemini/config/plugins/torqkage` for Antigravity).
///
/// # Errors
/// Returns an error if the plugin bundle exists and cannot be removed.
pub fn eject_plugin_dir(runner: &ProjectRunnerConfig) -> Result<Vec<String>, String> {
    let Some(dest) = plugin_install_dir(runner) else {
        return Ok(Vec::new());
    };
    let Ok(meta) = dest.symlink_metadata() else {
        return Ok(Vec::new());
    };
    if meta.file_type().is_symlink() || !dest.is_dir() {
        std::fs::remove_file(&dest)
            .map_err(|e| format!("Failed to remove plugin link {}: {e}", dest.display()))?;
    } else {
        std::fs::remove_dir_all(&dest)
            .map_err(|e| format!("Failed to remove plugin dir {}: {e}", dest.display()))?;
    }
    Ok(vec![format!(
        "✔ Removed installed plugin bundle at {}",
        dest.display()
    )])
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
        let cursor_hooks: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp_ws.join(".cursor/hooks.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            cursor_hooks
                .pointer("/hooks/preToolUse/0/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-hooks pre-tool-use")
        );

        register_codex_config(&temp_ws).unwrap();
        let codex_toml: toml::Table =
            toml::from_str(&std::fs::read_to_string(temp_ws.join(".codex/config.toml")).unwrap())
                .unwrap();
        assert_eq!(
            codex_toml
                .get("mcp_servers")
                .and_then(|v| v.get("torqkage"))
                .and_then(|v| v.get("command"))
                .and_then(toml::Value::as_str),
            Some("tkg-mcp")
        );
        let codex_hooks: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp_ws.join(".codex/hooks.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            codex_hooks
                .pointer("/PreToolUse/0/hooks/0/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-hooks pre-tool-use")
        );

        register_gemini_config(&temp_ws).unwrap();
        let gemini_cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp_ws.join(".gemini/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            gemini_cfg
                .pointer("/mcpServers/torqkage/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-mcp")
        );
        assert_eq!(
            gemini_cfg
                .pointer("/hooks/BeforeTool/0/hooks/0/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-hooks pre-tool-use")
        );

        register_github_copilot_config(&temp_ws).unwrap();
        let vscode_cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp_ws.join(".vscode/mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            vscode_cfg
                .pointer("/servers/torqkage/type")
                .and_then(serde_json::Value::as_str),
            Some("stdio")
        );
        let gh_hooks: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp_ws.join(".github/hooks/torqkage.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            gh_hooks
                .pointer("/hooks/preToolUse/0/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-hooks pre-tool-use")
        );

        register_roo_config(&temp_ws).unwrap();
        let roo_cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(temp_ws.join(".roo/mcp.json")).unwrap())
                .unwrap();
        assert_eq!(
            roo_cfg
                .pointer("/mcpServers/torqkage/command")
                .and_then(serde_json::Value::as_str),
            Some("tkg-mcp")
        );

        register_windsurf_config_at(&temp_ws.join("windsurf_mcp.json")).unwrap();
        register_cline_config_at(&temp_ws.join("cline_mcp.json")).unwrap();
        assert!(temp_ws.join("windsurf_mcp.json").is_file());
        assert!(temp_ws.join("cline_mcp.json").is_file());

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

    #[test]
    fn test_register_codex_preserves_existing_toml_keys() {
        let temp_ws =
            std::env::temp_dir().join(format!("tkg_codex_merge_test_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&temp_ws);
        std::fs::create_dir_all(temp_ws.join(".codex")).unwrap();

        std::fs::write(
            temp_ws.join(".codex/config.toml"),
            "model = \"o4-mini\"\n\n[mcp_servers.other]\ncommand = \"other-mcp\"\n",
        )
        .unwrap();

        register_codex_config(&temp_ws).unwrap();
        register_codex_config(&temp_ws).unwrap();

        let parsed: toml::Table =
            toml::from_str(&std::fs::read_to_string(temp_ws.join(".codex/config.toml")).unwrap())
                .unwrap();
        assert_eq!(
            parsed.get("model").and_then(toml::Value::as_str),
            Some("o4-mini")
        );
        assert_eq!(
            parsed
                .get("mcp_servers")
                .and_then(|v| v.get("other"))
                .and_then(|v| v.get("command"))
                .and_then(toml::Value::as_str),
            Some("other-mcp")
        );
        assert_eq!(
            parsed
                .get("mcp_servers")
                .and_then(|v| v.get("torqkage"))
                .and_then(|v| v.get("command"))
                .and_then(toml::Value::as_str),
            Some("tkg-mcp")
        );

        _ = std::fs::remove_dir_all(&temp_ws);
    }

    #[test]
    fn test_eject_workspace_harnesses_clean_and_preserve_user_keys() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let ws = temp_dir.path();

        // Register all workspace harnesses cleanly from scratch, then eject -> zero files remain.
        register_claude_config(ws).unwrap();
        register_cursor_config(ws).unwrap();
        register_codex_config(ws).unwrap();
        register_gemini_config(ws).unwrap();
        register_github_copilot_config(ws).unwrap();
        register_opencode_config(ws).unwrap();
        register_roo_config(ws).unwrap();
        register_custom_mcp_config(ws).unwrap();

        assert_ne!(eject_claude_config(ws).unwrap(), Vec::<String>::new());
        assert_ne!(eject_cursor_config(ws).unwrap(), Vec::<String>::new());
        assert_ne!(eject_codex_config(ws).unwrap(), Vec::<String>::new());
        assert_ne!(eject_gemini_config(ws).unwrap(), Vec::<String>::new());
        assert_ne!(
            eject_github_copilot_config(ws).unwrap(),
            Vec::<String>::new()
        );
        assert_ne!(eject_opencode_config(ws).unwrap(), Vec::<String>::new());
        assert_ne!(eject_roo_config(ws).unwrap(), Vec::<String>::new());
        assert_ne!(eject_custom_mcp_config(ws).unwrap(), Vec::<String>::new());

        for leftover in [
            ".mcp.json",
            ".claude",
            ".cursor",
            ".codex",
            ".gemini",
            ".github",
            ".vscode",
            "opencode.json",
            ".roo",
            ".tkg",
        ] {
            assert!(!ws.join(leftover).exists(), "{leftover} should be removed");
        }

        // Now verify existing user keys in Codex, Gemini, and Windsurf are preserved.
        std::fs::create_dir_all(ws.join(".codex")).unwrap();
        std::fs::write(
            ws.join(".codex/config.toml"),
            "model = \"o4-mini\"\n\n[mcp_servers.other]\ncommand = \"other\"\n",
        )
        .unwrap();
        register_codex_config(ws).unwrap();
        assert_ne!(eject_codex_config(ws).unwrap(), Vec::<String>::new());
        let codex_after = std::fs::read_to_string(ws.join(".codex/config.toml")).unwrap();
        assert!(codex_after.contains("o4-mini"));
        assert!(codex_after.contains("other"));
        assert!(!codex_after.contains("torqkage"));
        assert!(!ws.join(".codex/hooks.json").exists());

        std::fs::create_dir_all(ws.join(".gemini")).unwrap();
        std::fs::write(
            ws.join(".gemini/settings.json"),
            r#"{"theme":"dark","hooks":{"BeforeTool":[{"matcher":"*","hooks":[{"type":"command","command":"my-guard"}]}]}}"#,
        )
        .unwrap();
        register_gemini_config(ws).unwrap();
        assert_ne!(eject_gemini_config(ws).unwrap(), Vec::<String>::new());
        let gem_after = std::fs::read_to_string(ws.join(".gemini/settings.json")).unwrap();
        assert!(gem_after.contains("theme"));
        assert!(gem_after.contains("my-guard"));
        assert!(!gem_after.contains("tkg-hooks"));
        assert!(!gem_after.contains("torqkage"));

        let windsurf_file = ws.join("windsurf_mcp.json");
        register_windsurf_config_at(&windsurf_file).unwrap();
        assert!(windsurf_file.is_file());
        assert_ne!(
            eject_windsurf_config_at(&windsurf_file).unwrap(),
            Vec::<String>::new()
        );
        assert!(!windsurf_file.exists());
    }
}
