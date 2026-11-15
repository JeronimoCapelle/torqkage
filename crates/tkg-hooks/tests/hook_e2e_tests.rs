//! End-to-end integration tests for `tkg-hooks` lifecycle hook CLI.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::tests_outside_test_module,
    clippy::collapsible_if,
    reason = "Integration test assertions"
)]

use std::{
    io::Write,
    process::{Command, Stdio},
};

use tkg_common::{
    PreInvocationInput, PreInvocationResponse, SessionStartInput, SessionStartResponse, StopInput,
    StopResponse,
};

fn run_hook_subcommand(subcmd: &str, stdin_json: Option<&str>) -> (bool, String, String) {
    let (code, stdout, stderr) = run_hook_subcommand_with_env(subcmd, stdin_json, &[]);
    (code == Some(0), stdout, stderr)
}

fn run_hook_subcommand_with_env(
    subcmd: &str,
    stdin_json: Option<&str>,
    envs: &[(&str, &str)],
) -> (Option<i32>, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tkg-hooks"));
    cmd.arg(subcmd);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    if stdin_json.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    let mut child = cmd.spawn().expect("failed to spawn tkg-hooks binary");

    if let Some(json) = stdin_json {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(json.as_bytes())
                .expect("write to stdin failed");
        }
    }

    let output = child
        .wait_with_output()
        .expect("failed to read hook output");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();

    (output.status.code(), stdout, stderr)
}

#[test]
fn test_record_handoff_and_inject_wake_roundtrip_e2e() {
    let temp_workspace = std::env::temp_dir().join(format!("tkg_test_ws_{}", std::process::id()));
    std::fs::create_dir_all(&temp_workspace).expect("create temp workspace");

    // 1. Run record-handoff
    let stop_input = StopInput {
        execution_num: Some(1),
        termination_reason: Some("NO_TOOL_CALL".to_owned()),
        error: None,
        fully_idle: Some(true),
        common: tkg_common::CommonHookPayload {
            conversation_id: Some("test-handoff-conv".to_owned()),
            workspace_paths: vec![temp_workspace.clone()],
            transcript_path: None,
            artifact_directory_path: None,
            model_name: Some("agent_outgoing".to_owned()),
            last_user_input: None,
        },
    };

    let stop_json = serde_json::to_string(&stop_input).expect("serialize stop input");
    let (success_stop, stdout_stop, _) = run_hook_subcommand("record-handoff", Some(&stop_json));
    assert!(success_stop);
    let stop_resp: StopResponse =
        serde_json::from_str(&stdout_stop).expect("valid StopResponse JSON");
    assert_eq!(stop_resp.decision, None); // {} permits stop

    // Check that .tkg/state/last_handoff.md was created
    let handoff_file = tkg_common::TkgStatePaths::new(&temp_workspace).handoff_md_file();
    assert!(handoff_file.exists());

    // 2. Run inject-wake
    let start_input = SessionStartInput {
        common: tkg_common::CommonHookPayload {
            conversation_id: Some("test-wake-conv".to_owned()),
            workspace_paths: vec![temp_workspace.clone()],
            transcript_path: None,
            artifact_directory_path: None,
            model_name: Some("agent_incoming".to_owned()),
            last_user_input: None,
        },
    };

    let start_json = serde_json::to_string(&start_input).expect("serialize start input");
    let (success_wake, stdout_wake, _) = run_hook_subcommand("inject-wake", Some(&start_json));
    assert!(success_wake);

    let wake_resp: SessionStartResponse =
        serde_json::from_str(&stdout_wake).expect("valid SessionStartResponse");
    assert_eq!(wake_resp.inject_steps.len(), 1);

    // Clean up temp workspace
    drop(std::fs::remove_dir_all(&temp_workspace));
}

#[test]
fn test_detect_wip_consecutive_burns_zero_tokens_e2e() {
    let payload = PreInvocationInput {
        invocation_num: Some(1),
        initial_num_steps: Some(10),
        common: tkg_common::CommonHookPayload {
            conversation_id: Some(format!("test-wip-conv-{}", std::process::id())),
            workspace_paths: vec![std::env::temp_dir()],
            transcript_path: None,
            artifact_directory_path: None,
            model_name: Some("auto".to_owned()),
            last_user_input: Some("proceed".to_owned()),
        },
    };

    let input_json = serde_json::to_string(&payload).expect("serialize payload");

    // First call
    let (success1, ..) = run_hook_subcommand("detect-wip", Some(&input_json));
    assert!(success1);

    // Immediate second call should be clean/unchanged -> burns 0 tokens ({})
    let (success2, stdout2, _) = run_hook_subcommand("detect-wip", Some(&input_json));
    assert!(success2);
    let resp2: PreInvocationResponse =
        serde_json::from_str(&stdout2).expect("valid PreInvocationResponse");
    assert_eq!(resp2.inject_steps, None);
    assert_eq!(stdout2, "{}");
}

#[test]
fn test_check_conflicts_e2e() {
    let (success, stdout, _) = run_hook_subcommand("check-conflicts", None);
    assert!(success);
    let resp: tkg_common::PreToolResponse =
        serde_json::from_str(&stdout).expect("valid PreToolResponse");
    // Clean working repo should allow
    assert_eq!(resp.decision, tkg_common::HookDecision::Allow);
}

#[test]
fn test_pre_tool_use_read_only_allowed_e2e() {
    let payload = tkg_common::PreToolUseInput {
        tool_call: Some(tkg_common::models::ToolCallPayload {
            name: "read_project_file".to_owned(),
            args: serde_json::json!({"path": "src/lib.rs"}),
        }),
        ..Default::default()
    };

    let input_json = serde_json::to_string(&payload).expect("serialize payload");
    let (success, stdout, _) = run_hook_subcommand("pre-tool-use", Some(&input_json));
    assert!(success);
    let resp: tkg_common::PreToolResponse =
        serde_json::from_str(&stdout).expect("valid PreToolResponse");
    assert_eq!(resp.decision, tkg_common::HookDecision::Allow);
}

#[test]
fn test_pre_tool_use_mutating_asks_e2e() {
    let payload = tkg_common::PreToolUseInput {
        tool_call: Some(tkg_common::models::ToolCallPayload {
            name: "modify_code".to_owned(),
            args: serde_json::json!({"path": "src/lib.rs"}),
        }),
        ..Default::default()
    };

    let input_json = serde_json::to_string(&payload).expect("serialize payload");
    let (success, stdout, _) = run_hook_subcommand("pre-tool-use", Some(&input_json));
    assert!(success);
    let resp: tkg_common::PreToolResponse =
        serde_json::from_str(&stdout).expect("valid PreToolResponse");
    assert_eq!(resp.decision, tkg_common::HookDecision::Ask);
}

#[test]
fn test_claude_dialect_deny_returns_exit_code_2_e2e() {
    let ws = std::env::temp_dir().join(format!("tkg_claude_deny_ws_{}", std::process::id()));
    std::fs::create_dir_all(ws.join("plugins/torqkage/agents")).expect("create agents dir");
    std::fs::create_dir_all(ws.join("tests")).expect("create tests dir");
    std::fs::write(
        ws.join("plugins/torqkage/agents/scoped_agent.md"),
        "---\nname: scoped_agent\nallowed_dirs:\n  - docs\n---\n",
    )
    .expect("write manifest");

    let payload = tkg_common::PreToolUseInput {
        tool_call: Some(tkg_common::models::ToolCallPayload {
            name: "custom_writer".to_owned(),
            args: serde_json::json!({"path": "tests/test.rs", "content": "fn t() {}"}),
        }),
        common: tkg_common::CommonHookPayload {
            workspace_paths: vec![ws.clone()],
            model_name: Some("scoped_agent".to_owned()),
            ..Default::default()
        },
        ..Default::default()
    };
    let input_json = serde_json::to_string(&payload).expect("serialize payload");

    // Default dialect: exit code 0, canonical JSON deny
    let (code_proto, stdout_proto, _) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&input_json), &[]);
    assert_eq!(code_proto, Some(0));
    let resp: tkg_common::PreToolResponse =
        serde_json::from_str(&stdout_proto).expect("valid PreToolResponse");
    assert_eq!(resp.decision, tkg_common::HookDecision::Deny);

    // Claude Code dialect: exit code 2, stderr diagnostics, hookSpecificOutput
    // JSON
    let (code_claude, stdout_claude, stderr_claude) = run_hook_subcommand_with_env(
        "pre-tool-use",
        Some(&input_json),
        &[("TKG_HARNESS", "claude")],
    );
    assert_eq!(code_claude, Some(2), "stderr: {stderr_claude}");
    assert!(stderr_claude.contains("TorqKage policy violation"));
    let v: serde_json::Value = serde_json::from_str(&stdout_claude).expect("valid JSON");
    assert_eq!(
        v.pointer("/hookSpecificOutput/permissionDecision")
            .and_then(serde_json::Value::as_str),
        Some("deny")
    );

    std::fs::remove_dir_all(&ws).expect("cleanup");
}

#[test]
fn test_corrupt_hooks_and_tools_toml_fail_closed_e2e() {
    let ws = std::env::temp_dir().join(format!(
        "tkg_corrupt_hooks_ws_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(ws.join(".tkg")).expect("create .tkg dir");
    std::fs::write(
        ws.join(".tkg/hooks.toml"),
        "[pre_tool_use]\npipeline = [[\n",
    )
    .expect("write corrupt hooks.toml");

    let payload = tkg_common::PreToolUseInput {
        tool_call: Some(tkg_common::models::ToolCallPayload {
            name: "read_file".to_owned(),
            args: serde_json::json!({"path": "README.md"}),
        }),
        common: tkg_common::CommonHookPayload {
            workspace_paths: vec![ws.clone()],
            ..Default::default()
        },
        ..Default::default()
    };
    let input_json = serde_json::to_string(&payload).expect("serialize payload");

    let (code, stdout, stderr) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&input_json), &[]);
    assert_ne!(
        code,
        Some(0),
        "corrupt hooks.toml must fail closed with non-zero exit"
    );
    assert!(stderr.contains("at line 2, column"), "stderr: {stderr}");
    let resp: tkg_common::PreToolResponse =
        serde_json::from_str(&stdout).expect("valid PreToolResponse");
    assert_eq!(resp.decision, tkg_common::HookDecision::Deny);

    // Replace hooks.toml with valid config and corrupt tools.toml instead.
    std::fs::remove_file(ws.join(".tkg/hooks.toml")).expect("remove hooks.toml");
    std::fs::write(
        ws.join(".tkg/tools.toml"),
        "[tools.read_file]\nmutating = [[\n",
    )
    .expect("write corrupt tools.toml");

    let (code_tools, stdout_tools, stderr_tools) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&input_json), &[]);
    assert_ne!(
        code_tools,
        Some(0),
        "corrupt tools.toml must fail closed with non-zero exit"
    );
    assert!(
        stderr_tools.contains("at line 2, column"),
        "stderr: {stderr_tools}"
    );
    let resp_tools: tkg_common::PreToolResponse =
        serde_json::from_str(&stdout_tools).expect("valid PreToolResponse");
    assert_eq!(resp_tools.decision, tkg_common::HookDecision::Deny);

    std::fs::remove_dir_all(&ws).expect("cleanup");
}
