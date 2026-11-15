//! # installer
//!
//! Host system prerequisite diagnostics and thin delegation to the unified
//! [`Harness`](tkg_common::harness::Harness) config installer.

use std::{
    fmt::Write,
    path::{Path, PathBuf},
};

use tkg_common::FastHashSet;

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

/// Runs a diagnostic scan of the host environment, plugin install path,
/// and dynamically validates all binaries declared across `tools.toml` and
/// `project.toml`.
#[must_use]
pub(crate) fn run_diagnostics(workspace_root: &Path) -> DiagnosticsReport {
    let project_cfg = tkg_common::project::ProjectConfig::detect_or_load(workspace_root);
    let runner = project_cfg.resolved_runner();
    let harness_binary = runner.resolved_binary();

    let (plugin_path, plugin_installed, is_symlink) =
        runner.harness().config_status(workspace_root, &runner);

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

/// Registers or installs the `TorqKage` configuration for the selected
/// execution harness via
/// [`Harness::inject_configs`](tkg_common::harness::Harness::inject_configs).
pub(crate) fn register_harness_config(
    workspace_root: &Path,
    runner: &tkg_common::ProjectRunnerConfig,
) -> Result<String, String> {
    runner
        .harness()
        .inject_configs(workspace_root, runner)
        .map_err(|e| e.to_string())
}
