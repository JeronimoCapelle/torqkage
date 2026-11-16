//! Hook wire dialect abstraction for translating between harness-specific
//! lifecycle JSON (`ProtoJSON`, Claude Code / Codex, Gemini CLI, Cursor,
//! GitHub Copilot) and `TorqKage`'s canonical models.

use std::process::ExitCode;

use crate::models::{
    CommonHookPayload, HookDecision, InjectStep, PreInvocationInput, PreInvocationResponse,
    PreToolResponse, PreToolUseInput, SessionStartInput, SessionStartResponse, StopInput,
    StopResponse, ToolCallPayload,
};

/// Wire-protocol adapter translating between a harness's hook JSON and
/// `TorqKage`'s canonical hook types.
pub trait HookDialect: Send + Sync {
    /// Parses a raw `stdin` JSON string into a canonical [`PreToolUseInput`].
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if the JSON payload cannot be parsed.
    fn parse_pre_tool_use(&self, raw: &str) -> Result<PreToolUseInput, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Formats a canonical [`PreToolResponse`] into the harness's `stdout` JSON
    /// string.
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if serialization fails.
    fn format_pre_tool_use(&self, resp: &PreToolResponse) -> Result<String, serde_json::Error> {
        serde_json::to_string(resp)
    }

    /// Computes the hook process exit code and optional `stderr` diagnostic for
    /// a [`PreToolResponse`].
    fn pre_tool_use_exit(
        &self,
        resp: &PreToolResponse,
        is_config_error: bool,
    ) -> (ExitCode, Option<String>) {
        match resp.decision {
            HookDecision::Deny if is_config_error => (ExitCode::FAILURE, None),
            _ => (ExitCode::SUCCESS, None),
        }
    }

    /// Parses a raw `stdin` JSON string into a canonical
    /// [`PreInvocationInput`].
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if the JSON payload cannot be parsed.
    fn parse_pre_invocation(&self, raw: &str) -> Result<PreInvocationInput, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Formats a canonical [`PreInvocationResponse`] into the harness's
    /// `stdout` JSON string.
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if serialization fails.
    fn format_pre_invocation(
        &self,
        resp: &PreInvocationResponse,
    ) -> Result<String, serde_json::Error> {
        serde_json::to_string(resp)
    }

    /// Parses a raw `stdin` JSON string into a canonical [`StopInput`].
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if the JSON payload cannot be parsed.
    fn parse_stop(&self, raw: &str) -> Result<StopInput, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Formats a canonical [`StopResponse`] into the harness's `stdout` JSON
    /// string.
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if serialization fails.
    fn format_stop(&self, resp: &StopResponse) -> Result<String, serde_json::Error> {
        serde_json::to_string(resp)
    }

    /// Parses a raw `stdin` JSON string into a canonical [`SessionStartInput`].
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if the JSON payload cannot be parsed.
    fn parse_session_start(&self, raw: &str) -> Result<SessionStartInput, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Formats a canonical [`SessionStartResponse`] into the harness's `stdout`
    /// JSON string.
    ///
    /// # Errors
    /// Returns [`serde_json::Error`] if serialization fails.
    fn format_session_start(
        &self,
        resp: &SessionStartResponse,
    ) -> Result<String, serde_json::Error> {
        serde_json::to_string(resp)
    }
}

fn normalize_common_from_raw(raw: &str, common: &mut CommonHookPayload) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    if common.workspace_paths.is_empty()
        && let Some(cwd) = val.get("cwd").and_then(serde_json::Value::as_str)
    {
        common.workspace_paths = vec![std::path::PathBuf::from(cwd)];
    }
    if common.conversation_id.is_none()
        && let Some(session_id) = val
            .get("session_id")
            .or_else(|| val.get("conversation_id"))
            .and_then(serde_json::Value::as_str)
    {
        common.conversation_id = Some(session_id.to_owned());
    }
    if common.transcript_path.is_none()
        && let Some(tp) = val
            .get("transcript_path")
            .and_then(serde_json::Value::as_str)
    {
        common.transcript_path = Some(std::path::PathBuf::from(tp));
    }
}

fn parse_tool_args_value(raw_args: Option<&serde_json::Value>) -> serde_json::Value {
    match raw_args {
        Some(serde_json::Value::String(s)) => serde_json::from_str::<serde_json::Value>(s)
            .unwrap_or_else(|_| serde_json::json!({ "raw": s })),
        Some(v) => v.clone(),
        None => serde_json::json!({}),
    }
}

fn exit_code_2_on_deny(
    resp: &PreToolResponse,
    _is_config_error: bool,
) -> (ExitCode, Option<String>) {
    match resp.decision {
        HookDecision::Deny => (
            ExitCode::from(2),
            resp.reason
                .as_ref()
                .map(|reason| format!("TorqKage policy violation: {reason}")),
        ),
        HookDecision::Ask | HookDecision::ForceAsk | HookDecision::Allow => {
            (ExitCode::SUCCESS, None)
        },
    }
}

fn format_hook_specific_permission(
    event_name: Option<&str>,
    resp: &PreToolResponse,
) -> Result<String, serde_json::Error> {
    let decision_str = match resp.decision {
        HookDecision::Allow => "allow",
        HookDecision::Deny => "deny",
        HookDecision::Ask | HookDecision::ForceAsk => "ask",
    };
    let mut hook_output = serde_json::Map::new();
    if let Some(ev) = event_name {
        hook_output.insert("hookEventName".to_owned(), serde_json::json!(ev));
    }
    hook_output.insert(
        "permissionDecision".to_owned(),
        serde_json::json!(decision_str),
    );
    if let Some(ref reason) = resp.reason {
        hook_output.insert(
            "permissionDecisionReason".to_owned(),
            serde_json::json!(reason),
        );
    }
    serde_json::to_string(&serde_json::json!({
        "hookSpecificOutput": serde_json::Value::Object(hook_output)
    }))
}

fn extract_inject_steps_text(steps: &[InjectStep]) -> String {
    steps
        .iter()
        .map(|step| match step {
            InjectStep::System { system_message } => system_message.text.as_str(),
            InjectStep::User { user_message } => user_message.as_str(),
            InjectStep::Ephemeral { ephemeral_message } => ephemeral_message.as_str(),
        })
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Default `ProtoJSON` dialect used by Antigravity and generic harnesses, with
/// cross-harness `stdin` normalization for `tool_name`, `toolName`, and
/// `beforeShellExecution`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProtoJsonDialect;

impl HookDialect for ProtoJsonDialect {
    fn parse_pre_tool_use(&self, raw: &str) -> Result<PreToolUseInput, serde_json::Error> {
        let mut parsed: PreToolUseInput = serde_json::from_str(raw)?;
        if parsed.tool_call.is_none()
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(raw)
        {
            if let Some(name) = val
                .get("tool_name")
                .or_else(|| val.get("toolName"))
                .and_then(serde_json::Value::as_str)
            {
                let args = parse_tool_args_value(
                    val.get("tool_input")
                        .or_else(|| val.get("toolArgs"))
                        .or_else(|| val.get("args")),
                );
                parsed.tool_call = Some(ToolCallPayload {
                    name: name.to_owned(),
                    args,
                });
            } else if val
                .get("hook_event_name")
                .and_then(serde_json::Value::as_str)
                == Some("beforeShellExecution")
                && let Some(cmd) = val.get("command").and_then(serde_json::Value::as_str)
            {
                parsed.tool_call = Some(ToolCallPayload {
                    name: "Shell".to_owned(),
                    args: serde_json::json!({ "command": cmd }),
                });
            }
        }
        normalize_common_from_raw(raw, &mut parsed.common);
        Ok(parsed)
    }

    fn parse_pre_invocation(&self, raw: &str) -> Result<PreInvocationInput, serde_json::Error> {
        let mut parsed: PreInvocationInput = serde_json::from_str(raw)?;
        normalize_common_from_raw(raw, &mut parsed.common);
        Ok(parsed)
    }

    fn parse_stop(&self, raw: &str) -> Result<StopInput, serde_json::Error> {
        let mut parsed: StopInput = serde_json::from_str(raw)?;
        normalize_common_from_raw(raw, &mut parsed.common);
        Ok(parsed)
    }

    fn parse_session_start(&self, raw: &str) -> Result<SessionStartInput, serde_json::Error> {
        let mut parsed: SessionStartInput = serde_json::from_str(raw)?;
        normalize_common_from_raw(raw, &mut parsed.common);
        Ok(parsed)
    }
}

/// Hook dialect tailored for Claude Code and `OpenAI` Codex CLI hook wire schemas.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCodeDialect;

impl HookDialect for ClaudeCodeDialect {
    fn parse_pre_tool_use(&self, raw: &str) -> Result<PreToolUseInput, serde_json::Error> {
        ProtoJsonDialect.parse_pre_tool_use(raw)
    }

    fn parse_pre_invocation(&self, raw: &str) -> Result<PreInvocationInput, serde_json::Error> {
        ProtoJsonDialect.parse_pre_invocation(raw)
    }

    fn parse_stop(&self, raw: &str) -> Result<StopInput, serde_json::Error> {
        ProtoJsonDialect.parse_stop(raw)
    }

    fn parse_session_start(&self, raw: &str) -> Result<SessionStartInput, serde_json::Error> {
        ProtoJsonDialect.parse_session_start(raw)
    }

    fn format_pre_tool_use(&self, resp: &PreToolResponse) -> Result<String, serde_json::Error> {
        format_hook_specific_permission(Some("PreToolUse"), resp)
    }

    fn pre_tool_use_exit(
        &self,
        resp: &PreToolResponse,
        is_config_error: bool,
    ) -> (ExitCode, Option<String>) {
        exit_code_2_on_deny(resp, is_config_error)
    }

    fn format_pre_invocation(
        &self,
        resp: &PreInvocationResponse,
    ) -> Result<String, serde_json::Error> {
        let ctx = resp
            .inject_steps
            .as_deref()
            .map_or_default(extract_inject_steps_text);
        if ctx.is_empty() {
            return Ok("{}".to_owned());
        }
        serde_json::to_string(&serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": ctx,
            }
        }))
    }

    fn format_session_start(
        &self,
        resp: &SessionStartResponse,
    ) -> Result<String, serde_json::Error> {
        let ctx = extract_inject_steps_text(&resp.inject_steps);
        if ctx.is_empty() {
            return Ok("{}".to_owned());
        }
        serde_json::to_string(&serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": ctx,
            }
        }))
    }
}

/// Hook dialect tailored for Google Gemini CLI (`BeforeTool` / `SessionStart`).
#[derive(Debug, Default, Clone, Copy)]
pub struct GeminiCliDialect;

impl HookDialect for GeminiCliDialect {
    fn parse_pre_tool_use(&self, raw: &str) -> Result<PreToolUseInput, serde_json::Error> {
        ProtoJsonDialect.parse_pre_tool_use(raw)
    }

    fn parse_pre_invocation(&self, raw: &str) -> Result<PreInvocationInput, serde_json::Error> {
        ProtoJsonDialect.parse_pre_invocation(raw)
    }

    fn parse_stop(&self, raw: &str) -> Result<StopInput, serde_json::Error> {
        ProtoJsonDialect.parse_stop(raw)
    }

    fn parse_session_start(&self, raw: &str) -> Result<SessionStartInput, serde_json::Error> {
        ProtoJsonDialect.parse_session_start(raw)
    }

    fn format_pre_tool_use(&self, resp: &PreToolResponse) -> Result<String, serde_json::Error> {
        let decision = match resp.decision {
            HookDecision::Allow => "allow",
            HookDecision::Deny => "deny",
            HookDecision::Ask | HookDecision::ForceAsk => "ask",
        };
        let mut out = serde_json::json!({ "decision": decision });
        if let Some(ref reason) = resp.reason
            && let Some(map) = out.as_object_mut()
        {
            map.insert("reason".to_owned(), serde_json::json!(reason));
        }
        serde_json::to_string(&out)
    }

    fn format_pre_invocation(
        &self,
        resp: &PreInvocationResponse,
    ) -> Result<String, serde_json::Error> {
        let ctx = resp
            .inject_steps
            .as_deref()
            .map_or_default(extract_inject_steps_text);
        if ctx.is_empty() {
            return Ok("{}".to_owned());
        }
        serde_json::to_string(&serde_json::json!({
            "hookSpecificOutput": {
                "additionalContext": ctx,
            }
        }))
    }

    fn format_session_start(
        &self,
        resp: &SessionStartResponse,
    ) -> Result<String, serde_json::Error> {
        let ctx = extract_inject_steps_text(&resp.inject_steps);
        if ctx.is_empty() {
            return Ok("{}".to_owned());
        }
        serde_json::to_string(&serde_json::json!({
            "hookSpecificOutput": {
                "additionalContext": ctx,
            }
        }))
    }
}

/// Hook dialect tailored for Cursor (`.cursor/hooks.json`: `preToolUse`,
/// `beforeShellExecution`, `beforeMCPExecution`, `sessionStart`).
#[derive(Debug, Default, Clone, Copy)]
pub struct CursorDialect;

impl HookDialect for CursorDialect {
    fn parse_pre_tool_use(&self, raw: &str) -> Result<PreToolUseInput, serde_json::Error> {
        ProtoJsonDialect.parse_pre_tool_use(raw)
    }

    fn parse_pre_invocation(&self, raw: &str) -> Result<PreInvocationInput, serde_json::Error> {
        ProtoJsonDialect.parse_pre_invocation(raw)
    }

    fn parse_stop(&self, raw: &str) -> Result<StopInput, serde_json::Error> {
        ProtoJsonDialect.parse_stop(raw)
    }

    fn parse_session_start(&self, raw: &str) -> Result<SessionStartInput, serde_json::Error> {
        ProtoJsonDialect.parse_session_start(raw)
    }

    fn format_pre_tool_use(&self, resp: &PreToolResponse) -> Result<String, serde_json::Error> {
        let permission = match resp.decision {
            HookDecision::Allow => "allow",
            HookDecision::Deny => "deny",
            HookDecision::Ask | HookDecision::ForceAsk => "ask",
        };
        let mut out = serde_json::json!({ "permission": permission });
        if let Some(ref reason) = resp.reason
            && let Some(map) = out.as_object_mut()
        {
            map.insert("agentMessage".to_owned(), serde_json::json!(reason));
        }
        serde_json::to_string(&out)
    }

    fn pre_tool_use_exit(
        &self,
        resp: &PreToolResponse,
        is_config_error: bool,
    ) -> (ExitCode, Option<String>) {
        exit_code_2_on_deny(resp, is_config_error)
    }

    fn format_pre_invocation(
        &self,
        resp: &PreInvocationResponse,
    ) -> Result<String, serde_json::Error> {
        let ctx = resp
            .inject_steps
            .as_deref()
            .map_or_default(extract_inject_steps_text);
        if ctx.is_empty() {
            return Ok("{}".to_owned());
        }
        serde_json::to_string(&serde_json::json!({
            "additional_context": ctx,
        }))
    }

    fn format_session_start(
        &self,
        resp: &SessionStartResponse,
    ) -> Result<String, serde_json::Error> {
        let ctx = extract_inject_steps_text(&resp.inject_steps);
        if ctx.is_empty() {
            return Ok("{}".to_owned());
        }
        serde_json::to_string(&serde_json::json!({
            "additional_context": ctx,
        }))
    }
}

/// Hook dialect tailored for GitHub Copilot CLI (`.github/hooks/*.json`).
#[derive(Debug, Default, Clone, Copy)]
pub struct GithubCopilotDialect;

impl HookDialect for GithubCopilotDialect {
    fn parse_pre_tool_use(&self, raw: &str) -> Result<PreToolUseInput, serde_json::Error> {
        ProtoJsonDialect.parse_pre_tool_use(raw)
    }

    fn parse_pre_invocation(&self, raw: &str) -> Result<PreInvocationInput, serde_json::Error> {
        ProtoJsonDialect.parse_pre_invocation(raw)
    }

    fn parse_stop(&self, raw: &str) -> Result<StopInput, serde_json::Error> {
        ProtoJsonDialect.parse_stop(raw)
    }

    fn parse_session_start(&self, raw: &str) -> Result<SessionStartInput, serde_json::Error> {
        ProtoJsonDialect.parse_session_start(raw)
    }

    fn format_pre_tool_use(&self, resp: &PreToolResponse) -> Result<String, serde_json::Error> {
        format_hook_specific_permission(None, resp)
    }

    fn pre_tool_use_exit(
        &self,
        resp: &PreToolResponse,
        is_config_error: bool,
    ) -> (ExitCode, Option<String>) {
        exit_code_2_on_deny(resp, is_config_error)
    }

    fn format_pre_invocation(
        &self,
        resp: &PreInvocationResponse,
    ) -> Result<String, serde_json::Error> {
        ClaudeCodeDialect.format_pre_invocation(resp)
    }

    fn format_session_start(
        &self,
        resp: &SessionStartResponse,
    ) -> Result<String, serde_json::Error> {
        ClaudeCodeDialect.format_session_start(resp)
    }
}

/// Shared static [`ProtoJsonDialect`] instance.
pub static PROTO_JSON_DIALECT: ProtoJsonDialect = ProtoJsonDialect;
/// Shared static [`ClaudeCodeDialect`] instance.
pub static CLAUDE_CODE_DIALECT: ClaudeCodeDialect = ClaudeCodeDialect;
/// Shared static [`GeminiCliDialect`] instance.
pub static GEMINI_CLI_DIALECT: GeminiCliDialect = GeminiCliDialect;
/// Shared static [`CursorDialect`] instance.
pub static CURSOR_DIALECT: CursorDialect = CursorDialect;
/// Shared static [`GithubCopilotDialect`] instance.
pub static GITHUB_COPILOT_DIALECT: GithubCopilotDialect = GithubCopilotDialect;

/// Wire-protocol dialect inferred directly from raw `stdin` JSON payload keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireDialectHint {
    ClaudeCode,
    GeminiCli,
    Cursor,
    GithubCopilot,
}

/// Inspects a raw `stdin` JSON payload and infers a non-Antigravity
/// [`WireDialectHint`] when harness-specific wire keys are present.
#[must_use]
pub fn detect_payload_dialect(raw: &str) -> Option<WireDialectHint> {
    let val = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    if val.get("toolCall").is_some() {
        return None;
    }

    if let Some(event) = val
        .get("hook_event_name")
        .and_then(serde_json::Value::as_str)
    {
        return match event {
            "BeforeTool" | "AfterTool" | "BeforeAgent" | "AfterAgent" | "BeforeModel"
            | "AfterModel" => Some(WireDialectHint::GeminiCli),
            "beforeShellExecution"
            | "beforeMCPExecution"
            | "beforeSubmitPrompt"
            | "afterAgentResponse" => Some(WireDialectHint::Cursor),
            "preToolUse" | "sessionStart" | "stop" => {
                if val.get("toolName").is_some() || val.get("toolArgs").is_some() {
                    Some(WireDialectHint::GithubCopilot)
                } else {
                    Some(WireDialectHint::Cursor)
                }
            },
            _ => Some(WireDialectHint::ClaudeCode),
        };
    }

    if val.get("toolName").is_some() || val.get("toolArgs").is_some() {
        return Some(WireDialectHint::GithubCopilot);
    }
    if val.get("tool_name").is_some() {
        return Some(WireDialectHint::ClaudeCode);
    }
    None
}

/// Returns `true` if a raw `stdin` JSON payload uses Claude Code hook fields
/// (`tool_name` or `hook_event_name` without Antigravity `toolCall`).
#[must_use]
pub fn is_claude_payload(raw: &str) -> bool {
    matches!(
        detect_payload_dialect(raw),
        Some(WireDialectHint::ClaudeCode)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dialect_normalizes_claude_code_pre_tool_use() {
        let raw = r#"{
            "session_id": "sess-1",
            "transcript_path": "/tmp/transcript.jsonl",
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": { "file_path": "src/lib.rs", "content": "fn main() {}" },
            "cwd": "/tmp/ws"
        }"#;
        assert!(is_claude_payload(raw));
        let parsed = CLAUDE_CODE_DIALECT.parse_pre_tool_use(raw).unwrap();
        let tc = parsed.tool_call.unwrap();
        assert_eq!(tc.name, "Write");
        assert_eq!(
            parsed.common.workspace_paths.first(),
            Some(&std::path::PathBuf::from("/tmp/ws"))
        );
        assert_eq!(parsed.common.conversation_id.as_deref(), Some("sess-1"));
        assert_eq!(
            parsed.common.transcript_path.as_deref(),
            Some(std::path::Path::new("/tmp/transcript.jsonl"))
        );
    }

    #[test]
    fn test_proto_json_payload_is_not_detected_as_claude() {
        let raw = r#"{
            "toolCall": { "name": "modify_code", "args": { "path": "src/lib.rs" } },
            "workspacePaths": ["/tmp/ws"]
        }"#;
        assert!(!is_claude_payload(raw));
        assert_eq!(detect_payload_dialect(raw), None);
    }

    #[test]
    fn test_gemini_cli_dialect_roundtrip() {
        let raw = r#"{
            "session_id": "gem-42",
            "cwd": "/repo",
            "hook_event_name": "BeforeTool",
            "tool_name": "write_file",
            "tool_input": { "file_path": "Cargo.toml" }
        }"#;
        assert_eq!(
            detect_payload_dialect(raw),
            Some(WireDialectHint::GeminiCli)
        );
        let parsed = GEMINI_CLI_DIALECT.parse_pre_tool_use(raw).unwrap();
        assert_eq!(
            parsed.tool_call.as_ref().map(|t| t.name.as_str()),
            Some("write_file")
        );
        let formatted = GEMINI_CLI_DIALECT
            .format_pre_tool_use(&PreToolResponse::deny("blocked"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&formatted).unwrap();
        assert_eq!(
            v.get("decision").and_then(serde_json::Value::as_str),
            Some("deny")
        );
        assert_eq!(
            v.get("reason").and_then(serde_json::Value::as_str),
            Some("blocked")
        );

        let start = GEMINI_CLI_DIALECT
            .format_session_start(&SessionStartResponse {
                inject_steps: vec![InjectStep::Ephemeral {
                    ephemeral_message: "wake-ctx".to_owned(),
                }],
            })
            .unwrap();
        let sv: serde_json::Value = serde_json::from_str(&start).unwrap();
        assert_eq!(
            sv.pointer("/hookSpecificOutput/additionalContext")
                .and_then(serde_json::Value::as_str),
            Some("wake-ctx")
        );
    }

    #[test]
    fn test_cursor_dialect_shell_and_permission_roundtrip() {
        let raw = r#"{
            "conversation_id": "cur-9",
            "cwd": "/ws",
            "hook_event_name": "beforeShellExecution",
            "command": "rm -rf /"
        }"#;
        assert_eq!(detect_payload_dialect(raw), Some(WireDialectHint::Cursor));
        let parsed = CURSOR_DIALECT.parse_pre_tool_use(raw).unwrap();
        let tc = parsed.tool_call.unwrap();
        assert_eq!(tc.name, "Shell");
        assert_eq!(
            tc.args.get("command").and_then(serde_json::Value::as_str),
            Some("rm -rf /")
        );
        assert_eq!(parsed.common.conversation_id.as_deref(), Some("cur-9"));

        let out = CURSOR_DIALECT
            .format_pre_tool_use(&PreToolResponse::ask("confirm"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v.get("permission").and_then(serde_json::Value::as_str),
            Some("ask")
        );
        assert_eq!(
            v.get("agentMessage").and_then(serde_json::Value::as_str),
            Some("confirm")
        );
    }

    #[test]
    fn test_github_copilot_dialect_camel_case() {
        let raw = r#"{
            "cwd": "/ws",
            "hook_event_name": "preToolUse",
            "toolName": "edit",
            "toolArgs": "{\"path\":\"Cargo.toml\"}"
        }"#;
        assert_eq!(
            detect_payload_dialect(raw),
            Some(WireDialectHint::GithubCopilot)
        );
        let parsed = GITHUB_COPILOT_DIALECT.parse_pre_tool_use(raw).unwrap();
        let tc = parsed.tool_call.unwrap();
        assert_eq!(tc.name, "edit");
        assert_eq!(
            tc.args.get("path").and_then(serde_json::Value::as_str),
            Some("Cargo.toml")
        );

        let out = GITHUB_COPILOT_DIALECT
            .format_pre_tool_use(&PreToolResponse::deny("no"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v.pointer("/hookSpecificOutput/permissionDecision")
                .and_then(serde_json::Value::as_str),
            Some("deny")
        );
    }
}
