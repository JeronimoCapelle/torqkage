//! # tkg-mcp
//!
//! Declarative, bounded-action Model Context Protocol (MCP) server daemon over
//! stdio.
//!
//! ## Overview
//! `tkg-mcp` is a persistent daemon that implements the JSON-RPC 2.0 MCP
//! specification over `stdin`/`stdout`. It reads tool definitions and security
//! guard policies from `plugins/torqkage/tools.toml` and scopes exposed tools
//! dynamically based on the active agent's YAML frontmatter (`mcp_tools:`).
//!
//! ## Architecture
//! - [`actions`]: 10 native action handlers (`file_read`, `file_write`,
//!   `mpatch_edit`, `parameterized_cmd`, etc.).
//! - [`agent_scope`]: Resolves active agent scope and enforces air-gapped
//!   default-deny tool allowlists.
//! - [`config`]: Declarative `tools.toml` parser with template inheritance and
//!   security guard models.
//! - [`guards`]: Path confinement, dangerous file sanitization, and UTF-8
//!   truncation guards.
//! - [`server`]: Asynchronous JSON-RPC 2.0 event loop powered by Tokio.
//! - [`validator`]: Startup and pre-flight validation verifying binaries,
//!   schemas, and guards.

#![forbid(unsafe_code)]
#![allow(
    clippy::print_stderr,
    reason = "Daemon diagnostics are logged to stderr"
)]

mod actions;
mod agent_scope;
mod config;
mod guards;
mod models;
mod protocol;
mod schema;
mod server;
mod validator;

use std::{path::PathBuf, process::ExitCode};

use server::McpServer;
use tkg_common::WorkspaceResolver;

#[tokio::main]
async fn main() -> ExitCode {
    let raw_args: Vec<String> = std::env::args().collect();
    let args = tkg_common::diagnostics::init_stderr_tracing(
        &raw_args,
        tkg_common::diagnostics::DefaultVerbosity::Info,
    );
    let (workspace_root, config_path, agent_name) = resolve_workspace_and_config(&args);

    tracing::info!(workspace = %workspace_root.display(), "[tkg-mcp] Workspace root");
    tracing::info!(
        config = %config_path.display(),
        "[tkg-mcp] Loading declarative tools configuration..."
    );

    let loaded_config = match config::load_full_config_from_path(&config_path) {
        Ok(t) => t,
        Err(err) => {
            tracing::error!("🛑 [tkg-mcp] Startup failure: {err}");
            return ExitCode::FAILURE;
        },
    };

    let report = validator::validate_tools(&loaded_config.tools);
    tracing::info!("{}", report.summary());

    for warning in report.warnings {
        tracing::warn!("[tkg-mcp] {warning}");
    }

    let guards = agent_scope::resolve_agent_guards(&workspace_root, agent_name.as_deref());

    let server = McpServer::new(loaded_config.tools, workspace_root, agent_name, guards);

    if let Err(err) = server.run_stdio().await {
        tracing::error!("🛑 [tkg-mcp] Stdio loop error: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn resolve_workspace_and_config(args: &[String]) -> (PathBuf, PathBuf, Option<String>) {
    let mut custom_config = None;
    let mut custom_workspace = None;

    let mut i = 1;
    while i < args.len() {
        if let Some(arg) = args.get(i) {
            if (arg == "--config" || arg == "-c")
                && let Some(val) = args.get(i + 1)
            {
                custom_config = Some(PathBuf::from(val));
                i += 1;
            } else if (arg == "--workspace" || arg == "-w")
                && let Some(val) = args.get(i + 1)
            {
                custom_workspace = Some(PathBuf::from(val));
                i += 1;
            }
        }
        i += 1;
    }

    let workspace_root = custom_workspace.unwrap_or_else(|| {
        WorkspaceResolver::find_root(None).unwrap_or_else(|_| PathBuf::from("."))
    });

    let config_path = custom_config.unwrap_or_else(|| {
        if let Some(found) = tkg_common::harness::find_tools_toml(&workspace_root) {
            if !found.starts_with(&workspace_root) {
                tracing::info!(
                    path = %found.display(),
                    "[tkg-mcp] Using installed plugin configuration"
                );
            }
            return found;
        }

        let fallback = workspace_root.join("tools.toml");
        tracing::warn!(
            path = %fallback.display(),
            "[tkg-mcp] No tools.toml found in workspace or plugins; using fallback"
        );
        fallback
    });

    let agent_name = agent_scope::resolve_agent_scope(args, &workspace_root);

    (workspace_root, config_path, agent_name)
}
