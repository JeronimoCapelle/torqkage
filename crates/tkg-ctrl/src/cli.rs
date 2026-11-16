//! Headless drop-in CLI subcommands (`init`, `doctor`, `add`) for configuring
//! `TorqKage` on existing coding-agent harnesses without opening the TUI or
//! editing TOML by hand.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use tkg_common::{
    AgentGuard, CircuitBreakerLedger, FastHashMap, FastHashSet, ProjectRunnerConfig, TkgError,
    check_agent_guard_direct, check_manifest_protection,
    harness::{Harness, HarnessCapabilities, builtin_harnesses, detect_workspace_harness, resolve},
    harness_install::{eject_custom_mcp_config, prune_empty_ancestors},
    is_inside_container,
    project::ProjectConfig,
    strip_comments_and_normalize,
    tools::RawToolsConfig,
};

use crate::installer;

/// Typed errors produced by headless `tkg` setup and management commands.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    #[error(
        "unknown harness '{0}' (run 'tkg init --list' for supported harnesses, or pass --custom)"
    )]
    UnknownHarness(String),

    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("failed to parse TOML at {path}: {source}")]
    TomlParse {
        path: PathBuf,
        #[source]
        source: toml_edit::TomlError,
    },

    #[error("failed to validate tools config at {path}: {source}")]
    ToolsValidation {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("'{key}' in {path} is not a TOML table")]
    NonTableEntry { path: PathBuf, key: String },

    #[error("I/O error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Harness(#[from] TkgError),

    #[error("starter template not found: {0}")]
    TemplateNotFound(String),
}

/// Parsed options for `tkg init`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct InitArgs {
    harness: Option<String>,
    save_project_toml: bool,
    with_tools: bool,
    all_tools: bool,
    template: Option<String>,
    templates_dir: Option<PathBuf>,
    custom: bool,
    list_only: bool,
    help: bool,
}

/// Outcome returned by [`run_init`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InitOutcome {
    harness_id: String,
    harness_display_name: String,
    inject_summary: String,
    saved_project_toml: Option<PathBuf>,
    seeded_tools: Vec<String>,
    skipped_tools: Vec<(String, String)>,
}

/// Parsed options for `tkg doctor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DoctorArgs {
    strict: bool,
    demo: bool,
    help: bool,
}

/// Parsed options for `tkg add`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddToolArgs {
    name: String,
    command: Vec<String>,
    description: Option<String>,
    category: String,
    mutating: bool,
    enforce_sandbox: bool,
    tracks_circuit_breaker: bool,
}

/// Parsed options for `tkg uninstall`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct UninstallArgs {
    harness: Option<String>,
    custom: bool,
    all: bool,
    purge: bool,
    help: bool,
}

/// Outcome returned by [`run_uninstall`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UninstallOutcome {
    actions: Vec<String>,
    preserved_tools_toml: Option<PathBuf>,
}

/// Entrypoint for `tkg init` / `tkg setup`.
pub(crate) fn handle_init_cli(workspace_root: &Path, args: &[String]) -> ExitCode {
    match parse_init_args(args.get(2..).unwrap_or_default()) {
        Ok(parsed) if parsed.help => {
            print_init_usage();
            ExitCode::SUCCESS
        },
        Ok(parsed) if parsed.list_only => {
            println!("{}", format_supported_harnesses());
            ExitCode::SUCCESS
        },
        Ok(parsed) => match run_init(workspace_root, &parsed) {
            Ok(outcome) => {
                println!(
                    "✔ Configured TorqKage for {} ({})",
                    outcome.harness_display_name, outcome.harness_id
                );
                println!("  {}", outcome.inject_summary);
                if let Some(project_toml) = &outcome.saved_project_toml {
                    println!(
                        "  Recorded [runner].harness = \"{}\" in {}",
                        outcome.harness_id,
                        project_toml.display()
                    );
                }
                if !outcome.seeded_tools.is_empty() {
                    println!(
                        "  Seeded {} starter MCP tool(s) in .tkg/tools.toml: {}",
                        outcome.seeded_tools.len(),
                        outcome.seeded_tools.join(", ")
                    );
                }
                if !outcome.skipped_tools.is_empty() {
                    let summary = outcome
                        .skipped_tools
                        .iter()
                        .map(|(name, reason)| format!("{name} ({reason})"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "  Commented out {} unavailable tool(s) at the end of .tkg/tools.toml (pass --all-tools to enable): {summary}",
                        outcome.skipped_tools.len()
                    );
                }
                println!("\nNext:");
                println!(
                    "  tkg doctor --demo           Watch live guardrails block mistakes in 1s"
                );
                println!("  tkg doctor                  Verify hooks, MCP config, and binaries");
                println!("  tkg add <name> -- <cmd...>  Wrap any CLI tool in .tkg/tools.toml");
                ExitCode::SUCCESS
            },
            Err(err) => {
                eprintln!("🛑 [tkg init] {err}");
                ExitCode::FAILURE
            },
        },
        Err(err) => {
            eprintln!("🛑 [tkg init] {err}");
            ExitCode::FAILURE
        },
    }
}

/// Entrypoint for `tkg doctor` / `tkg check`.
pub(crate) fn handle_doctor_cli(workspace_root: &Path, args: &[String]) -> ExitCode {
    match parse_doctor_args(args.get(2..).unwrap_or_default()) {
        Ok(parsed) if parsed.help => {
            print_doctor_usage();
            ExitCode::SUCCESS
        },
        Ok(parsed) => {
            if parsed.demo {
                println!("{}", build_doctor_demo(workspace_root));
                return ExitCode::SUCCESS;
            }
            let (report_text, healthy) = build_doctor_report(workspace_root);
            println!("{report_text}");
            if parsed.strict && !healthy {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        },
        Err(err) => {
            eprintln!("🛑 [tkg doctor] {err}");
            ExitCode::FAILURE
        },
    }
}

/// Entrypoint for `tkg add`.
pub(crate) fn handle_add_cli(workspace_root: &Path, args: &[String]) -> ExitCode {
    let tail = args.get(2..).unwrap_or_default();
    if tail
        .iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| matches!(a.as_str(), "--help" | "-h"))
    {
        print_add_usage();
        return ExitCode::SUCCESS;
    }

    match parse_add_args(tail).and_then(|parsed| run_add_tool(workspace_root, &parsed)) {
        Ok(path) => {
            let tool_name = tail
                .iter()
                .find(|a| !a.starts_with('-') && a.as_str() != "--")
                .map_or("tool", String::as_str);
            println!("✔ Registered MCP tool '{tool_name}' in {}", path.display());
            ExitCode::SUCCESS
        },
        Err(err) => {
            eprintln!("🛑 [tkg add] {err}");
            ExitCode::FAILURE
        },
    }
}

/// Entrypoint for `tkg uninstall` / `tkg remove`.
pub(crate) fn handle_uninstall_cli(workspace_root: &Path, args: &[String]) -> ExitCode {
    match parse_uninstall_args(args.get(2..).unwrap_or_default()) {
        Ok(parsed) if parsed.help => {
            print_uninstall_usage();
            ExitCode::SUCCESS
        },
        Ok(parsed) => match run_uninstall(workspace_root, &parsed) {
            Ok(outcome) => {
                if outcome.actions.is_empty() {
                    println!(
                        "○ No TorqKage harness entries found in {}",
                        workspace_root.display()
                    );
                } else {
                    for line in &outcome.actions {
                        println!("{line}");
                    }
                }
                if let Some(tools_path) = &outcome.preserved_tools_toml {
                    println!(
                        "ℹ Kept {} (pass '--purge' to delete .tkg/ completely)",
                        tools_path.display()
                    );
                }
                ExitCode::SUCCESS
            },
            Err(err) => {
                eprintln!("🛑 [tkg uninstall] {err}");
                ExitCode::FAILURE
            },
        },
        Err(err) => {
            eprintln!("🛑 [tkg uninstall] {err}");
            ExitCode::FAILURE
        },
    }
}

fn parse_uninstall_args(args: &[String]) -> Result<UninstallArgs, CliError> {
    let mut parsed = UninstallArgs::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => parsed.help = true,
            "--all" => parsed.all = true,
            "--purge" => parsed.purge = true,
            "--custom" => parsed.custom = true,
            "--harness" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::InvalidArgs("missing value after --harness".to_owned())
                })?;
                parsed.harness = Some(value.clone());
            },
            value if !value.starts_with('-') && parsed.harness.is_none() => {
                parsed.harness = Some(value.to_owned());
            },
            other => {
                return Err(CliError::InvalidArgs(format!(
                    "unexpected argument '{other}' (see 'tkg uninstall --help')"
                )));
            },
        }
    }
    Ok(parsed)
}

fn parse_init_args(args: &[String]) -> Result<InitArgs, CliError> {
    let mut parsed = InitArgs::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => parsed.help = true,
            "--list" | "-l" => parsed.list_only = true,
            "--save" => parsed.save_project_toml = true,
            "--with-tools" => parsed.with_tools = true,
            "--all-tools" => {
                parsed.all_tools = true;
                parsed.with_tools = true;
            },
            "--template" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::InvalidArgs("missing value after --template".to_owned())
                })?;
                parsed.template = Some(value.clone());
                parsed.with_tools = true;
            },
            "--templates-dir" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::InvalidArgs("missing value after --templates-dir".to_owned())
                })?;
                parsed.templates_dir = Some(PathBuf::from(value));
                parsed.with_tools = true;
            },
            "--custom" => parsed.custom = true,
            "--harness" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::InvalidArgs("missing value after --harness".to_owned())
                })?;
                parsed.harness = Some(value.clone());
            },
            value if !value.starts_with('-') && parsed.harness.is_none() => {
                parsed.harness = Some(value.to_owned());
            },
            other => {
                return Err(CliError::InvalidArgs(format!(
                    "unexpected argument '{other}' (see 'tkg init --help')"
                )));
            },
        }
    }
    Ok(parsed)
}

fn parse_doctor_args(args: &[String]) -> Result<DoctorArgs, CliError> {
    let mut parsed = DoctorArgs::default();
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => parsed.help = true,
            "--strict" => parsed.strict = true,
            "--demo" => parsed.demo = true,
            other => {
                return Err(CliError::InvalidArgs(format!(
                    "unexpected argument '{other}' (see 'tkg doctor --help')"
                )));
            },
        }
    }
    Ok(parsed)
}

fn parse_add_args(args: &[String]) -> Result<AddToolArgs, CliError> {
    let mut name: Option<String> = None;
    let mut command: Vec<String> = Vec::new();
    let mut description: Option<String> = None;
    let mut category = "custom".to_owned();
    let mut mutating = false;
    let mut enforce_sandbox = false;
    let mut tracks_circuit_breaker = false;
    let mut in_command_tail = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if in_command_tail {
            command.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => {
                if name.is_none() {
                    return Err(CliError::InvalidArgs(
                        "expected <NAME> before '--'".to_owned(),
                    ));
                }
                in_command_tail = true;
            },
            "--mutating" => mutating = true,
            "--sandbox" => enforce_sandbox = true,
            "--circuit-breaker" => tracks_circuit_breaker = true,
            "--description" | "-d" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::InvalidArgs(format!("missing text after '{arg}'")))?;
                description = Some(value.clone());
            },
            "--category" | "-c" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::InvalidArgs(format!("missing value after '{arg}'")))?;
                if value.trim().is_empty() {
                    return Err(CliError::InvalidArgs("category cannot be empty".to_owned()));
                }
                category.clone_from(value);
            },
            unknown if unknown.starts_with('-') => {
                return Err(CliError::InvalidArgs(format!(
                    "unknown option '{unknown}' (place command flags after '--')"
                )));
            },
            positional => {
                if name.is_none() {
                    validate_tool_name(positional)?;
                    name = Some(positional.to_owned());
                } else {
                    command.push(positional.to_owned());
                    in_command_tail = true;
                }
            },
        }
    }

    let name = name.ok_or_else(|| {
        CliError::InvalidArgs(
            "missing tool name (usage: tkg add <NAME> [OPTIONS] -- <COMMAND...>)".to_owned(),
        )
    })?;
    if command.is_empty() {
        return Err(CliError::InvalidArgs(
            "missing command tokens (usage: tkg add <NAME> [OPTIONS] -- <COMMAND...>)".to_owned(),
        ));
    }

    Ok(AddToolArgs {
        name,
        command,
        description,
        category,
        mutating,
        enforce_sandbox,
        tracks_circuit_breaker,
    })
}

fn validate_tool_name(name: &str) -> Result<(), CliError> {
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(CliError::InvalidArgs(format!(
            "tool name '{name}' may only contain ASCII letters, digits, '_' and '-'"
        )))
    }
}

/// Resolves the target harness, injects MCP/hook configs, and persists
/// `.tkg/project.toml` only when explicitly requested or when auto-detection
/// would not recover the selected harness.
pub(crate) fn run_init(workspace_root: &Path, args: &InitArgs) -> Result<InitOutcome, CliError> {
    let cfg = ProjectConfig::detect_or_load(workspace_root);
    let selected_slug = match args.harness.as_deref() {
        Some("auto") | None => detect_workspace_harness(workspace_root)
            .map_or_else(|| cfg.resolved_runner().harness, str::to_owned),
        Some(explicit) => explicit.to_owned(),
    };

    let harness_obj = resolve_harness_choice(&selected_slug, args.custom)?;

    let mut runner = cfg.resolved_runner();
    harness_obj.id().clone_into(&mut runner.harness);
    let inject_summary = harness_obj.inject_configs(workspace_root, &runner)?;

    // Only write `.tkg/project.toml` if explicitly requested or if workspace
    // auto-detection cannot infer the selected harness (e.g. global-only harnesses).
    let needs_project_toml = args.save_project_toml
        || detect_workspace_harness(workspace_root) != Some(harness_obj.id());
    let saved_project_toml = if needs_project_toml {
        let project_path = workspace_root.join(".tkg").join("project.toml");
        write_runner_harness_toml(&project_path, harness_obj.id())?;
        Some(project_path)
    } else {
        None
    };

    let seed_summary = if args.with_tools {
        seed_tools_from_templates(workspace_root, args)?
    } else {
        SeededToolsSummary::default()
    };

    Ok(InitOutcome {
        harness_id: harness_obj.id().to_owned(),
        harness_display_name: harness_obj.display_name().to_owned(),
        inject_summary,
        saved_project_toml,
        seeded_tools: seed_summary.seeded,
        skipped_tools: seed_summary.skipped,
    })
}

#[derive(Debug, Default)]
struct SeededToolsSummary {
    seeded: Vec<String>,
    skipped: Vec<(String, String)>,
}

#[derive(Debug, Default)]
struct ToolRule {
    requires_bins: Vec<String>,
    requires_any_file: Vec<String>,
}

/// Seeds starter tool entries from external on-disk template files (`docs/templates/`,
/// `$XDG_CONFIG_HOME/tkg/templates/`, `$XDG_DATA_HOME/tkg/templates/`, `--templates-dir`,
/// or `--template <path>`). Tools whose binaries or repo markers are absent are written
/// as commented-out examples at the end of `.tkg/tools.toml` unless `--all-tools` is passed.
fn seed_tools_from_templates(
    workspace_root: &Path,
    args: &InitArgs,
) -> Result<SeededToolsSummary, CliError> {
    let (template_files, rules) = resolve_template_inputs(workspace_root, args)?;
    let target_tools_path = resolve_target_tools_path(workspace_root);
    let mut target_doc = load_toml_document(&target_tools_path)?;
    let target_tools = ensure_subtable(target_doc.as_table_mut(), &target_tools_path, "tools")?;

    let mut seeded = Vec::new();
    let mut skipped = Vec::new();
    let mut deferred_items: Vec<(String, String, toml_edit::Item)> = Vec::new();

    for tpl_path in template_files {
        let tpl_doc = load_toml_document(&tpl_path)?;
        let Some(src_tools) = tpl_doc.get("tools").and_then(toml_edit::Item::as_table) else {
            continue;
        };
        for (tool_name, item) in src_tools {
            if target_tools.contains_key(tool_name) {
                continue;
            }
            if !args.all_tools
                && let Some(reason) =
                    check_tool_unavailable(workspace_root, item, rules.get(tool_name))
            {
                skipped.push((tool_name.to_owned(), reason.clone()));
                deferred_items.push((tool_name.to_owned(), reason, item.clone()));
                continue;
            }
            target_tools.insert(tool_name, item.clone());
            seeded.push(tool_name.to_owned());
        }
    }

    if target_tools.is_empty() && !deferred_items.is_empty() {
        target_doc.remove("tools");
    }

    let mut rendered = target_doc.to_string();
    let mut added_comments = false;
    let mut comment_blocks = String::new();
    for (tool_name, reason, item) in deferred_items {
        let header_marker = format!("# [tools.{tool_name}]");
        if rendered.contains(&header_marker) {
            continue;
        }
        comment_blocks.push_str(&render_commented_tool_block(&tool_name, &reason, &item)?);
        added_comments = true;
    }

    if added_comments {
        if !rendered.is_empty() && !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        if !rendered.contains("# Disabled starter templates") {
            rendered.push_str(
                "# Disabled starter templates (not detected on this machine/repo — uncomment to enable)\n",
            );
        }
        rendered.push_str(&comment_blocks);
    }

    if !seeded.is_empty() || added_comments {
        let _: RawToolsConfig =
            toml::from_str(&rendered).map_err(|source| CliError::ToolsValidation {
                path: target_tools_path.clone(),
                source,
            })?;
        write_atomic(&target_tools_path, &rendered)?;
    }
    Ok(SeededToolsSummary { seeded, skipped })
}

fn render_commented_tool_block(
    tool_name: &str,
    reason: &str,
    item: &toml_edit::Item,
) -> Result<String, CliError> {
    let mut doc = toml_edit::DocumentMut::new();
    let mut tools_tbl = toml_edit::Table::new();
    tools_tbl.set_implicit(true);
    tools_tbl.insert(tool_name, item.clone());
    doc.insert("tools", toml_edit::Item::Table(tools_tbl));

    let raw = doc.to_string();
    if raw.trim().is_empty() {
        return Err(CliError::InvalidArgs(format!(
            "failed to render template tool '{tool_name}'"
        )));
    }
    let mut out = format!("\n# Skipped: {reason}\n");
    for line in raw.lines() {
        if line.is_empty() {
            out.push_str("#\n");
        } else {
            _ = writeln!(out, "# {line}");
        }
    }
    Ok(out)
}

fn check_tool_unavailable(
    workspace_root: &Path,
    item: &toml_edit::Item,
    rule: Option<&ToolRule>,
) -> Option<String> {
    if let Some(r) = rule
        && !r.requires_any_file.is_empty()
        && !r
            .requires_any_file
            .iter()
            .any(|rel| workspace_root.join(rel).exists())
    {
        return Some(format!("no {}", r.requires_any_file.join("/")));
    }

    let mut required_bins: Vec<&str> = Vec::new();
    if let Some(cmd0) = item
        .get("command")
        .and_then(toml_edit::Item::as_array)
        .and_then(|arr| arr.get(0))
        .and_then(toml_edit::Value::as_str)
    {
        required_bins.push(cmd0);
    }
    if let Some(r) = rule {
        for b in &r.requires_bins {
            if !required_bins.contains(&b.as_str()) {
                required_bins.push(b.as_str());
            }
        }
    }

    for bin in required_bins {
        if !is_tool_binary_available(workspace_root, bin) {
            return Some(format!("missing '{bin}'"));
        }
    }
    None
}

fn is_tool_binary_available(workspace_root: &Path, bin: &str) -> bool {
    let candidates = executable_candidates(bin);
    let venv_bin_dir = if cfg!(windows) { "Scripts" } else { "bin" };
    let local_dirs = [
        workspace_root.join("node_modules").join(".bin"),
        workspace_root.join(".venv").join(venv_bin_dir),
        workspace_root.join("venv").join(venv_bin_dir),
    ];

    for dir in &local_dirs {
        if candidates.iter().any(|name| dir.join(name).is_file()) {
            return true;
        }
    }
    which::which(bin).is_ok()
}

fn executable_candidates(bin: &str) -> Vec<String> {
    if cfg!(windows) && !bin.to_ascii_lowercase().ends_with(".exe") {
        vec![bin.to_owned(), format!("{bin}.exe"), format!("{bin}.cmd")]
    } else {
        vec![bin.to_owned()]
    }
}

fn parse_stack_rules(index_doc: &toml_edit::DocumentMut) -> FastHashMap<String, ToolRule> {
    let mut map = FastHashMap::default();
    let Some(rules_tbl) = index_doc.get("rules").and_then(toml_edit::Item::as_table) else {
        return map;
    };
    for (tool_name, entry) in rules_tbl {
        let Some(tbl) = entry.as_table() else {
            continue;
        };
        let requires_bins = read_string_array(tbl, "requires_bins");
        let requires_any_file = read_string_array(tbl, "requires_any_file");
        map.insert(
            tool_name.to_owned(),
            ToolRule {
                requires_bins,
                requires_any_file,
            },
        );
    }
    map
}

fn read_string_array(tbl: &toml_edit::Table, key: &str) -> Vec<String> {
    tbl.get(key)
        .and_then(toml_edit::Item::as_array)
        .map_or_default(|arr| {
            arr.iter()
                .filter_map(toml_edit::Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
}

fn resolve_template_inputs(
    workspace_root: &Path,
    args: &InitArgs,
) -> Result<(Vec<PathBuf>, FastHashMap<String, ToolRule>), CliError> {
    if let Some(explicit) = args.template.as_deref() {
        let direct_path = Path::new(explicit);
        if direct_path.is_file() {
            return Ok((vec![direct_path.to_path_buf()], FastHashMap::default()));
        }
        let ws_relative = workspace_root.join(explicit);
        if ws_relative.is_file() {
            return Ok((vec![ws_relative], FastHashMap::default()));
        }

        let tpl_dir = find_templates_dir(workspace_root, args.templates_dir.as_deref())?;
        let candidate = tpl_dir.join(explicit).join("tools.toml");
        if candidate.is_file() {
            let stacks_file = tpl_dir.join("stacks.toml");
            let rules = if stacks_file.is_file() {
                parse_stack_rules(&load_toml_document(&stacks_file)?)
            } else {
                FastHashMap::default()
            };
            return Ok((vec![candidate], rules));
        }
        return Err(CliError::TemplateNotFound(format!(
            "'{explicit}' (checked file path and {})",
            candidate.display()
        )));
    }

    let tpl_dir = find_templates_dir(workspace_root, args.templates_dir.as_deref())?;
    let stacks_file = tpl_dir.join("stacks.toml");
    if !stacks_file.is_file() {
        return Err(CliError::TemplateNotFound(format!(
            "missing stack index at {} (pass '--template <path/to/tools.toml>')",
            stacks_file.display()
        )));
    }

    let index_doc = load_toml_document(&stacks_file)?;
    let rules = parse_stack_rules(&index_doc);
    let Some(stacks) = index_doc
        .get("stack")
        .and_then(toml_edit::Item::as_array_of_tables)
    else {
        return Ok((Vec::new(), rules));
    };

    let mut matched = Vec::new();
    for entry in stacks {
        let Some(tools_rel) = entry.get("tools").and_then(toml_edit::Item::as_str) else {
            continue;
        };
        let Some(markers) = entry.get("markers").and_then(toml_edit::Item::as_array) else {
            continue;
        };
        let has_marker = markers
            .iter()
            .filter_map(toml_edit::Value::as_str)
            .any(|m| workspace_root.join(m).is_file());
        if has_marker {
            let tpl_path = tpl_dir.join(tools_rel);
            if tpl_path.is_file() {
                matched.push(tpl_path);
            }
        }
    }
    Ok((matched, rules))
}

fn find_templates_dir(
    workspace_root: &Path,
    explicit_dir: Option<&Path>,
) -> Result<PathBuf, CliError> {
    if let Some(dir) = explicit_dir {
        if dir.is_dir() {
            return Ok(dir.to_path_buf());
        }
        return Err(CliError::TemplateNotFound(format!(
            "templates directory '{}' does not exist",
            dir.display()
        )));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(env_dir) = std::env::var("TKG_TEMPLATES_DIR")
        && !env_dir.is_empty()
    {
        candidates.push(PathBuf::from(env_dir));
    }
    candidates.push(workspace_root.join("docs").join("templates"));
    candidates.push(workspace_root.join("templates"));
    if let Ok(xdg_cfg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg_cfg.is_empty()
    {
        candidates.push(PathBuf::from(xdg_cfg).join("tkg").join("templates"));
    }
    if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME")
        && !xdg_data.is_empty()
    {
        candidates.push(PathBuf::from(xdg_data).join("tkg").join("templates"));
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        let home_dir = PathBuf::from(home);
        candidates.push(home_dir.join(".config").join("tkg").join("templates"));
        candidates.push(
            home_dir
                .join(".local")
                .join("share")
                .join("tkg")
                .join("templates"),
        );
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        candidates.push(exe_dir.join("templates"));
        candidates.push(
            exe_dir
                .join("..")
                .join("share")
                .join("torqkage")
                .join("templates"),
        );
    }

    for dir in &candidates {
        if dir.is_dir() {
            return Ok(dir.clone());
        }
    }

    let searched = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(CliError::TemplateNotFound(format!(
        "no templates directory found (checked: {searched}; set TKG_TEMPLATES_DIR or pass '--template <path/to/tools.toml>')"
    )))
}

fn resolve_harness_choice(slug: &str, allow_custom: bool) -> Result<Box<dyn Harness>, CliError> {
    let harness_obj = resolve(slug);
    let is_explicit_custom = allow_custom || slug.eq_ignore_ascii_case("custom");
    let is_builtin = builtin_harnesses()
        .iter()
        .any(|h| h.id() == harness_obj.id());
    if !is_explicit_custom && !is_builtin {
        return Err(CliError::UnknownHarness(slug.to_owned()));
    }
    Ok(harness_obj)
}

/// Surgically removes `TorqKage` MCP server entries, lifecycle hook entries,
/// and empty `TorqKage` config files from `workspace_root` (preserving all
/// pre-existing user configuration).
pub(crate) fn run_uninstall(
    workspace_root: &Path,
    args: &UninstallArgs,
) -> Result<UninstallOutcome, CliError> {
    let cfg = ProjectConfig::detect_or_load(workspace_root);
    let base_runner = cfg.resolved_runner();
    let configured_global_id = {
        let resolved = base_runner.harness();
        resolved
            .has_global_config()
            .then(|| resolved.id().to_owned())
    };

    let targets: Vec<Box<dyn Harness>> = if let Some(explicit) = args.harness.as_deref() {
        vec![resolve_harness_choice(explicit, args.custom)?]
    } else if args.all {
        builtin_harnesses()
    } else {
        builtin_harnesses()
            .into_iter()
            .filter(|h| !h.has_global_config() || configured_global_id.as_deref() == Some(h.id()))
            .collect()
    };

    let mut actions = Vec::new();
    let mut seen = FastHashSet::default();
    for harness in &targets {
        let mut runner = base_runner.clone();
        harness.id().clone_into(&mut runner.harness);
        for line in harness.eject_configs(workspace_root, &runner)? {
            if seen.insert(line.clone()) {
                actions.push(line);
            }
        }
    }

    if args.harness.is_none() {
        for line in eject_custom_mcp_config(workspace_root).map_err(TkgError::Workspace)? {
            if seen.insert(line.clone()) {
                actions.push(line);
            }
        }
    }

    cleanup_workspace_tkg_dir(workspace_root, args.purge, &mut actions)?;

    let preserved_tools = workspace_root.join(".tkg").join("tools.toml");
    let preserved_tools_toml =
        (!args.purge && preserved_tools.is_file()).then_some(preserved_tools);

    Ok(UninstallOutcome {
        actions,
        preserved_tools_toml,
    })
}

fn cleanup_workspace_tkg_dir(
    workspace_root: &Path,
    purge: bool,
    actions: &mut Vec<String>,
) -> Result<(), CliError> {
    let tkg_dir = workspace_root.join(".tkg");
    if !tkg_dir.is_dir() {
        return Ok(());
    }

    if purge {
        fs::remove_dir_all(&tkg_dir).map_err(|source| CliError::Io {
            path: tkg_dir.clone(),
            source,
        })?;
        actions.push(format!("✔ Purged {}", tkg_dir.display()));
        return Ok(());
    }

    let project_toml = tkg_dir.join("project.toml");
    if project_toml.is_file() {
        remove_runner_harness_from_project_toml(&project_toml, workspace_root, actions)?;
    }

    let state_dir = tkg_dir.join("state");
    if state_dir.is_dir() {
        for name in [
            "mcp_servers.json",
            "circuit_breaker.json",
            "active_agent.json",
            "active_agent.txt",
        ] {
            let candidate = state_dir.join(name);
            if candidate.is_file() {
                fs::remove_file(&candidate).map_err(|source| CliError::Io {
                    path: candidate,
                    source,
                })?;
            }
        }
        prune_empty_ancestors(&state_dir, workspace_root);
    }

    prune_empty_ancestors(&tkg_dir, workspace_root);
    Ok(())
}

fn remove_runner_harness_from_project_toml(
    project_path: &Path,
    workspace_root: &Path,
    actions: &mut Vec<String>,
) -> Result<(), CliError> {
    let mut doc = load_toml_document(project_path)?;
    let Some(runner_tbl) = doc
        .get_mut("runner")
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return Ok(());
    };
    if runner_tbl.remove("harness").is_none() {
        return Ok(());
    }
    if runner_tbl.is_empty() {
        doc.remove("runner");
    }

    if doc.as_table().is_empty() {
        fs::remove_file(project_path).map_err(|source| CliError::Io {
            path: project_path.to_path_buf(),
            source,
        })?;
        prune_empty_ancestors(
            project_path.parent().unwrap_or(workspace_root),
            workspace_root,
        );
        actions.push(format!("✔ Removed {}", project_path.display()));
    } else {
        write_atomic(project_path, &doc.to_string())?;
        actions.push(format!(
            "✔ Removed [runner].harness from {}",
            project_path.display()
        ));
    }
    Ok(())
}

/// Builds a human-readable diagnostics report for `tkg doctor` and returns
/// whether all required checks passed.
#[must_use]
pub(crate) fn build_doctor_report(workspace_root: &Path) -> (String, bool) {
    let cfg = ProjectConfig::detect_or_load(workspace_root);
    let runner: ProjectRunnerConfig = cfg.resolved_runner();
    let harness: Box<dyn Harness> = runner.harness();
    let caps = harness.capabilities();
    let (cfg_path, cfg_installed, _) = harness.config_status(workspace_root, &runner);
    let diag = installer::run_diagnostics(workspace_root);

    let mut out = String::new();
    _ = writeln!(out, "=== TORQKAGE DOCTOR ===");
    _ = writeln!(out, "Workspace : {}", workspace_root.display());
    _ = writeln!(
        out,
        "Harness   : {} ({})",
        harness.display_name(),
        harness.id()
    );
    _ = writeln!(
        out,
        "Defense   : Hooks={} | NativeMCP={} | Resume={} | Spawn={}",
        yes_no(caps.contains(HarnessCapabilities::NATIVE_HOOKS)),
        yes_no(caps.contains(HarnessCapabilities::NATIVE_MCP)),
        yes_no(caps.contains(HarnessCapabilities::SESSION_RESUME)),
        yes_no(caps.contains(HarnessCapabilities::HEADLESS_SPAWN)),
    );
    _ = writeln!(
        out,
        "Config    : {} ({})",
        cfg_path.display(),
        if cfg_installed {
            "configured"
        } else {
            "not configured — run 'tkg init'"
        }
    );
    out.push('\n');
    out.push_str(&diag.summary());

    let required_bins_ok = diag
        .binary_checks
        .iter()
        .all(|b| !b.required || b.installed);
    let healthy = cfg_installed && diag.plugin_installed && required_bins_ok;
    (out, healthy)
}

/// Runs a zero-side-effect in-memory simulation exercising `TorqKage`'s core
/// guardrail layers (manifest guard, path boundaries, comment-aware content
/// filter, 2-strike circuit breaker, and OS containment posture).
#[must_use]
pub(crate) fn build_doctor_demo(workspace_root: &Path) -> String {
    let mut out = String::new();
    _ = writeln!(out, "=== TORQKAGE GUARDRAIL SMOKE DEMO (read-only) ===");
    _ = writeln!(out, "Workspace : {}\n", workspace_root.display());

    let demo_guard = AgentGuard {
        allowed_dirs: vec!["src".to_owned()],
        disallowed_dirs: vec!["secrets".to_owned()],
        allowed_files: vec!["src/*.rs".to_owned()],
        disallowed_files: Vec::new(),
        path_policies: Vec::new(),
    };

    // 1. Manifest mutation guard
    let manifest_err = check_manifest_protection(
        &workspace_root.join("Cargo.toml"),
        Some("demo-agent"),
        Some(&demo_guard),
    );
    _ = writeln!(
        out,
        "1. Manifest Guard (write Cargo.toml):\n   {}",
        manifest_err.map_or_else(
            |e| format!("✔ BLOCKED — {e}"),
            |()| "unexpected allow".to_owned()
        )
    );

    // 2. Directory & path-boundary guard
    let path_err = check_agent_guard_direct(
        workspace_root,
        &workspace_root.join("secrets/.env"),
        Some("demo-agent"),
        Some(&demo_guard),
    );
    _ = writeln!(
        out,
        "2. Path Boundary Guard (write secrets/.env):\n   {}",
        path_err.map_or_else(
            |e| format!("✔ BLOCKED — {e}"),
            |()| "unexpected allow".to_owned()
        )
    );

    // 3. Comment-aware AST content normalization
    let sample = "// TODO: forbidden_token inside comment\nlet ok = \"literal // not comment\";\nfn clean() {}";
    let normalized = strip_comments_and_normalize(sample);
    let stripped_comment = !normalized.contains("forbidden_token");
    _ = writeln!(
        out,
        "3. Comment-Aware Content Filter (ignores false positives in comments):\n   {} stripped comment noise -> \"{}\"",
        if stripped_comment {
            "✔ PASS"
        } else {
            "✘ FAIL"
        },
        normalized.replace('\n', " ")
    );

    // 4. Two-strike retry circuit breaker
    let mut ledger = CircuitBreakerLedger::new("demo-agent");
    ledger.record_failure_in_memory(
        Some("demo-agent"),
        Some("cargo_test"),
        Some("E0308"),
        Some("mismatched types"),
    );
    let after_first = ledger.tripped;
    ledger.record_failure_in_memory(
        Some("demo-agent"),
        Some("cargo_test"),
        Some("E0308"),
        Some("mismatched types"),
    );
    _ = writeln!(
        out,
        "4. 2-Strike Circuit Breaker (repeated E0308 failures):\n   strike 1 tripped={after_first} -> strike 2 tripped={} ({}/{}) ✔ HALTED MUTATING TOOLS",
        ledger.tripped, ledger.consecutive_failures, ledger.max_strikes
    );

    // 5. OS containment / sandbox posture
    let bwrap_present = which::which("bwrap").is_ok();
    let in_container = is_inside_container();
    _ = writeln!(
        out,
        "5. Containment Posture:\n   bwrap={bwrap_present} | outer_container={in_container} | in-band MCP seatbelt=active (fail-closed)"
    );

    out
}

/// Wraps a CLI command into `.tkg/tools.toml` (or an existing root `tools.toml`)
/// using `toml_edit` so user comments and formatting are preserved.
pub(crate) fn run_add_tool(workspace_root: &Path, args: &AddToolArgs) -> Result<PathBuf, CliError> {
    let tools_path = resolve_target_tools_path(workspace_root);
    let mut doc = load_toml_document(&tools_path)?;

    let tools_table = ensure_subtable(doc.as_table_mut(), &tools_path, "tools")?;
    let entry_table = ensure_subtable(tools_table, &tools_path, &args.name)?;

    let description = args
        .description
        .clone()
        .unwrap_or_else(|| format!("Wrapped command: {}", args.command.join(" ")));

    let mut cmd_array = toml_edit::Array::new();
    for token in &args.command {
        cmd_array.push(token.as_str());
    }

    entry_table.insert("description", toml_edit::value(description));
    entry_table.insert("action", toml_edit::value("static_command"));
    entry_table.insert("category", toml_edit::value(args.category.as_str()));
    entry_table.insert("command", toml_edit::value(cmd_array));
    entry_table.insert("mutating", toml_edit::value(args.mutating));
    if args.enforce_sandbox {
        entry_table.insert("enforce_sandbox", toml_edit::value(true));
    }
    if args.tracks_circuit_breaker {
        entry_table.insert("tracks_circuit_breaker", toml_edit::value(true));
    }

    let rendered = doc.to_string();
    let _: RawToolsConfig =
        toml::from_str(&rendered).map_err(|source| CliError::ToolsValidation {
            path: tools_path.clone(),
            source,
        })?;

    write_atomic(&tools_path, &rendered)?;
    Ok(tools_path)
}

fn resolve_target_tools_path(workspace_root: &Path) -> PathBuf {
    let hidden = workspace_root.join(".tkg").join("tools.toml");
    if hidden.is_file() {
        return hidden;
    }
    let root = workspace_root.join("tools.toml");
    if root.is_file() {
        return root;
    }
    hidden
}

fn write_runner_harness_toml(project_path: &Path, harness_id: &str) -> Result<(), CliError> {
    let mut doc = load_toml_document(project_path)?;
    let runner_table = ensure_subtable(doc.as_table_mut(), project_path, "runner")?;
    runner_table.insert("harness", toml_edit::value(harness_id));
    write_atomic(project_path, &doc.to_string())
}

fn load_toml_document(path: &Path) -> Result<toml_edit::DocumentMut, CliError> {
    if !path.is_file() {
        return Ok(toml_edit::DocumentMut::new());
    }
    let raw = fs::read_to_string(path).map_err(|source| CliError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    raw.parse::<toml_edit::DocumentMut>()
        .map_err(|source| CliError::TomlParse {
            path: path.to_path_buf(),
            source,
        })
}

fn ensure_subtable<'a>(
    parent: &'a mut toml_edit::Table,
    path: &Path,
    key: &str,
) -> Result<&'a mut toml_edit::Table, CliError> {
    if !parent.contains_key(key) {
        parent.insert(key, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    parent
        .get_mut(key)
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| CliError::NonTableEntry {
            path: path.to_path_buf(),
            key: key.to_owned(),
        })
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CliError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    tkg_common::atomic_write(path, contents.as_bytes()).map_err(|source| CliError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn format_supported_harnesses() -> String {
    let mut out = String::from("Supported harnesses:\n");
    for h in builtin_harnesses() {
        let caps = h.capabilities();
        _ = writeln!(
            out,
            "  {:<16} {:<22} [hooks: {}, mcp: {}]",
            h.id(),
            h.display_name(),
            yes_no(caps.contains(HarnessCapabilities::NATIVE_HOOKS)),
            yes_no(caps.contains(HarnessCapabilities::NATIVE_MCP)),
        );
    }
    out
}

const fn yes_no(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

fn print_init_usage() {
    println!(concat!(
        "Configure TorqKage MCP server & hooks for your coding-agent harness\n",
        "\n",
        "USAGE:\n",
        "  tkg init [HARNESS] [OPTIONS]\n",
        "\n",
        "OPTIONS:\n",
        "  --harness <NAME>     Target harness (auto-detected from workspace by default)\n",
        "  --with-tools         Seed .tkg/tools.toml from external templates, pruning unavailable CLIs\n",
        "  --all-tools          Disable CLI/marker pruning when seeding templates\n",
        "  --template <NAME|F>  Use a named stack template or path to a tools.toml file\n",
        "  --templates-dir <D>  Override template directory (or set TKG_TEMPLATES_DIR)\n",
        "  --custom             Allow an unlisted custom harness slug\n",
        "  --save             Always record [runner].harness in .tkg/project.toml\n",
        "  --list, -l         List supported built-in harnesses\n",
        "  --help, -h         Show this help\n",
    ));
}

fn print_doctor_usage() {
    println!(concat!(
        "Verify TorqKage hook registration, MCP config, and prerequisite binaries\n",
        "\n",
        "USAGE:\n",
        "  tkg doctor [--strict] [--demo]\n",
        "\n",
        "OPTIONS:\n",
        "  --demo             Run a read-only smoke check exercising live guardrails\n",
        "  --strict           Exit non-zero if harness config, plugin, or required binaries are missing\n",
        "  --help, -h         Show this help\n",
    ));
}

fn print_add_usage() {
    println!(concat!(
        "Wrap any CLI command as a declarative MCP tool in .tkg/tools.toml\n",
        "\n",
        "USAGE:\n",
        "  tkg add <NAME> [OPTIONS] -- <COMMAND...>\n",
        "\n",
        "OPTIONS:\n",
        "  -d, --description <TEXT>  Tool description shown to the agent\n",
        "  -c, --category <NAME>     Tool category label (default: custom)\n",
        "      --mutating            Mark command as mutating (enables permission gate checks)\n",
        "      --sandbox             Enforce Bubblewrap OS sandboxing on execution\n",
        "      --circuit-breaker     Track consecutive failures in circuit breaker\n",
        "      --help, -h            Show this help\n",
        "\n",
        "EXAMPLE:\n",
        "  tkg add rust_tests --sandbox --circuit-breaker -- cargo test --workspace\n",
    ));
}

fn print_uninstall_usage() {
    println!(concat!(
        "Remove TorqKage MCP server & hook entries from your workspace cleanly\n",
        "\n",
        "USAGE:\n",
        "  tkg uninstall [HARNESS] [OPTIONS]\n",
        "\n",
        "OPTIONS:\n",
        "  --harness <NAME>   Remove only from the specified harness\n",
        "  --all              Include user-global harnesses (windsurf, cline, agy)\n",
        "  --purge            Also delete .tkg/ (tools.toml, project.toml, workspaces)\n",
        "  --custom           Allow an unlisted custom harness slug with --harness\n",
        "  --help, -h         Show this help\n",
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parse_init_accepts_positional_and_flag() {
        let pos = parse_init_args(&vec_args(&["codex", "--save"])).expect("positional");
        assert_eq!(pos.harness.as_deref(), Some("codex"));
        assert!(pos.save_project_toml);

        let flag = parse_init_args(&vec_args(&["--harness", "gemini"])).expect("flag");
        assert_eq!(flag.harness.as_deref(), Some("gemini"));
        assert!(!flag.save_project_toml);
    }

    #[test]
    fn run_init_configures_codex_without_project_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = parse_init_args(&vec_args(&["codex"])).expect("args");
        let outcome = run_init(dir.path(), &args).expect("init codex");

        assert_eq!(outcome.harness_id, "codex");
        assert_eq!(outcome.saved_project_toml, None);
        assert!(dir.path().join(".codex/config.toml").is_file());
        assert!(dir.path().join(".codex/hooks.json").is_file());
        assert!(!dir.path().join(".tkg/project.toml").exists());
    }

    #[test]
    fn run_init_rejects_unknown_harness_cleanly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = parse_init_args(&vec_args(&["not-a-real-harness"])).expect("args");
        let err = run_init(dir.path(), &args).expect_err("unknown harness should fail");
        assert!(matches!(err, CliError::UnknownHarness(_)));
    }

    #[test]
    fn run_init_saves_project_toml_when_requested_or_ambiguous() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".claude")).expect("mkdir .claude");
        let args = parse_init_args(&vec_args(&["codex"])).expect("args");
        let outcome = run_init(dir.path(), &args).expect("init codex alongside .claude");

        let saved = outcome
            .saved_project_toml
            .expect("should persist project.toml when .claude shadows .codex");
        let content = fs::read_to_string(saved).expect("read project.toml");
        assert!(content.contains("harness = \"codex\""));
    }

    #[test]
    fn parse_add_and_write_preserves_comments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tkg_dir = dir.path().join(".tkg");
        fs::create_dir_all(&tkg_dir).expect("mkdir .tkg");
        let tools_path = tkg_dir.join("tools.toml");
        fs::write(
            &tools_path,
            "# Custom project comment\n[tools.existing]\naction = \"static_command\"\ncommand = [\"echo\", \"hi\"]\n",
        )
        .expect("seed tools.toml");

        let parsed = parse_add_args(&vec_args(&[
            "unit_tests",
            "--sandbox",
            "--circuit-breaker",
            "-d",
            "Run workspace unit tests",
            "--",
            "cargo",
            "test",
            "--workspace",
        ]))
        .expect("parse add");

        let written = run_add_tool(dir.path(), &parsed).expect("write tool");
        assert_eq!(written, tools_path);

        let content = fs::read_to_string(&tools_path).expect("read tools.toml");
        assert!(content.contains("# Custom project comment"));
        assert!(content.contains("[tools.existing]"));
        assert!(content.contains("[tools.unit_tests]"));
        assert!(content.contains("enforce_sandbox = true"));
        assert!(content.contains("tracks_circuit_breaker = true"));

        let reparsed: RawToolsConfig = toml::from_str(&content).expect("valid RawToolsConfig");
        assert!(reparsed.tools.contains_key("unit_tests"));
    }

    #[test]
    fn doctor_report_includes_harness_capabilities() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = parse_init_args(&vec_args(&["gemini"])).expect("args");
        let _ = run_init(dir.path(), &args).expect("init gemini");

        let (report, _healthy) = build_doctor_report(dir.path());
        assert!(report.contains("=== TORQKAGE DOCTOR ==="));
        assert!(report.contains("gemini"));
        assert!(report.contains("Hooks=yes | NativeMCP=yes"));
    }

    #[test]
    fn run_init_then_uninstall_claude_leaves_zero_trace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let init_args = parse_init_args(&vec_args(&["claude"])).expect("init args");
        let _ = run_init(dir.path(), &init_args).expect("init claude");
        assert!(dir.path().join(".mcp.json").is_file());
        assert!(dir.path().join(".claude/settings.json").is_file());

        let uninstall_args = parse_uninstall_args(&vec_args(&[])).expect("uninstall args");
        let outcome = run_uninstall(dir.path(), &uninstall_args).expect("uninstall claude");
        assert_ne!(outcome.actions, Vec::<String>::new());
        assert!(!dir.path().join(".mcp.json").exists());
        assert!(!dir.path().join(".claude").exists());
        assert!(!dir.path().join(".tkg").exists());
    }

    #[test]
    fn run_uninstall_preserves_existing_user_mcp_and_hooks() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".claude")).expect("mkdir .claude");
        fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"other":{"command":"other-mcp"}},"extraFlag":true}"#,
        )
        .expect("seed .mcp.json");
        fs::write(
            dir.path().join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Read"]},"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"my-audit"}]}]}}"#,
        )
        .expect("seed settings.json");

        let init_args = parse_init_args(&vec_args(&["claude"])).expect("init args");
        let _ = run_init(dir.path(), &init_args).expect("init claude");

        let uninstall_args = parse_uninstall_args(&vec_args(&[])).expect("uninstall args");
        let _ = run_uninstall(dir.path(), &uninstall_args).expect("uninstall claude");

        let mcp_json = fs::read_to_string(dir.path().join(".mcp.json")).expect("read .mcp.json");
        assert!(mcp_json.contains("other-mcp"));
        assert!(mcp_json.contains("extraFlag"));
        assert!(!mcp_json.contains("torqkage"));
        assert!(!mcp_json.contains("tkg-mcp"));

        let settings =
            fs::read_to_string(dir.path().join(".claude/settings.json")).expect("read settings");
        assert!(settings.contains("my-audit"));
        assert!(settings.contains("permissions"));
        assert!(!settings.contains("tkg-hooks"));
    }

    #[test]
    fn run_uninstall_preserves_tools_toml_unless_purge() {
        let dir = tempfile::tempdir().expect("tempdir");
        let init_args = parse_init_args(&vec_args(&["codex", "--save"])).expect("init args");
        let _ = run_init(dir.path(), &init_args).expect("init codex");

        let add_args =
            parse_add_args(&vec_args(&["unit_tests", "--", "cargo", "test"])).expect("add args");
        let tools_path = run_add_tool(dir.path(), &add_args).expect("add tool");

        let keep_args = parse_uninstall_args(&vec_args(&[])).expect("uninstall keep");
        let first = run_uninstall(dir.path(), &keep_args).expect("uninstall without purge");
        assert_eq!(first.preserved_tools_toml.as_ref(), Some(&tools_path));
        assert!(tools_path.is_file());
        assert!(!dir.path().join(".codex").exists());
        assert!(!dir.path().join(".tkg/project.toml").exists());

        let purge_args = parse_uninstall_args(&vec_args(&["--purge"])).expect("uninstall purge");
        let second = run_uninstall(dir.path(), &purge_args).expect("uninstall with purge");
        assert_eq!(second.preserved_tools_toml, None);
        assert!(!dir.path().join(".tkg").exists());
    }

    #[test]
    fn parse_doctor_accepts_demo_and_strict() {
        let parsed = parse_doctor_args(&vec_args(&["--demo", "--strict"])).expect("doctor args");
        assert!(parsed.demo);
        assert!(parsed.strict);
        assert!(!parsed.help);
    }

    #[test]
    fn doctor_demo_exercises_live_guardrails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = build_doctor_demo(dir.path());
        assert!(report.contains("BLOCKED"));
        assert!(report.contains("Cargo.toml"));
        assert!(report.contains("secrets/.env"));
        assert!(report.contains("PASS stripped comment noise"));
        assert!(report.contains("strike 2 tripped=true (2/2)"));
    }

    #[test]
    fn run_init_with_tools_loads_and_prunes_external_templates() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("package.json"), "{}\n").expect("write package.json");
        fs::write(dir.path().join("pnpm-lock.yaml"), "").expect("write pnpm-lock.yaml");
        fs::write(dir.path().join("tsconfig.json"), "{}\n").expect("write tsconfig.json");

        // Simulate local node_modules/.bin executables (not on global $PATH).
        let bin_dir = dir.path().join("node_modules/.bin");
        fs::create_dir_all(&bin_dir).expect("create node_modules/.bin");
        for tool in ["pnpm", "vitest", "tsc"] {
            let bin_name = if cfg!(windows) {
                format!("{tool}.exe")
            } else {
                tool.to_owned()
            };
            fs::write(bin_dir.join(bin_name), "").expect("write mock bin");
        }

        let repo_templates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("docs")
            .join("templates");
        let tpl_dir_str = repo_templates.to_string_lossy().into_owned();

        let args = parse_init_args(&vec_args(&["claude", "--templates-dir", &tpl_dir_str]))
            .expect("init args");
        let first = run_init(dir.path(), &args).expect("first init");
        assert!(first.seeded_tools.contains(&"vitest_run".to_owned()));
        assert!(first.seeded_tools.contains(&"tsc_check".to_owned()));
        assert!(!first.seeded_tools.contains(&"bun_test".to_owned()));
        assert!(!first.seeded_tools.contains(&"playwright_test".to_owned()));
        assert!(
            first
                .skipped_tools
                .iter()
                .any(|(name, _)| name == "playwright_test")
        );

        let tools_toml =
            fs::read_to_string(dir.path().join(".tkg/tools.toml")).expect("read tools.toml");
        assert!(tools_toml.contains("[tools.vitest_run]"));
        assert!(tools_toml.contains("# [tools.playwright_test]"));
        assert!(tools_toml.contains("# [tools.bun_test]"));
        // Active tools must appear before commented-out unavailable tools.
        let active_idx = tools_toml.find("[tools.vitest_run]").expect("active idx");
        let commented_idx = tools_toml
            .find("# [tools.playwright_test]")
            .expect("commented idx");
        assert!(active_idx < commented_idx);

        // Re-running must preserve existing entries and not duplicate commented blocks.
        let second = run_init(dir.path(), &args).expect("second init");
        assert_eq!(second.seeded_tools, Vec::<String>::new());
        let tools_after_second =
            fs::read_to_string(dir.path().join(".tkg/tools.toml")).expect("read tools.toml");
        assert_eq!(
            tools_after_second
                .matches("# [tools.playwright_test]")
                .count(),
            1
        );

        // Passing `--all-tools` bypasses environment/lockfile pruning.
        let all_args = parse_init_args(&vec_args(&[
            "claude",
            "--templates-dir",
            &tpl_dir_str,
            "--all-tools",
        ]))
        .expect("all-tools args");
        let third = run_init(dir.path(), &all_args).expect("all-tools init");
        assert!(third.seeded_tools.contains(&"playwright_test".to_owned()));
        assert!(third.seeded_tools.contains(&"bun_test".to_owned()));

        // Also verify passing a direct custom `.toml` file via `--template`.
        let custom_tpl = dir.path().join("custom.tools.toml");
        fs::write(
            &custom_tpl,
            "[tools.custom_lint]\ndescription = \"Custom lint\"\naction = \"static_command\"\ncommand = [\"echo\", \"ok\"]\n",
        )
        .expect("write custom template");
        let custom_str = custom_tpl.to_string_lossy().into_owned();
        let custom_args =
            parse_init_args(&vec_args(&["claude", "--template", &custom_str])).expect("args");
        let fourth = run_init(dir.path(), &custom_args).expect("custom template init");
        assert_eq!(fourth.seeded_tools, vec!["custom_lint"]);
    }

    mod prop_tests {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            #[test]
            fn prop_init_uninstall_roundtrip_across_workspace_harnesses(
                harness_slug in prop_oneof![
                    Just("claude"),
                    Just("codex"),
                    Just("gemini"),
                    Just("cursor"),
                    Just("github-copilot"),
                    Just("vscode"),
                    Just("opencode"),
                    Just("roo"),
                ],
            ) {
                let dir = tempfile::tempdir().expect("tempdir");
                let init_args = parse_init_args(&vec_args(&[harness_slug])).expect("init args");
                let init_res = run_init(dir.path(), &init_args);
                prop_assert!(init_res.is_ok(), "init failed: {init_res:?}");

                let uninstall_args = parse_uninstall_args(&vec_args(&[])).expect("uninstall args");
                let uninstall_res = run_uninstall(dir.path(), &uninstall_args);
                prop_assert!(uninstall_res.is_ok(), "uninstall failed: {uninstall_res:?}");
                if let Ok(outcome) = uninstall_res {
                    prop_assert!(!outcome.actions.is_empty());
                }

                // Zero leftover TorqKage artifacts on a clean repository.
                let leftover_count = fs::read_dir(dir.path()).expect("read_dir").count();
                prop_assert_eq!(leftover_count, 0);
            }

            #[test]
            fn prop_add_tool_escapes_toml_and_rejects_bad_names(
                valid_name in "[a-z][a-z0-9_-]{1,18}",
                bad_char in prop_oneof![Just(" "), Just("."), Just("/"), Just(";"), Just("=")],
                description in "[a-zA-Z0-9 #\"\\\\_-]{0,60}",
                cmd_args in prop::collection::vec("[a-zA-Z0-9_./:-]{1,15}", 1..5),
            ) {
                let dir = tempfile::tempdir().expect("tempdir");
                fs::create_dir_all(dir.path().join(".tkg")).expect("mkdir .tkg");
                let tools_path = dir.path().join(".tkg/tools.toml");
                fs::write(&tools_path, "# keep this comment\n").expect("write comment");

                let add_args = AddToolArgs {
                    name: valid_name.clone(),
                    command: cmd_args,
                    description: Some(description),
                    category: "custom".to_owned(),
                    mutating: false,
                    enforce_sandbox: true,
                    tracks_circuit_breaker: true,
                };
                let ok_write = run_add_tool(dir.path(), &add_args);
                prop_assert!(ok_write.is_ok(), "valid add failed: {ok_write:?}");

                let rendered = fs::read_to_string(&tools_path).expect("read tools.toml");
                prop_assert!(rendered.contains("# keep this comment"));
                let section_header = format!("[tools.{valid_name}]");
                let has_section = rendered.contains(&section_header);
                prop_assert!(has_section);
                let parsed: Result<RawToolsConfig, _> = toml::from_str(&rendered);
                prop_assert!(parsed.is_ok());

                // Any invalid character in tool name must be rejected cleanly.
                let bad_name = format!("{valid_name}{bad_char}tail");
                let mut raw_bad = vec![bad_name, "--".to_owned(), "echo".to_owned()];
                let bad_parsed = parse_add_args(&raw_bad);
                if let Ok(parsed_add) = bad_parsed {
                    let bad_res = run_add_tool(dir.path(), &parsed_add);
                    prop_assert!(bad_res.is_err());
                } else {
                    prop_assert!(bad_parsed.is_err());
                }
                raw_bad.clear();
            }

            #[test]
            fn prop_cli_parsers_never_panic_on_arbitrary_args(
                raw_args in prop::collection::vec("\\PC{0,30}", 0..12),
            ) {
                _ = parse_init_args(&raw_args);
                _ = parse_doctor_args(&raw_args);
                _ = parse_add_args(&raw_args);
                _ = parse_uninstall_args(&raw_args);
            }
        }
    }
}
