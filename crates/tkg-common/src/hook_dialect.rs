//! Hook wire dialect abstraction for translating between harness-specific
//! lifecycle JSON (`ProtoJSON`, Claude Code) and `TorqKage`'s canonical models.

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
        && let Some(session_id) = val.get("session_id").and_then(serde_json::Value::as_str)
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

/// Default `ProtoJSON` dialect used by Antigravity and generic harnesses.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProtoJsonDialect;

impl HookDialect for ProtoJsonDialect {
    fn parse_pre_tool_use(&self, raw: &str) -> Result<PreToolUseInput, serde_json::Error> {
        let mut parsed: PreToolUseInput = serde_json::from_str(raw)?;
        if parsed.tool_call.is_none()
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(raw)
            && let Some(name) = val.get("tool_name").and_then(serde_json::Value::as_str)
        {
            let args = val
                .get("tool_input")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            parsed.tool_call = Some(ToolCallPayload {
                name: name.to_owned(),
                args,
            });
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

/// Hook dialect tailored for Claude Code hook wire schemas.
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
        let decision_str = match resp.decision {
            HookDecision::Allow => "allow",
            HookDecision::Deny => "deny",
            HookDecision::Ask | HookDecision::ForceAsk => "ask",
        };
        let mut hook_output = serde_json::json!({
            "hookEventName": "PreToolUse",
            "permissionDecision": decision_str,
        });
        if let Some(ref reason) = resp.reason
            && let Some(map) = hook_output.as_object_mut()
        {
            map.insert(
                "permissionDecisionReason".to_owned(),
                serde_json::json!(reason),
            );
        }
        serde_json::to_string(&serde_json::json!({
            "hookSpecificOutput": hook_output
        }))
    }

    fn pre_tool_use_exit(
        &self,
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

/// Shared static [`ProtoJsonDialect`] instance.
pub static PROTO_JSON_DIALECT: ProtoJsonDialect = ProtoJsonDialect;
/// Shared static [`ClaudeCodeDialect`] instance.
pub static CLAUDE_CODE_DIALECT: ClaudeCodeDialect = ClaudeCodeDialect;

/// Returns `true` if a raw `stdin` JSON payload uses Claude Code hook fields
/// (`tool_name` or `hook_event_name` without Antigravity `toolCall`).
#[must_use]
pub fn is_claude_payload(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw).is_ok_and(|val| {
        val.get("toolCall").is_none()
            && (val.get("hook_event_name").is_some() || val.get("tool_name").is_some())
    })
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
    }
}
