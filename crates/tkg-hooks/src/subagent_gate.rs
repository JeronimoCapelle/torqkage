//! Pre-tool gate for `invoke_subagent` calls validating declared
//! `pre_launch_checks`.

use serde_json::Value;
use tkg_common::{PreToolResponse, PreToolUseInput};

use crate::stdin;

/// Executes the `check-subagent` command.
///
/// Inspects incoming `PreToolUseInput` on `stdin`. If the tool call is
/// `invoke_subagent`, it parses the target agent's declared `pre_launch_checks`
/// and executes all specified gates. If any check fails, it outputs a blocking
/// `PreToolResponse::deny` payload. Otherwise, outputs
/// `PreToolResponse::allow`.
pub(crate) fn execute() -> std::process::ExitCode {
    let (input, claude_payload) = stdin::read_pre_tool_use_context();
    let root = input.common.resolve_workspace_root();
    let (response, is_config_error) = match crate::config::HooksConfig::try_load(&root) {
        Ok(cfg) => (
            evaluate_tool_invocation_for_tools(&input, &cfg.pre_tool_use.subagent_tools),
            false,
        ),
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

/// Evaluates a tool invocation input against the default `invoke_subagent` tool
/// name.
pub(crate) fn evaluate_tool_invocation(input: &PreToolUseInput) -> PreToolResponse {
    evaluate_tool_invocation_with_check(input)
}

/// Evaluates a tool invocation input only when the tool matches
/// `subagent_tools`.
pub(crate) fn evaluate_tool_invocation_for_tools(
    input: &PreToolUseInput,
    subagent_tools: &[String],
) -> PreToolResponse {
    let Some(ref tc) = input.tool_call else {
        return PreToolResponse::allow();
    };
    if !subagent_tools.iter().any(|t| t == &tc.name) {
        return PreToolResponse::allow();
    }
    evaluate_tool_invocation_with_check(input)
}

fn evaluate_tool_invocation_with_check(input: &PreToolUseInput) -> PreToolResponse {
    let Some(ref tc) = input.tool_call else {
        return PreToolResponse::allow();
    };
    if tc.name.is_empty() {
        return PreToolResponse::allow();
    }

    let type_name = tc
        .args
        .get("TypeName")
        .or_else(|| tc.args.get("typeName"))
        .or_else(|| tc.args.get("type_name"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    if type_name.is_empty() {
        return PreToolResponse::allow();
    }

    // Resolve workspace root from payload or current working directory
    let root = input.common.resolve_workspace_root();

    // Evaluate declared pre_launch_checks via canonical subagent admission gate
    match tkg_common::subagent_gate::check_subagent_admission(&root, type_name) {
        Ok(_) => PreToolResponse::allow(),
        Err(reason) => PreToolResponse::deny(reason),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tkg_common::models::ToolCallPayload;

    use super::*;

    #[test]
    fn test_non_subagent_tool_is_allowed() {
        let input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "run_command".to_owned(),
                args: json!({"CommandLine": "cargo test"}),
            }),
            ..Default::default()
        };

        let res = evaluate_tool_invocation(&input);
        assert_eq!(res.decision, tkg_common::models::HookDecision::Allow);
    }

    #[test]
    fn test_other_subagent_is_allowed() {
        let input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "invoke_subagent".to_owned(),
                args: json!({
                    "TypeName": "helper_agent",
                    "Role": "Tester",
                    "Prompt": "Calculate 2+2"
                }),
            }),
            ..Default::default()
        };

        let res = evaluate_tool_invocation(&input);
        assert_eq!(res.decision, tkg_common::models::HookDecision::Allow);
    }

    #[test]
    fn test_subagent_evaluates_readiness() {
        let temp_dir = std::env::temp_dir().join(format!(
            "admission_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let agents_dir = temp_dir.join("plugins/torqkage/agents");
        _ = std::fs::create_dir_all(&agents_dir);
        drop(std::fs::write(
            agents_dir.join("reviewer.md"),
            "---\nname: reviewer\npre_launch_checks:\n  - vcs_conflicts\n---\nPrompt",
        ));

        let mut input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "invoke_subagent".to_owned(),
                args: json!({
                    "TypeName": "reviewer",
                    "Role": "Review auditor",
                    "Prompt": "Check workspace readiness"
                }),
            }),
            ..Default::default()
        };
        input.common.workspace_paths = vec![temp_dir.clone()];

        let res = evaluate_tool_invocation(&input);
        assert_eq!(res.decision, tkg_common::models::HookDecision::Allow);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    mod prop_tests {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            #[test]
            fn prop_non_subagent_tools_always_allowed(
                tool_name in "[a-zA-Z0-9_.-]{0,24}",
                type_name in ".{0,32}",
            ) {
                let effective_tool = if tool_name == "invoke_subagent" {
                    format!("{tool_name}_other")
                } else {
                    tool_name
                };
                let input = PreToolUseInput {
                    tool_call: Some(ToolCallPayload {
                        name: effective_tool,
                        args: json!({
                            "TypeName": type_name,
                            "Role": "Tester",
                            "Prompt": "Task"
                        }),
                    }),
                    ..Default::default()
                };
                let res = evaluate_tool_invocation_for_tools(&input, &["invoke_subagent".to_owned()]);
                prop_assert_eq!(res.decision, tkg_common::models::HookDecision::Allow);
            }

            #[test]
            fn prop_subagent_gate_handles_arbitrary_type_names_and_key_aliases(
                type_name in "[a-zA-Z0-9_./:-]{0,40}",
                key in prop_oneof![Just("TypeName"), Just("typeName"), Just("type_name")],
            ) {
                let dir_id = format!(
                    "subagent_prop_{}_{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock")
                        .as_nanos()
                );
                let temp_dir = std::env::temp_dir().join(dir_id);
                let agents_dir = temp_dir.join("plugins/torqkage/agents");
                _ = std::fs::create_dir_all(&agents_dir);
                _ = std::fs::write(
                    agents_dir.join("clean_agent.md"),
                    "---\nname: clean_agent\npre_launch_checks:\n  - vcs_conflicts\n---\nPrompt",
                );

                let mut args_map = serde_json::Map::new();
                args_map.insert(key.to_owned(), Value::String(type_name));
                let mut input = PreToolUseInput {
                    tool_call: Some(ToolCallPayload {
                        name: "invoke_subagent".to_owned(),
                        args: Value::Object(args_map),
                    }),
                    ..Default::default()
                };
                input.common.workspace_paths = vec![temp_dir.clone()];

                let res = evaluate_tool_invocation_for_tools(&input, &["invoke_subagent".to_owned()]);
                prop_assert_eq!(res.decision, tkg_common::models::HookDecision::Allow);

                _ = std::fs::remove_dir_all(&temp_dir);
            }
        }
    }
}
