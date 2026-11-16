//! End-to-end integration tests for `tkg-hooks` lifecycle hook CLI.

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

#[allow(
    clippy::expect_used,
    reason = "Integration test helper panics on spawn or pipe failure"
)]
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

    if let Some(json) = stdin_json
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin
            .write_all(json.as_bytes())
            .expect("write to stdin failed");
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
    _ = std::fs::remove_dir_all(&temp_workspace);
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

    // Auto-detected Claude Code payload (no TKG_HARNESS env var) on Deny:
    // returns exit code 2 + Claude hookSpecificOutput JSON with
    // permissionDecision = "deny"
    let claude_deny_payload = serde_json::json!({
        "session_id": "claude-auto-deny",
        "cwd": ws,
        "hook_event_name": "PreToolUse",
        "tool_name": "custom_writer",
        "tool_input": { "path": "tests/test.rs", "content": "fn t() {}" },
        "modelName": "scoped_agent"
    })
    .to_string();
    let (code_auto_deny, stdout_auto_deny, stderr_auto_deny) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&claude_deny_payload), &[]);
    assert_eq!(code_auto_deny, Some(2), "stderr: {stderr_auto_deny}");
    assert!(stderr_auto_deny.contains("TorqKage policy violation"));
    let v_deny: serde_json::Value =
        serde_json::from_str(&stdout_auto_deny).expect("valid Claude JSON");
    assert_eq!(
        v_deny
            .pointer("/hookSpecificOutput/hookEventName")
            .and_then(serde_json::Value::as_str),
        Some("PreToolUse")
    );
    assert_eq!(
        v_deny
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(serde_json::Value::as_str),
        Some("deny")
    );

    // Auto-detected Claude Code payload on Ask (mutating tool):
    // returns exit code 0 + Claude hookSpecificOutput JSON with
    // permissionDecision = "ask"
    let claude_ask_payload = serde_json::json!({
        "session_id": "claude-auto-ask",
        "hook_event_name": "PreToolUse",
        "tool_name": "modify_code",
        "tool_input": { "path": "src/lib.rs" }
    })
    .to_string();
    let (code_auto_ask, stdout_auto_ask, _) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&claude_ask_payload), &[]);
    assert_eq!(code_auto_ask, Some(0));
    let v_ask: serde_json::Value =
        serde_json::from_str(&stdout_auto_ask).expect("valid Claude Ask JSON");
    assert_eq!(
        v_ask
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(serde_json::Value::as_str),
        Some("ask")
    );
    assert!(
        v_ask
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .and_then(serde_json::Value::as_str)
            .is_some()
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

    // Auto-detected Claude Code payload (no TKG_HARNESS env var) on Deny:
    // returns exit code 2 + Claude hookSpecificOutput JSON with
    // permissionDecision = "deny"
    let claude_deny_payload = serde_json::json!({
        "session_id": "claude-auto-deny",
        "cwd": ws,
        "hook_event_name": "PreToolUse",
        "tool_name": "custom_writer",
        "tool_input": { "path": "tests/test.rs", "content": "fn t() {}" },
        "modelName": "scoped_agent"
    })
    .to_string();
    let (code_auto_deny, stdout_auto_deny, stderr_auto_deny) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&claude_deny_payload), &[]);
    assert_eq!(code_auto_deny, Some(2), "stderr: {stderr_auto_deny}");
    assert!(stderr_auto_deny.contains("TorqKage policy violation"));
    let v_deny: serde_json::Value =
        serde_json::from_str(&stdout_auto_deny).expect("valid Claude JSON");
    assert_eq!(
        v_deny
            .pointer("/hookSpecificOutput/hookEventName")
            .and_then(serde_json::Value::as_str),
        Some("PreToolUse")
    );
    assert_eq!(
        v_deny
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(serde_json::Value::as_str),
        Some("deny")
    );

    // Auto-detected Claude Code payload on Ask (mutating tool):
    // returns exit code 0 + Claude hookSpecificOutput JSON with
    // permissionDecision = "ask"
    let claude_ask_payload = serde_json::json!({
        "session_id": "claude-auto-ask",
        "hook_event_name": "PreToolUse",
        "tool_name": "modify_code",
        "tool_input": { "path": "src/lib.rs" }
    })
    .to_string();
    let (code_auto_ask, stdout_auto_ask, _) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&claude_ask_payload), &[]);
    assert_eq!(code_auto_ask, Some(0));
    let v_ask: serde_json::Value =
        serde_json::from_str(&stdout_auto_ask).expect("valid Claude Ask JSON");
    assert_eq!(
        v_ask
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(serde_json::Value::as_str),
        Some("ask")
    );
    assert!(
        v_ask
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .and_then(serde_json::Value::as_str)
            .is_some()
    );

    std::fs::remove_dir_all(&ws).expect("cleanup");
}

#[test]
fn test_claude_session_start_pre_invocation_and_custom_subagent_tools_e2e() {
    let ws = std::env::temp_dir().join(format!(
        "tkg_test_claude_hooks_custom_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(ws.join(".tkg/agents")).expect("create .tkg/agents");
    std::fs::write(
        ws.join(".tkg/project.toml"),
        "[gates.failing_gate]\ncommand = [\"false\"]\n",
    )
    .expect("write project.toml");
    std::fs::write(
        ws.join(".tkg/agents/worker.md"),
        "---\nname: worker\ndescription: Worker\npre_launch_checks:\n  - failing_gate\n---\n\n# Worker\n",
    )
    .expect("write worker manifest");
    std::fs::write(
        ws.join(".tkg/hooks.toml"),
        "[pre_tool_use]\nsubagent_tools = [\"delegate_agent\"]\n",
    )
    .expect("write hooks.toml");

    // 1. Custom subagent_tools blocks unpermitted write inside subagent
    //    dispatch
    let subagent_call = serde_json::json!({
        "session_id": "claude-sub-1",
        "cwd": ws,
        "hook_event_name": "PreToolUse",
        "tool_name": "delegate_agent",
        "tool_input": {
            "TypeName": "worker",
            "tool_name": "write_file"
        }
    })
    .to_string();
    let (code_sub, stdout_sub, _) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&subagent_call), &[]);
    assert_eq!(code_sub, Some(2));
    let v_sub: serde_json::Value = serde_json::from_str(&stdout_sub).expect("valid Claude JSON");
    assert_eq!(
        v_sub
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(serde_json::Value::as_str),
        Some("deny")
    );

    // 2. Claude Stop + SessionStart roundtrip emits
    //    hookSpecificOutput.additionalContext
    let stop_payload = serde_json::json!({
        "session_id": "claude-stop-1",
        "cwd": ws,
        "hook_event_name": "Stop",
        "termination_reason": "NO_TOOL_CALL"
    })
    .to_string();
    let (code_stop, _, stderr_stop) =
        run_hook_subcommand_with_env("record-handoff", Some(&stop_payload), &[]);
    assert_eq!(code_stop, Some(0), "stderr: {stderr_stop}");

    let start_payload = serde_json::json!({
        "session_id": "claude-start-1",
        "cwd": ws,
        "hook_event_name": "SessionStart"
    })
    .to_string();
    let (code_start, stdout_start, stderr_start) =
        run_hook_subcommand_with_env("inject-wake", Some(&start_payload), &[]);
    assert_eq!(code_start, Some(0), "stderr: {stderr_start}");
    let v_start: serde_json::Value =
        serde_json::from_str(&stdout_start).expect("valid Claude SessionStart JSON");
    assert_eq!(
        v_start
            .pointer("/hookSpecificOutput/hookEventName")
            .and_then(serde_json::Value::as_str),
        Some("SessionStart")
    );
    assert!(
        v_start
            .pointer("/hookSpecificOutput/additionalContext")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| !s.is_empty()),
        "expected non-empty SessionStart additionalContext: {stdout_start}"
    );

    // 3. Claude UserPromptSubmit (PreInvocation via detect-wip) emits
    //    UserPromptSubmit hookSpecificOutput when workspace has uncommitted VCS
    //    changes
    let git_init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&ws)
        .status()
        .expect("git init");
    assert!(git_init.success());
    std::fs::write(ws.join("dirty.txt"), "uncommitted change\n").expect("write dirty.txt");
    let prompt_payload = serde_json::json!({
        "session_id": "claude-prompt-1",
        "cwd": ws,
        "hook_event_name": "UserPromptSubmit",
        "prompt": "Continue working"
    })
    .to_string();
    let (code_wip, stdout_wip, stderr_wip) =
        run_hook_subcommand_with_env("detect-wip", Some(&prompt_payload), &[]);
    assert_eq!(code_wip, Some(0), "stderr: {stderr_wip}");
    let v_wip: serde_json::Value =
        serde_json::from_str(&stdout_wip).expect("valid Claude UserPromptSubmit JSON");
    assert_eq!(
        v_wip
            .pointer("/hookSpecificOutput/hookEventName")
            .and_then(serde_json::Value::as_str),
        Some("UserPromptSubmit")
    );
    assert!(
        v_wip
            .pointer("/hookSpecificOutput/additionalContext")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| s.contains("dirty.txt")),
        "expected WIP path in additionalContext: {stdout_wip}"
    );

    std::fs::remove_dir_all(&ws).expect("cleanup");
}

#[test]
fn test_gemini_cursor_copilot_wire_dialects_e2e() {
    let ws = std::env::temp_dir().join(format!("tkg_multi_dialect_e2e_{}", std::process::id()));
    _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).expect("create temp workspace");
    std::fs::write(
        ws.join("tools.toml"),
        "[tools.custom_mutator]\nmutating = true\naction = \"static_command\"\n",
    )
    .expect("write tools.toml");

    // 1. Gemini CLI BeforeTool emits top-level decision = "ask" with exit 0
    let gem_payload = serde_json::json!({
        "session_id": "gem-e2e-1",
        "cwd": ws,
        "hook_event_name": "BeforeTool",
        "tool_name": "custom_mutator",
        "tool_input": {}
    })
    .to_string();
    let (gem_code, gem_out, gem_err) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&gem_payload), &[]);
    assert_eq!(gem_code, Some(0), "stderr: {gem_err}");
    let gem_v: serde_json::Value = serde_json::from_str(&gem_out).expect("valid Gemini JSON");
    assert_eq!(
        gem_v.get("decision").and_then(serde_json::Value::as_str),
        Some("ask")
    );

    // 2. Cursor preToolUse emits Cursor's { "permission": "ask" }
    let cur_payload = serde_json::json!({
        "conversation_id": "cur-e2e-1",
        "cwd": ws,
        "hook_event_name": "preToolUse",
        "tool_name": "custom_mutator",
        "tool_input": {}
    })
    .to_string();
    let (cur_code, cur_out, cur_err) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&cur_payload), &[]);
    assert_eq!(cur_code, Some(0), "stderr: {cur_err}");
    let cur_v: serde_json::Value = serde_json::from_str(&cur_out).expect("valid Cursor JSON");
    assert_eq!(
        cur_v.get("permission").and_then(serde_json::Value::as_str),
        Some("ask")
    );

    // 3. GitHub Copilot camelCase toolName/toolArgs emits hookSpecificOutput.permissionDecision
    let gh_payload = serde_json::json!({
        "cwd": ws,
        "hook_event_name": "preToolUse",
        "toolName": "custom_mutator",
        "toolArgs": "{}"
    })
    .to_string();
    let (gh_code, gh_out, gh_err) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&gh_payload), &[]);
    assert_eq!(gh_code, Some(0), "stderr: {gh_err}");
    let gh_v: serde_json::Value = serde_json::from_str(&gh_out).expect("valid Copilot JSON");
    assert_eq!(
        gh_v.pointer("/hookSpecificOutput/permissionDecision")
            .and_then(serde_json::Value::as_str),
        Some("ask")
    );

    _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn test_zero_toml_workspace_enforces_manifest_guard_and_circuit_breaker() {
    let ws = std::env::temp_dir().join(format!("tkg_zero_toml_e2e_{}", std::process::id()));
    _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).expect("create empty ws");

    // 1. No .tkg/tools.toml and no .tkg/hooks.toml: native `Edit` on Cargo.toml is still blocked.
    let manifest_payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "cwd": ws,
        "tool_name": "Edit",
        "tool_input": { "file_path": "Cargo.toml" }
    })
    .to_string();
    let (code, out, err) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&manifest_payload), &[]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(
        out.contains("MANIFEST MUTATION GUARD"),
        "expected manifest guard even with zero TOMLs, got: {out}"
    );

    // 2. Trip the 2-strike breaker in this zero-TOML workspace.
    let mut ledger = tkg_common::CircuitBreakerLedger::new("default");
    ledger
        .record_failure(
            &ws,
            Some("default"),
            Some("cargo_test"),
            Some("E0308"),
            None,
        )
        .expect("strike 1");
    ledger
        .record_failure(
            &ws,
            Some("default"),
            Some("cargo_test"),
            Some("E0308"),
            None,
        )
        .expect("strike 2");

    // Mutating native host `Write` on a normal source file must now be halted.
    let write_src_payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "cwd": ws,
        "tool_name": "Write",
        "tool_input": { "file_path": "src/main.rs" }
    })
    .to_string();
    let (w_code, w_out, w_err) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&write_src_payload), &[]);
    assert_eq!(w_code, Some(0), "stderr: {w_err}");
    assert!(
        w_out.contains("CIRCUIT BREAKER TRIPPED"),
        "expected tripped breaker on native Write without tools.toml, got: {w_out}"
    );

    // Read-only `Read` must still be allowed so the agent can inspect errors.
    let read_payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "cwd": ws,
        "tool_name": "Read",
        "tool_input": { "file_path": "src/main.rs" }
    })
    .to_string();
    let (r_code, r_out, r_err) =
        run_hook_subcommand_with_env("pre-tool-use", Some(&read_payload), &[]);
    assert_eq!(r_code, Some(0), "stderr: {r_err}");
    assert!(
        !r_out.contains("CIRCUIT BREAKER TRIPPED"),
        "read-only tool should stay allowed when breaker is tripped, got: {r_out}"
    );

    _ = std::fs::remove_dir_all(&ws);
}

mod prop_e2e_tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn prop_e2e_binary_blocks_nested_manifests_across_all_dialects(
            dialect_idx in 0usize..4,
            manifest_idx in 0usize..tkg_common::DEFAULT_PROTECTED_MANIFESTS.len(),
            tool_name in prop_oneof![Just("Edit"), Just("Write"), Just("edit_file"), Just("apply_patch")],
            subdir in prop::option::of("[a-z]{2,7}"),
        ) {
            let ws = std::env::temp_dir().join(format!("tkg_e2e_prop_{}", std::process::id()));
            _ = std::fs::remove_dir_all(&ws);
            std::fs::create_dir_all(&ws).expect("create e2e ws");

            let manifest = tkg_common::DEFAULT_PROTECTED_MANIFESTS
                .get(manifest_idx)
                .copied()
                .unwrap_or("Cargo.toml");
            let target_path = subdir.map_or_else(
                || manifest.to_owned(),
                |d| format!("{d}/{manifest}"),
            );

            // Place path inside nested edits/args to test deep recursive extraction end-to-end.
            let nested_args = serde_json::json!({
                "edits": [{ "file_path": target_path }]
            });
            let payload = match dialect_idx {
                0 => serde_json::json!({
                    "hook_event_name": "PreToolUse",
                    "cwd": ws,
                    "tool_name": tool_name,
                    "tool_input": nested_args,
                }),
                1 => serde_json::json!({
                    "hook_event_name": "BeforeTool",
                    "cwd": ws,
                    "tool_name": tool_name,
                    "tool_input": nested_args,
                }),
                2 => serde_json::json!({
                    "hook_event_name": "preToolUse",
                    "cwd": ws,
                    "tool_name": tool_name,
                    "tool_input": nested_args,
                }),
                _ => serde_json::json!({
                    "hook_event_name": "preToolUse",
                    "cwd": ws,
                    "toolName": tool_name,
                    "toolArgs": nested_args.to_string(),
                }),
            }
            .to_string();

            let (code, out, err) =
                run_hook_subcommand_with_env("pre-tool-use", Some(&payload), &[]);
            let _ = &err;
            prop_assert_eq!(code, Some(0));
            let is_ask = out.contains("MANIFEST MUTATION GUARD")
                && (out.contains("ask") || out.contains("hookSpecificOutput"));
            prop_assert!(is_ask, "dialect {} allowed {}: {}", dialect_idx, target_path, out);

            _ = std::fs::remove_dir_all(&ws);
        }

        #[test]
        fn prop_e2e_binary_survives_arbitrary_malformed_stdin(
            raw_stdin in "\\PC{0,160}",
        ) {
            let (code, _out, _err) =
                run_hook_subcommand_with_env("pre-tool-use", Some(&raw_stdin), &[]);
            // Binary must always terminate cleanly with a defined exit code (never panic/signal-abort).
            prop_assert!(code.is_some());
        }
    }
}
