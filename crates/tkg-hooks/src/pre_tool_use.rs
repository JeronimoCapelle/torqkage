//! Unified declarative `PreToolUse` hybrid execution engine.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use tkg_common::{CircuitBreakerLedger, HookDecision, PreToolResponse, PreToolUseInput};

use crate::{
    config::{CustomCheck, HooksConfig, matches_pattern},
    conflicts, manifest_guard, policy_gate, stdin, subagent_gate,
};

/// Executes the unified `pre-tool-use` hook.
pub(crate) fn execute() -> std::process::ExitCode {
    let (input, claude_payload) = stdin::read_pre_tool_use_context();
    let root = input.common.resolve_workspace_root();

    let (response, is_config_error) = match HooksConfig::try_load(&root)
        .and_then(|config| evaluate_pre_tool_use_checked(&input, &root, &config))
    {
        Ok(resp) => (resp, false),
        Err(err) => {
            eprintln!("tkg-hooks: {err}");
            (
                PreToolResponse::deny(format!("Configuration error (fail-closed): {err}")),
                true,
            )
        },
    };

    stdin::finalize_pre_tool_use(&response, &root, claude_payload, is_config_error)
}

/// Evaluates a tool invocation input against the configured hybrid pipeline.
#[cfg(test)]
#[must_use]
pub(crate) fn evaluate_pre_tool_use(
    input: &PreToolUseInput,
    root: &Path,
    config: &HooksConfig,
) -> PreToolResponse {
    evaluate_pre_tool_use_checked(input, root, config).unwrap_or_else(|err| {
        PreToolResponse::deny(format!("Configuration error (fail-closed): {err}"))
    })
}

fn evaluate_pre_tool_use_checked(
    input: &PreToolUseInput,
    root: &Path,
    config: &HooksConfig,
) -> tkg_common::Result<PreToolResponse> {
    let tools_cfg = policy_gate::ParsedToolsConfig::try_load(root)?;

    let Some(ref tc) = input.tool_call else {
        return Ok(PreToolResponse::allow());
    };

    let tool_name = tc.name.as_str();

    // 1. Evaluate native pipeline primitives in declared sequence
    for stage in &config.pre_tool_use.pipeline {
        match stage.as_str() {
            "circuit_breaker" => {
                if config.circuit_breaker.enabled {
                    let mut ledger = CircuitBreakerLedger::load(root);
                    if config.circuit_breaker.max_strikes > 0 {
                        ledger.max_strikes = config.circuit_breaker.max_strikes;
                    }
                    if (ledger.tripped || ledger.consecutive_failures >= ledger.max_strikes)
                        && tools_cfg.is_mutating_with_fallback(tool_name)
                    {
                        return Ok(PreToolResponse::ask(format!(
                            "🛑 CIRCUIT BREAKER TRIPPED ({} consecutive gate/verification failures):\n\
                             Last Error/Failure: {}\n\
                             Autonomous loop halted to prevent token burn and code regression.\n\
                             Confirm manual override to allow tool '{tool_name}'?",
                            ledger.consecutive_failures,
                            ledger
                                .last_error_signature
                                .as_deref()
                                .unwrap_or("Verification error")
                        )));
                    }
                }
            },
            "manifest_guard" => {
                if let Some(resp) = manifest_guard::evaluate_manifest_guard(input, root, config) {
                    return Ok(resp);
                }
            },
            "path_policies" => {
                if let Some(resp) = evaluate_path_policies(input, root) {
                    return Ok(resp);
                }
            },
            "subagent_admission" => {
                if config
                    .pre_tool_use
                    .subagent_tools
                    .iter()
                    .any(|t| t == tool_name)
                {
                    let resp = subagent_gate::evaluate_tool_invocation(input);
                    if resp.decision != HookDecision::Allow {
                        return Ok(resp);
                    }
                }
            },
            "vcs_conflicts" => {
                if tools_cfg.tool_requires_gate(tool_name, "vcs_conflicts")
                    && let Some(reason) = conflicts::detect_conflicts(root)
                {
                    return Ok(PreToolResponse::deny(reason));
                }
            },
            "tool_policy" => {
                if tools_cfg.requires_permission_gate(tool_name) {
                    return Ok(PreToolResponse::ask(format!(
                        "Policy Gate: Tool '{tool_name}' requires interactive operator confirmation before execution."
                    )));
                }
                if tools_cfg.is_mutating(tool_name) {
                    return Ok(PreToolResponse::ask(format!(
                        "Policy Gate: Tool '{tool_name}' applies automated filesystem or code modifications. Confirm execution?"
                    )));
                }
            },
            _ => {
                // Non-native stages continue to declarative custom checks
            },
        }
    }

    // 2. Evaluate declarative custom checks
    for check in &config.pre_tool_use.custom_checks {
        let matches = tools_cfg.tool_requires_gate(tool_name, &check.name)
            || (!check.matcher.is_empty() && matches_pattern(tool_name, &check.matcher));

        if matches && let Some(resp) = execute_custom_check(check, root) {
            return Ok(resp);
        }
    }

    Ok(PreToolResponse::allow())
}

fn execute_custom_check(check: &CustomCheck, root: &Path) -> Option<PreToolResponse> {
    let binary = check.command.first()?;
    let args = check.command.get(1..).unwrap_or(&[]);

    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd.current_dir(root);

    let output = tkg_common::SubprocessSupervisor::run_supervised(
        cmd,
        tkg_common::DEFAULT_QUICK_COMMAND_TIMEOUT_SECS,
    )
    .ok()?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if let Some(ref needle) = check.fail_if_output_contains
        && combined.contains(needle)
    {
        return Some(match check.decision {
            HookDecision::Ask => PreToolResponse::ask(check.reason.clone()),
            _ => PreToolResponse::deny(check.reason.clone()),
        });
    }

    None
}

fn resolve_active_agent(input: &PreToolUseInput, root: &Path) -> Option<String> {
    if let Some(agent) = tkg_common::resolve_active_agent(root) {
        return Some(agent);
    }

    if let Some(ref model) = input.common.model_name {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }

    None
}

/// Evaluates agent directory boundary guards and path policies on mutating tool
/// calls.
#[must_use]
pub(crate) fn evaluate_path_policies(
    input: &PreToolUseInput,
    root: &Path,
) -> Option<PreToolResponse> {
    let tc = input.tool_call.as_ref()?;
    let tool_name = tc.name.as_str();

    let candidates = manifest_guard::extract_candidate_paths(&tc.args);
    if candidates.is_empty() {
        return None;
    }

    let is_mutation = policy_gate::is_mutating_tool(tool_name, root)
        || manifest_guard::is_default_fallback_mutation_tool(tool_name)
        || tc.args.get("content").is_some()
        || tc.args.get("Content").is_some()
        || tc.args.get("patch").is_some()
        || tc.args.get("target").is_some()
        || tc.args.get("Target").is_some();

    if !is_mutation {
        return None;
    }

    let active_agent = resolve_active_agent(input, root);
    let agent_guard = active_agent.as_deref().and_then(|agent| {
        let manifest_path = tkg_common::find_agent_manifest_path(root, agent)?;
        let content = std::fs::read_to_string(manifest_path).ok()?;
        let frontmatter = tkg_common::AgentFrontmatter::parse(&content).ok()?;
        Some(frontmatter.to_guard())
    });

    let project = tkg_common::ProjectConfig::detect_or_load(root);

    for cand in &candidates {
        let full_path = if Path::new(cand).is_absolute() {
            PathBuf::from(cand)
        } else {
            root.join(cand)
        };

        // 1. Check directory confinement
        if let Err(err) = tkg_common::check_agent_guard_direct(
            root,
            &full_path,
            active_agent.as_deref(),
            agent_guard.as_ref(),
        ) {
            return Some(PreToolResponse::deny(err));
        }

        // 2. Check path policies (content filter)
        let original = std::fs::read_to_string(&full_path).unwrap_or_default();
        if let Some(new_content) = extract_new_content(&tc.args, &original)
            && let Err(err) = tkg_common::check_agent_path_policies_direct(
                root,
                &full_path,
                &original,
                &new_content,
                active_agent.as_deref(),
                agent_guard.as_ref(),
                Some(&project),
            )
        {
            return Some(PreToolResponse::deny(err));
        }
    }

    None
}

fn extract_new_content(args: &serde_json::Value, original: &str) -> Option<String> {
    if let Some(c) = args
        .get("content")
        .or_else(|| args.get("Content"))
        .and_then(serde_json::Value::as_str)
    {
        return Some(c.to_owned());
    }
    let target = args
        .get("target")
        .or_else(|| args.get("Target"))
        .and_then(serde_json::Value::as_str)?;
    let replacement = args
        .get("replacement")
        .or_else(|| args.get("Replacement"))
        .and_then(serde_json::Value::as_str)?;
    Some(original.replace(target, replacement))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tkg_common::models::ToolCallPayload;

    use super::*;

    #[test]
    fn test_unified_read_only_tool_allowed() {
        let input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "read_project_file".to_owned(),
                args: json!({"path": "src/lib.rs"}),
            }),
            ..Default::default()
        };

        let config = HooksConfig::default();
        let res = evaluate_pre_tool_use(&input, Path::new("."), &config);
        assert_eq!(res.decision, HookDecision::Allow);
    }

    #[test]
    fn test_unified_mutating_tool_asks() {
        let input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "modify_code".to_owned(),
                args: json!({"path": "src/lib.rs"}),
            }),
            ..Default::default()
        };

        let config = HooksConfig::default();
        let res = evaluate_pre_tool_use(&input, Path::new("."), &config);
        assert_eq!(res.decision, HookDecision::Ask);
    }

    #[test]
    fn test_unified_custom_check_fails() {
        let mut config = HooksConfig::default();
        #[cfg(unix)]
        let check_cmd = vec!["echo".to_owned(), "DISK_FULL".to_owned()];
        #[cfg(windows)]
        let check_cmd = vec![
            "cmd".to_owned(),
            "/C".to_owned(),
            "echo".to_owned(),
            "DISK_FULL".to_owned(),
        ];
        config.pre_tool_use.custom_checks.push(CustomCheck {
            name: "test_check".to_owned(),
            matcher: "read_project_file".to_owned(),
            command: check_cmd,
            fail_if_output_contains: Some("DISK_FULL".to_owned()),
            decision: HookDecision::Deny,
            reason: "Simulated custom check failure".to_owned(),
        });

        let input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "read_project_file".to_owned(),
                args: json!({"path": "src/lib.rs"}),
            }),
            ..Default::default()
        };

        let res = evaluate_pre_tool_use(&input, Path::new("."), &config);
        assert_eq!(res.decision, HookDecision::Deny);
        assert!(
            res.reason
                .unwrap()
                .contains("Simulated custom check failure")
        );
    }

    #[test]
    fn test_explicit_category_gate_triggers_vcs_conflicts_and_custom_checks() {
        let temp_dir = std::env::temp_dir().join(format!(
            "gate_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        _ = std::fs::create_dir_all(&temp_dir);
        drop(std::fs::write(
            temp_dir.join("tools.toml"),
            r#"
[categories.compilation]
require_gates = ["vcs_conflicts", "custom_sentinel"]

[tools.my_compiler]
category = "compilation"

[tools.my_unfettered_tool]
category = "compilation"
skip_gates = ["vcs_conflicts", "custom_sentinel"]
"#,
        ));

        // Verify policy_gate resolves category gates & skip_gates from
        // tools.toml on disk
        assert!(policy_gate::tool_requires_gate(
            "my_compiler",
            &temp_dir,
            "vcs_conflicts"
        ));
        assert!(policy_gate::tool_requires_gate(
            "my_compiler",
            &temp_dir,
            "custom_sentinel"
        ));
        assert!(!policy_gate::tool_requires_gate(
            "my_unfettered_tool",
            &temp_dir,
            "vcs_conflicts"
        ));
        assert!(!policy_gate::tool_requires_gate(
            "my_unfettered_tool",
            &temp_dir,
            "custom_sentinel"
        ));

        // PreToolUse custom check triggers purely via declarative require_gates
        let mut config = HooksConfig::default();
        #[cfg(unix)]
        let sentinel_cmd = vec!["echo".to_owned(), "GATE_TRIGGERED".to_owned()];
        #[cfg(windows)]
        let sentinel_cmd = vec![
            "cmd".to_owned(),
            "/C".to_owned(),
            "echo".to_owned(),
            "GATE_TRIGGERED".to_owned(),
        ];
        config.pre_tool_use.custom_checks.push(CustomCheck {
            name: "custom_sentinel".to_owned(),
            matcher: String::new(),
            command: sentinel_cmd,
            fail_if_output_contains: Some("GATE_TRIGGERED".to_owned()),
            decision: HookDecision::Deny,
            reason: "Custom sentinel gate blocked execution".to_owned(),
        });

        let input_gated = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "my_compiler".to_owned(),
                args: json!({}),
            }),
            ..Default::default()
        };

        let res_gated = evaluate_pre_tool_use(&input_gated, &temp_dir, &config);
        assert_eq!(res_gated.decision, HookDecision::Deny);
        assert!(
            res_gated
                .reason
                .unwrap()
                .contains("Custom sentinel gate blocked execution")
        );

        let input_skipped = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "my_unfettered_tool".to_owned(),
                args: json!({}),
            }),
            ..Default::default()
        };

        let res_skipped = evaluate_pre_tool_use(&input_skipped, &temp_dir, &config);
        assert_eq!(res_skipped.decision, HookDecision::Allow);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_pre_tool_use_path_policies_and_agent_frontmatter_guards() {
        let unique_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("tkg_hook_policy_test_{unique_id}"));
        drop(std::fs::create_dir_all(
            temp_dir.join("plugins/torqkage/agents"),
        ));
        _ = std::fs::create_dir_all(temp_dir.join("src"));

        let scoped_agent_manifest = "---\nname: comments_restricted_agent\nallowed_dirs:\n  - docs\n  - src\npath_policies:\n  - path: src\n    filter: comments_only\n---\n";
        std::fs::write(
            temp_dir.join("plugins/torqkage/agents/comments_restricted_agent.md"),
            scoped_agent_manifest,
        )
        .unwrap();

        let orig_code = "pub fn add(a: u32, b: u32) -> u32 {\n    a + b\n}\n";
        std::fs::write(temp_dir.join("src/lib.rs"), orig_code).unwrap();

        let config = HooksConfig::default();

        // 1. Tool targeting path outside allowed_dirs (e.g. tests/) gets DENIED
        let input_disallowed = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "custom_writer".to_owned(),
                args: json!({
                    "path": "tests/test.rs",
                    "content": "fn test() {}"
                }),
            }),
            common: tkg_common::models::CommonHookPayload {
                model_name: Some("comments_restricted_agent".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        };
        let res_disallowed = evaluate_pre_tool_use(&input_disallowed, &temp_dir, &config);
        assert_eq!(res_disallowed.decision, HookDecision::Deny);
        assert!(
            res_disallowed
                .reason
                .unwrap()
                .contains("Security policy violation")
        );

        // 2. Tool attempting code tamper in src/ gets DENIED
        let input_tamper = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "edit_file".to_owned(),
                args: json!({
                    "path": "src/lib.rs",
                    "target": "a + b",
                    "replacement": "a + b + 1"
                }),
            }),
            common: tkg_common::models::CommonHookPayload {
                model_name: Some("comments_restricted_agent".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        };
        let res_tamper = evaluate_pre_tool_use(&input_tamper, &temp_dir, &config);
        assert_eq!(res_tamper.decision, HookDecision::Deny);
        assert!(
            res_tamper
                .reason
                .unwrap()
                .contains("Security policy violation")
        );

        // 3. Tool adding valid doc comments gets ALLOWED
        let input_docs = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "edit_file".to_owned(),
                args: json!({
                    "path": "src/lib.rs",
                    "target": "pub fn add",
                    "replacement": "/// Adds two numbers.\npub fn add"
                }),
            }),
            common: tkg_common::models::CommonHookPayload {
                model_name: Some("comments_restricted_agent".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        };
        let res_docs = evaluate_pre_tool_use(&input_docs, &temp_dir, &config);
        assert_eq!(res_docs.decision, HookDecision::Allow);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    mod prop_tests {
        use proptest::prelude::*;

        use super::*;

        fn independent_strictly_inside(raw: &str, scope_dir: &str) -> bool {
            if raw.is_empty() || raw.contains('\0') || raw.starts_with('/') || raw.starts_with('\\')
            {
                return false;
            }
            let bytes = raw.as_bytes();
            if bytes.get(1) == Some(&b':') && bytes.first().is_some_and(u8::is_ascii_alphabetic) {
                return false;
            }
            let mut components = Vec::new();
            for part in raw.split(['/', '\\']) {
                match part {
                    "" | "." => {},
                    ".." => return false,
                    other => components.push(other),
                }
            }
            components.first().is_some_and(|first| *first == scope_dir)
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(96))]

            #[test]
            fn prop_pre_tool_use_confines_arbitrary_paths_to_allowed_dir_and_blocks_tamper(
                allowed_dir in "[a-z]{2,8}",
                arbitrary_path in prop_oneof![
                    any::<String>(),
                    "\\PC*",
                ],
                mutator_tool in prop_oneof![
                    Just("edit_file"),
                    Just("write_file"),
                    Just("custom_writer"),
                ],
                tamper_payload in "\\PC{1,40}",
                tamper_delta in 1u32..500,
            ) {
                let dir = std::env::temp_dir().join(format!(
                    "tkg_ptu_prop_{}_{}",
                    std::process::id(),
                    tamper_delta
                ));
                _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(dir.join("plugins/torqkage/agents")).expect("mkdir agents");
                std::fs::create_dir_all(dir.join(&allowed_dir)).expect("mkdir allowed");

                let manifest = format!(
                    "---\nname: strict_doc_agent\nallowed_dirs:\n  - {allowed_dir}\npath_policies:\n  - path: {allowed_dir}\n    filter: comments_only\n---\n"
                );
                std::fs::write(
                    dir.join("plugins/torqkage/agents/strict_doc_agent.md"),
                    manifest,
                )
                .expect("write manifest");

                let lib_path = dir.join(&allowed_dir).join("lib.rs");
                std::fs::write(&lib_path, "pub fn answer() -> u32 {\n    42\n}\n")
                    .expect("write lib.rs");

                let config = HooksConfig::default();

                // 1. Prefix-lookalike path (`<allowed_dir>_extra/x.rs`) must ALWAYS be denied.
                let prefix_lookalike = format!("{allowed_dir}_extra/x.rs");
                let lookalike_input = PreToolUseInput {
                    tool_call: Some(ToolCallPayload {
                        name: mutator_tool.to_owned(),
                        args: json!({ "path": prefix_lookalike, "content": "// ok" }),
                    }),
                    common: tkg_common::models::CommonHookPayload {
                        model_name: Some("strict_doc_agent".to_owned()),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let lookalike_res = evaluate_pre_tool_use(&lookalike_input, &dir, &config);
                prop_assert_ne!(lookalike_res.decision, HookDecision::Allow);

                // 2. Any arbitrary UTF-8 path must never return Allow unless the independent
                //    lexical oracle proves it is strictly confined inside `allowed_dir`.
                let arb_input = PreToolUseInput {
                    tool_call: Some(ToolCallPayload {
                        name: mutator_tool.to_owned(),
                        args: json!({ "path": arbitrary_path, "content": "// comment" }),
                    }),
                    common: tkg_common::models::CommonHookPayload {
                        model_name: Some("strict_doc_agent".to_owned()),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let arb_res = evaluate_pre_tool_use(&arb_input, &dir, &config);
                if !independent_strictly_inside(&arbitrary_path, &allowed_dir) {
                    prop_assert_ne!(
                        arb_res.decision,
                        HookDecision::Allow,
                        "unsafe path unexpectedly allowed: {:?}",
                        arbitrary_path
                    );
                }

                // 3. Adversarial code tamper or comment-delimiter injection inside `allowed_dir/lib.rs`
                //    must NEVER be allowed unless stripped code tokens remain identical.
                let new_val = 42u32.saturating_add(tamper_delta);
                let tamper_replacement = format!("{new_val} /* {tamper_payload} */");
                let tamper_input = PreToolUseInput {
                    tool_call: Some(ToolCallPayload {
                        name: "edit_file".to_owned(),
                        args: json!({
                            "path": format!("{allowed_dir}/lib.rs"),
                            "target": "42",
                            "replacement": tamper_replacement,
                        }),
                    }),
                    common: tkg_common::models::CommonHookPayload {
                        model_name: Some("strict_doc_agent".to_owned()),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let tamper_res = evaluate_pre_tool_use(&tamper_input, &dir, &config);
                prop_assert_eq!(tamper_res.decision, HookDecision::Deny);

                _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}
