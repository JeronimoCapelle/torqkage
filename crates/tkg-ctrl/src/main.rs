//! # tkg
//!
//! Drop-in CLI utility and optional Ratatui (`--features tui`) dashboard for
//! `TorqKage`.
//!
//! ## Overview
//! `tkg` provides headless workspace setup (`init`, `doctor`, `add`),
//! diagnostics, circuit-breaker controls, and agent session launching with
//! pre-flight gatekeeper checks and optional Jujutsu workspace isolation.
//! Compiling with `--features tui` adds the interactive control deck via
//! `tkg ui`.
//!
//! ## Core Architecture
//! - `app`: Optional TUI state machine (`tui` feature).
//! - [`cli`]: Headless `init`, `doctor`, and `add` CLI commands.
//! - [`launcher`]: Pre-launch validator and runner pipeline.
//! - [`model`]: In-memory models for agent manifests and declarative tools.
//! - [`session`]: Reader querying conversation histories for session resumption.
//! - `ui`: Optional Ratatui rendering pipeline (`tui` feature).
//! - [`vcs`]: Jujutsu and Git snapshotting helpers.

#![forbid(unsafe_code)]
#![allow(
    clippy::print_stdout,
    reason = "CLI summary flags emit output to stdout"
)]
#![allow(
    clippy::print_stderr,
    reason = "CLI error diagnostics emit output to stderr"
)]

#[cfg(feature = "tui")]
mod app;
mod cli;
mod installer;
mod launcher;
#[cfg(feature = "tui")]
mod manual;
mod model;
mod session;
mod summary;
#[cfg(feature = "tui")]
mod ui;
mod vcs;

#[cfg(feature = "tui")]
use std::io::{self, stdout};
#[cfg(feature = "tui")]
use std::time::Duration;
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

#[cfg(feature = "tui")]
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
#[cfg(feature = "tui")]
use ratatui::{Terminal, backend::CrosstermBackend};
use tkg_common::WorkspaceResolver;

#[cfg(feature = "tui")]
use crate::app::{ActiveTab, App, FleetFocus};

fn main() -> ExitCode {
    let raw_args: Vec<String> = std::env::args().collect();
    let args = tkg_common::diagnostics::init_stderr_tracing(
        &raw_args,
        tkg_common::diagnostics::DefaultVerbosity::Warn,
    );
    let command = args.get(1).map(String::as_str);

    let workspace_root = WorkspaceResolver::find_root(None).unwrap_or_else(|_| PathBuf::from("."));

    match command {
        Some("init" | "setup") => cli::handle_init_cli(&workspace_root, &args),
        Some("doctor" | "check") => cli::handle_doctor_cli(&workspace_root, &args),
        Some("add") => cli::handle_add_cli(&workspace_root, &args),
        Some("uninstall" | "remove") => cli::handle_uninstall_cli(&workspace_root, &args),
        Some("--ui" | "ui" | "tui" | "deck") => run_tui_or_fail(workspace_root),
        Some("--status" | "status") => {
            let agents = model::discover_agents(&workspace_root);
            let tools = match model::find_tools_config_path(&workspace_root) {
                Some(path) => match model::load_tools_from_file(&path) {
                    Ok(loaded) => loaded,
                    Err(e) => {
                        eprintln!("🛑 [tkg] Failed to load configuration: {e}");
                        return ExitCode::FAILURE;
                    },
                },
                None => Vec::new(),
            };
            summary::print_fleet_status(&agents, &tools);
            ExitCode::SUCCESS
        },
        Some("--validate" | "validate") => {
            if let Err(e) = summary::print_validation_report(&workspace_root) {
                eprintln!("🛑 [tkg] Validation failed: {e}");
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        },
        Some("--launch" | "launch") => {
            let agent_arg = args.get(2).map(String::as_str);
            let has_sandbox = args.iter().any(|a| a == "--sandbox");
            let has_isolated = args.iter().any(|a| a == "--isolated" || a == "--workspace");
            handle_cli_launch(&workspace_root, agent_arg, has_sandbox, has_isolated)
        },
        Some(
            "reset-breaker"
            | "reset-circuit-breaker"
            | "inspect-breaker"
            | "breaker-status"
            | "circuit-breaker",
        ) => handle_breaker_cli(&workspace_root, command.unwrap_or_default()),
        Some("workspace" | "ws") => handle_workspace_cli(&workspace_root, &args),
        Some("--help" | "-h" | "help") => {
            print_cli_usage();
            ExitCode::SUCCESS
        },
        Some("-r" | "--reset-breaker") => handle_breaker_cli(&workspace_root, "reset-breaker"),
        Some(first) if first.starts_with('-') || !first.is_empty() => {
            let has_sandbox = args.iter().any(|a| a == "--sandbox");
            let has_isolated = args.iter().any(|a| a == "--isolated" || a == "--workspace");
            match find_agent_arg(&args) {
                Some(agent_name) => {
                    handle_cli_launch(&workspace_root, Some(agent_name), has_sandbox, has_isolated)
                },
                None if has_sandbox || has_isolated => {
                    eprintln!(
                        "🛑 [tkg] Missing agent name. Usage: tkg <AGENT> [--sandbox] [--isolated]"
                    );
                    ExitCode::FAILURE
                },
                None => {
                    print_cli_usage();
                    ExitCode::SUCCESS
                },
            }
        },
        _ => {
            print_cli_usage();
            ExitCode::SUCCESS
        },
    }
}

/// Resolves the agent name from `-a/--agent <name>` or the first positional
/// (non-flag) argument.
fn find_agent_arg(args: &[String]) -> Option<&str> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "-a" || arg == "--agent" {
            return iter.next().map(String::as_str);
        }
        if !arg.starts_with('-') {
            return Some(arg.as_str());
        }
    }
    None
}

#[cfg(feature = "tui")]
fn run_tui_or_fail(workspace_root: PathBuf) -> ExitCode {
    if let Err(err) = run_tui(workspace_root) {
        eprintln!("🛑 [tkg] TUI error: {err}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(not(feature = "tui"))]
fn run_tui_or_fail(_workspace_root: PathBuf) -> ExitCode {
    eprintln!(
        "🛑 [tkg] Interactive TUI is an optional compile-time feature ('tui').\n\
           Rebuild/install with: cargo install torqkage --features tui\n\
           Or use headless commands: 'tkg init', 'tkg doctor', 'tkg add', 'tkg --status'."
    );
    ExitCode::FAILURE
}

fn handle_cli_launch(
    workspace_root: &Path,
    agent_arg: Option<&str>,
    sandbox: bool,
    isolated: bool,
) -> ExitCode {
    let Some(agent_name) = agent_arg else {
        eprintln!(
            "🛑 [tkg] Missing agent name. Usage: tkg launch <AGENT> [--sandbox] [--isolated]"
        );
        return ExitCode::FAILURE;
    };

    let agents = model::discover_agents(workspace_root);
    let Some(agent) = agents.into_iter().find(|a| a.name == agent_name) else {
        eprintln!("🛑 [tkg] Unknown agent '{agent_name}'. Run 'tkg --status' to list agents.");
        return ExitCode::FAILURE;
    };

    let resume_conv = session::find_agent_session_for_workspace(&agent.name, workspace_root)
        .map(|s| s.conversation_id);

    match launcher::launch_agent_headless(
        workspace_root,
        &agent,
        sandbox,
        isolated,
        resume_conv.as_deref(),
    ) {
        Ok(status) => {
            if status.success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        },
        Err(err) => {
            eprintln!("🛑 [tkg] Launch failed:\n{err}");
            ExitCode::FAILURE
        },
    }
}

fn handle_breaker_cli(workspace_root: &Path, command: &str) -> ExitCode {
    match command {
        "reset-breaker" | "reset-circuit-breaker" => {
            let mut ledger = tkg_common::CircuitBreakerLedger::load(workspace_root);
            if ledger.reset(workspace_root).is_ok() {
                println!(
                    "✅ [tkg] Circuit breaker ledger reset successfully in .tkg/state/circuit_breaker.json"
                );
                ExitCode::SUCCESS
            } else {
                eprintln!("🛑 [tkg] Failed to reset circuit breaker.");
                ExitCode::FAILURE
            }
        },
        _ => {
            let ledger = tkg_common::CircuitBreakerLedger::load(workspace_root);
            let status = if ledger.tripped {
                "🛑 TRIPPED"
            } else {
                "🟢 HEALTHY"
            };
            println!("Circuit Breaker Status: {status}");
            println!("Agent: {}", ledger.agent);
            println!(
                "Consecutive Failures: {} / {}",
                ledger.consecutive_failures, ledger.max_strikes
            );
            if let Some(tool) = &ledger.last_tool {
                println!("Last Tool: {tool}");
            }
            if let Some(sig) = &ledger.last_error_signature {
                println!("Last Error Signature: {sig}");
            }
            if let Some(details) = &ledger.last_error_details {
                println!("Error Details:\n{details}");
            }
            println!("Last Updated: {}", ledger.last_updated);
            ExitCode::SUCCESS
        },
    }
}

fn handle_workspace_cli(workspace_root: &Path, args: &[String]) -> ExitCode {
    let sub = args.get(2).map(String::as_str);
    let agent = args.get(3).map(String::as_str);
    match (sub, agent) {
        (Some("ensure"), Some(agent_name)) => {
            match tkg_common::JujutsuWorkspace::ensure_agent_workspace(workspace_root, agent_name) {
                Ok(p) => {
                    println!("✅ [tkg] Workspace ready at: {}", p.display());
                    ExitCode::SUCCESS
                },
                Err(e) => {
                    eprintln!("🛑 [tkg] Failed to ensure workspace: {e}");
                    ExitCode::FAILURE
                },
            }
        },
        (Some("forget" | "remove"), Some(agent_name)) => {
            match tkg_common::JujutsuWorkspace::forget_agent_workspace(workspace_root, agent_name) {
                Ok(()) => {
                    println!("✅ [tkg] Workspace forgotten for agent: {agent_name}");
                    ExitCode::SUCCESS
                },
                Err(e) => {
                    eprintln!("🛑 [tkg] Failed to forget workspace: {e}");
                    ExitCode::FAILURE
                },
            }
        },
        _ => {
            eprintln!("Usage: tkg workspace <ensure|forget> <agent-name>");
            ExitCode::FAILURE
        },
    }
}

fn print_cli_usage() {
    println!(concat!(
        "tkg: Drop-in firewall & sandboxed CLI-to-MCP runtime for coding agents\n",
        "\n",
        "QUICKSTART COMMANDS:\n",
        "  tkg init [HARNESS] [--save] [--list]    Inject MCP & guardrail hooks into your harness\n",
        "  tkg doctor [--strict]                   Verify harness hooks, MCP config, and binaries\n",
        "  tkg add <NAME> [OPTS] -- <CMD...>       Wrap any CLI binary into .tkg/tools.toml\n",
        "  tkg uninstall [HARNESS] [--purge]       Remove TorqKage hooks & MCP entries cleanly\n",
        "\n",
        "GATEKEEPER & DIAGNOSTICS:\n",
        "  --status, status      Print agent fleet status & tool distribution matrix\n",
        "  --validate, validate  Run pre-flight validation on plugin & tool schemas\n",
        "  reset-breaker         Reset the circuit breaker strike counter\n",
        "  inspect-breaker       Inspect circuit breaker strikes and error details\n",
        "  --help, -h            Show this help reference\n",
        "\n",
        "OPTIONAL POWER FEATURES (OPT-IN / ISOLATION):\n",
        "  ui, tui, deck         Open interactive Ratatui deck (requires '--features tui')\n",
        "  launch <AGENT>        Launch an agent session (--sandbox, --isolated)\n",
        "  workspace <subcmd>    Manage isolated Jujutsu workspaces (ensure <agent>, forget <agent>)\n",
    ));
}

#[cfg(feature = "tui")]
fn run_tui(workspace_root: PathBuf) -> Result<(), String> {
    tui_loop::run_tui(workspace_root)
}

#[cfg(feature = "tui")]
mod tui_loop {
    use super::{
        ActiveTab, App, CrosstermBackend, Duration, EnterAlternateScreen, Event, FleetFocus,
        KeyCode, KeyEventKind, KeyModifiers, LeaveAlternateScreen, PathBuf, Terminal,
        disable_raw_mode, enable_raw_mode, event, execute, io, launcher, model, stdout, ui,
    };

    pub(super) fn run_tui(workspace_root: PathBuf) -> Result<(), String> {
        let mut app = App::new(workspace_root)?;

        // Set up terminal panic hook to guarantee terminal cleanup
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            _ = disable_raw_mode();
            _ = execute!(stdout(), LeaveAlternateScreen);
            default_hook(panic_info);
        }));

        // Initialize terminal
        enable_raw_mode().map_err(|e| format!("Failed to enable raw mode: {e}"))?;
        let mut stdout_handle = stdout();
        execute!(stdout_handle, EnterAlternateScreen)
            .map_err(|e| format!("Failed to enter alternate screen: {e}"))?;

        let backend = CrosstermBackend::new(stdout_handle);
        let mut terminal =
            Terminal::new(backend).map_err(|e| format!("Failed to initialize terminal: {e}"))?;

        let res = event_loop(&mut terminal, &mut app);

        // Restore terminal
        _ = disable_raw_mode();
        _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
        _ = terminal.show_cursor();

        res
    }

    fn event_loop(
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        app: &mut App,
    ) -> Result<(), String> {
        while !app.should_quit {
            // Handle pending launch request if one was triggered
            if let Some((agent_name, resume_conv)) = app.pending_launch.take() {
                if let Some(agent) = app.agents.iter().find(|a| a.name == agent_name).cloned() {
                    match launcher::launch_agent_interactive(
                        terminal,
                        &app.workspace_root,
                        &agent,
                        app.sandbox_enabled(),
                        app.isolated_workspace(),
                        resume_conv.as_deref(),
                    ) {
                        Ok(status) => {
                            // Reload agents from disk in case tools were modified
                            app.agents = model::discover_agents(&app.workspace_root);
                            app.refresh_selected_agent_session();
                            app.refresh_circuit_breaker();
                            let harness = app.runner_harness();
                            app.status_message = Some((
                                format!(
                                    "✔ Returned from {harness} session with agent '{agent_name}' ({status})"
                                ),
                                false,
                            ));
                        },
                        Err(err) => {
                            app.validation_modal = Some((false, err));
                            app.status_message = Some((
                                "❌ Pre-launch check failed! View diagnostic modal.".to_owned(),
                                true,
                            ));
                        },
                    }
                }
                continue;
            }

            terminal
                .draw(|f| ui::draw(f, app))
                .map_err(|e| format!("Draw error: {e}"))?;

            if event::poll(Duration::from_millis(100)).map_err(|e| format!("Poll error: {e}"))? {
                match event::read().map_err(|e| format!("Read error: {e}"))? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        handle_key_event(app, key.code, key.modifiers);
                    },
                    _ => {},
                }
            }
        }

        Ok(())
    }

    pub(super) fn handle_key_event(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        // 1. Modals capture keys first
        if app.show_help {
            if matches!(
                code,
                KeyCode::Esc | KeyCode::Enter | KeyCode::F(1) | KeyCode::Char('q')
            ) {
                app.show_help = false;
            }
            return;
        }

        if app.validation_modal.is_some() {
            match code {
                KeyCode::Esc | KeyCode::Enter => {
                    app.validation_modal = None;
                },
                KeyCode::F(4) | KeyCode::Char('r') => {
                    app.rollback();
                },
                _ => {},
            }
            return;
        }

        // 2. Global Shortcuts
        if handle_global_shortcuts(app, code, modifiers) {
            return;
        }

        // 3. Tab-Specific Navigation
        match app.active_tab {
            ActiveTab::FleetOverview => handle_fleet_overview_keys(app, code),
            ActiveTab::ToolCatalog => handle_tool_catalog_keys(app, code),
            ActiveTab::DirectivesEditor => handle_directives_editor_keys(app, code),
            ActiveTab::Manual => handle_manual_keys(app, code),
        }
    }

    fn handle_global_shortcuts(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
        match code {
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.should_quit = true;
                true
            },
            KeyCode::Char('q') => {
                app.should_quit = true;
                true
            },
            KeyCode::Char('n') => {
                app.request_launch(false);
                true
            },
            KeyCode::F(1) | KeyCode::Char('?') => {
                app.toggle_manual();
                true
            },
            KeyCode::Char('i') => {
                app.run_installer_and_diagnostics();
                true
            },
            KeyCode::F(2) | KeyCode::Char('S' | 's') => {
                app.save_and_validate();
                true
            },
            KeyCode::F(8) | KeyCode::Char('b') => {
                app.toggle_sandbox();
                true
            },
            KeyCode::Char('w') => {
                app.toggle_isolated_workspace();
                true
            },
            KeyCode::Char('c' | 'C' | 'K') => {
                app.reset_circuit_breaker();
                true
            },
            KeyCode::Char('p') => {
                app.cycle_selected_agent_policy();
                true
            },
            KeyCode::Char('g') if app.active_tab != ActiveTab::Manual => {
                app.toggle_tool_permission_gate();
                true
            },
            KeyCode::F(4) | KeyCode::Char('u' | 'U') => {
                app.rollback();
                true
            },
            KeyCode::Tab => {
                app.next_tab();
                true
            },
            KeyCode::BackTab => {
                app.prev_tab();
                true
            },
            KeyCode::Char('1') => {
                app.active_tab = ActiveTab::FleetOverview;
                true
            },
            KeyCode::Char('2') => {
                app.active_tab = ActiveTab::ToolCatalog;
                true
            },
            KeyCode::Char('3') => {
                app.active_tab = ActiveTab::DirectivesEditor;
                true
            },
            KeyCode::Char('4') => {
                app.active_tab = ActiveTab::Manual;
                true
            },
            _ => false,
        }
    }

    fn handle_fleet_overview_keys(app: &mut App, code: KeyCode) {
        match code {
            KeyCode::Left | KeyCode::Char('h') => {
                app.fleet_focus = FleetFocus::AgentList;
            },
            KeyCode::Right | KeyCode::Char('l') => {
                app.fleet_focus = FleetFocus::ToolList;
            },
            KeyCode::Up | KeyCode::Char('k') => match app.fleet_focus {
                FleetFocus::AgentList => app.prev_agent(),
                FleetFocus::ToolList => app.prev_tool(),
            },
            KeyCode::Down | KeyCode::Char('j') => match app.fleet_focus {
                FleetFocus::AgentList => app.next_agent(),
                FleetFocus::ToolList => app.next_tool(),
            },
            KeyCode::Enter | KeyCode::Char('r') => match app.fleet_focus {
                FleetFocus::AgentList => app.request_launch(true),
                FleetFocus::ToolList => app.toggle_current_tool(),
            },
            KeyCode::Char('n') => match app.fleet_focus {
                FleetFocus::AgentList => app.request_launch(false),
                FleetFocus::ToolList => {},
            },
            KeyCode::Char(' ') => match app.fleet_focus {
                FleetFocus::AgentList => app.toggle_fleet_focus(),
                FleetFocus::ToolList => app.toggle_current_tool(),
            },
            _ => {},
        }
    }

    fn handle_tool_catalog_keys(app: &mut App, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => app.prev_tool(),
            KeyCode::Down | KeyCode::Char('j') => app.next_tool(),
            KeyCode::PageUp => {
                for _ in 0..10 {
                    app.prev_tool();
                }
            },
            KeyCode::PageDown => {
                for _ in 0..10 {
                    app.next_tool();
                }
            },
            _ => {},
        }
    }

    const fn handle_directives_editor_keys(app: &mut App, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                app.directives_scroll = app.directives_scroll.saturating_sub(1);
            },
            KeyCode::Down | KeyCode::Char('j') => {
                app.directives_scroll = app.directives_scroll.saturating_add(1);
            },
            KeyCode::PageUp => {
                app.directives_scroll = app.directives_scroll.saturating_sub(10);
            },
            KeyCode::PageDown => {
                app.directives_scroll = app.directives_scroll.saturating_add(10);
            },
            _ => {},
        }
    }

    fn handle_manual_keys(app: &mut App, code: KeyCode) {
        match code {
            KeyCode::Char('[' | 'h') | KeyCode::Left => {
                app.prev_manual_section();
            },
            KeyCode::Char(']' | 'l') | KeyCode::Right => {
                app.next_manual_section();
            },
            KeyCode::Up | KeyCode::Char('k') => {
                app.manual_scroll = app.manual_scroll.saturating_sub(1);
            },
            KeyCode::Down | KeyCode::Char('j') => {
                app.manual_scroll = app.manual_scroll.saturating_add(1);
            },
            KeyCode::PageUp => {
                app.manual_scroll = app.manual_scroll.saturating_sub(10);
            },
            KeyCode::PageDown => {
                app.manual_scroll = app.manual_scroll.saturating_add(10);
            },
            KeyCode::Home | KeyCode::Char('g') => {
                app.manual_scroll = 0;
            },
            KeyCode::Esc | KeyCode::Enter => {
                app.toggle_manual();
            },
            _ => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_agent_arg_is_order_independent() {
        let v = |xs: &[&str]| xs.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            find_agent_arg(&v(&["tkg", "--sandbox", "coder"])),
            Some("coder")
        );
        assert_eq!(
            find_agent_arg(&v(&["tkg", "coder", "--sandbox"])),
            Some("coder")
        );
        assert_eq!(
            find_agent_arg(&v(&["tkg", "-a", "tester", "--sandbox"])),
            Some("tester")
        );
        assert_eq!(find_agent_arg(&v(&["tkg", "--sandbox"])), None);
    }

    #[cfg(feature = "tui")]
    #[test]
    fn test_k_navigates_up_and_does_not_reset_breaker() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut app = App::new(root).expect("create app");
        assert!(!app.agents.is_empty(), "expected agents in workspace");

        app.selected_agent_idx = 1;
        let initial_status = app.status_message.clone();

        tui_loop::handle_key_event(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);

        assert_eq!(app.selected_agent_idx, 0);
        assert_eq!(app.status_message, initial_status);
    }

    #[cfg(feature = "tui")]
    #[test]
    fn test_c_and_shift_k_reset_circuit_breaker() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut app = App::new(root).expect("create app");

        tui_loop::handle_key_event(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
        let status = app.status_message.as_ref().map(|(msg, _)| msg.clone());
        assert!(status.unwrap().contains("Circuit Breaker RESET"));

        tui_loop::handle_key_event(&mut app, KeyCode::Char('K'), KeyModifiers::NONE);
        let status_k = app.status_message.as_ref().map(|(msg, _)| msg.clone());
        assert!(status_k.unwrap().contains("Circuit Breaker RESET"));
    }

    #[cfg(feature = "tui")]
    #[test]
    fn test_manual_scrolling_and_home_keys() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut app = App::new(root).expect("create app");
        app.active_tab = ActiveTab::Manual;

        tui_loop::handle_key_event(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.manual_scroll, 1);

        tui_loop::handle_key_event(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.manual_scroll, 0);

        tui_loop::handle_key_event(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(app.manual_scroll, 10);
        tui_loop::handle_key_event(&mut app, KeyCode::Char('g'), KeyModifiers::NONE);
        assert_eq!(app.manual_scroll, 0);
    }
}
