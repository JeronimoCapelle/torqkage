//! Hook wire dialect abstraction and `stdin`/`stdout` JSON helpers for
//! lifecycle hooks.
//!
//! Each execution harness (Antigravity, Claude Code, `OpenCode`, etc.) can
//! speak a slightly different JSON dialect over `stdin`/`stdout`.
//! [`HookDialect`] isolates wire translation so all hook evaluators operate
//! purely on the canonical [`tkg_common`] types.

use std::io::IsTerminal;

use tkg_common::{
    CommonHookPayload, PreInvocationInput, PreInvocationResponse, PreToolResponse, PreToolUseInput,
    SessionStartInput, SessionStartResponse, StopInput, StopResponse, ToolCallPayload,
};

/// Wire-protocol adapter translating between a harness's hook JSON and
/// `TorqKage`'s canonical hook types.
pub(crate) trait HookDialect: Send + Sync {
    /// Parses a raw `stdin` JSON string into a canonical [`PreToolUseInput`].
    fn parse_pre_tool_use(&self, raw: &str) -> Result<PreToolUseInput, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Formats a canonical [`PreToolResponse`] into the harness's `stdout` JSON
    /// string.
    fn format_pre_tool_use(&self, resp: &PreToolResponse) -> Result<String, serde_json::Error> {
        serde_json::to_string(resp)
    }

    /// Parses a raw `stdin` JSON string into a canonical
    /// [`PreInvocationInput`].
    fn parse_pre_invocation(&self, raw: &str) -> Result<PreInvocationInput, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Formats a canonical [`PreInvocationResponse`] into the harness's
    /// `stdout` JSON string.
    fn format_pre_invocation(
        &self,
        resp: &PreInvocationResponse,
    ) -> Result<String, serde_json::Error> {
        serde_json::to_string(resp)
    }

    /// Parses a raw `stdin` JSON string into a canonical [`StopInput`].
    fn parse_stop(&self, raw: &str) -> Result<StopInput, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Formats a canonical [`StopResponse`] into the harness's `stdout` JSON
    /// string.
    fn format_stop(&self, resp: &StopResponse) -> Result<String, serde_json::Error> {
        serde_json::to_string(resp)
    }

    /// Parses a raw `stdin` JSON string into a canonical [`SessionStartInput`].
    fn parse_session_start(&self, raw: &str) -> Result<SessionStartInput, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Formats a canonical [`SessionStartResponse`] into the harness's `stdout`
    /// JSON string.
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

fn extract_inject_steps_text(steps: &[tkg_common::InjectStep]) -> String {
    steps
        .iter()
        .map(|step| match step {
            tkg_common::InjectStep::System { system_message } => system_message.text.as_str(),
            tkg_common::InjectStep::User { user_message } => user_message.as_str(),
            tkg_common::InjectStep::Ephemeral { ephemeral_message } => ephemeral_message.as_str(),
        })
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Default `ProtoJSON` dialect used by Antigravity and generic harnesses.
pub(crate) struct ProtoJsonDialect;

impl HookDialect for ProtoJsonDialect {
    fn parse_pre_tool_use(&self, raw: &str) -> Result<PreToolUseInput, serde_json::Error> {
        let mut parsed: PreToolUseInput = serde_json::from_str(raw)?;
        // Also normalize Claude Code / snake_case `tool_name` + `tool_input` if
        // `toolCall` was omitted
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
pub(crate) struct ClaudeCodeDialect;

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
            tkg_common::HookDecision::Allow => "allow",
            tkg_common::HookDecision::Deny => "deny",
            tkg_common::HookDecision::Ask | tkg_common::HookDecision::ForceAsk => "ask",
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

static PROTO_JSON: ProtoJsonDialect = ProtoJsonDialect;
static CLAUDE_CODE: ClaudeCodeDialect = ClaudeCodeDialect;

/// Returns true if a raw `stdin` JSON payload uses Claude Code hook fields
/// (`tool_name` or `hook_event_name` without Antigravity `toolCall`).
#[must_use]
pub(crate) fn is_claude_payload(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw).is_ok_and(|val| {
        val.get("toolCall").is_none()
            && (val.get("hook_event_name").is_some() || val.get("tool_name").is_some())
    })
}

/// Returns true if the current environment indicates execution under Claude
/// Code.
#[must_use]
pub(crate) fn is_claude_harness() -> bool {
    std::env::var("CLAUDE_PROJECT_DIR").is_ok()
        || std::env::var("TKG_HARNESS").is_ok_and(|s| s.eq_ignore_ascii_case("claude"))
}

/// Returns true if execution is under Claude Code via environment or workspace
/// configuration.
#[must_use]
pub(crate) fn is_claude_harness_for_root(root: &std::path::Path) -> bool {
    is_claude_harness()
        || tkg_common::harness::runner_for(root)
            .harness
            .eq_ignore_ascii_case("claude")
}

/// Returns true when the Claude Code wire dialect should be used for a hook
/// invocation given workspace config, environment, and incoming payload shape.
#[must_use]
pub(crate) fn is_claude_for_context(root: &std::path::Path, claude_payload: bool) -> bool {
    claude_payload || is_claude_harness_for_root(root)
}

#[must_use]
fn dialect_for(use_claude: bool) -> &'static dyn HookDialect {
    if use_claude {
        &CLAUDE_CODE
    } else {
        &PROTO_JSON
    }
}

fn read_raw_stdin() -> Option<String> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    let mut buffer = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut buffer).is_err() {
        return None;
    }
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Reads a `PreToolUseInput` payload from `stdin` and reports whether the
/// payload itself used Claude Code wire fields.
#[must_use]
pub(crate) fn read_pre_tool_use_context() -> (PreToolUseInput, bool) {
    read_stdin_context("PreToolUse", |d, raw| d.parse_pre_tool_use(raw))
}

#[must_use]
fn read_stdin_context<T: Default>(
    label: &str,
    parse: impl FnOnce(&dyn HookDialect, &str) -> Result<T, serde_json::Error>,
) -> (T, bool) {
    let Some(raw) = read_raw_stdin() else {
        return (T::default(), false);
    };
    let claude_payload = is_claude_payload(&raw);
    let dialect = dialect_for(claude_payload || is_claude_harness());
    match parse(dialect, &raw) {
        Ok(val) => (val, claude_payload),
        Err(err) => {
            eprintln!("tkg-hooks: warning: failed to parse {label} stdin payload: {err}");
            (T::default(), claude_payload)
        },
    }
}

/// Emits a `PreToolResponse` to `stdout` in the resolved harness dialect and
/// returns the corresponding process exit code.
pub(crate) fn finalize_pre_tool_use(
    resp: &PreToolResponse,
    root: &std::path::Path,
    claude_payload: bool,
    is_config_error: bool,
) -> std::process::ExitCode {
    let use_claude = is_claude_for_context(root, claude_payload);
    if let Ok(json) = dialect_for(use_claude).format_pre_tool_use(resp) {
        println!("{json}");
    }

    match resp.decision {
        tkg_common::HookDecision::Deny if use_claude => {
            if let Some(ref reason) = resp.reason {
                eprintln!("TorqKage policy violation: {reason}");
            }
            std::process::ExitCode::from(2)
        },
        tkg_common::HookDecision::Deny if is_config_error => std::process::ExitCode::FAILURE,
        tkg_common::HookDecision::Ask | tkg_common::HookDecision::ForceAsk if use_claude => {
            // Claude Code only reads `hookSpecificOutput.permissionDecision = "ask"`
            // when the hook exits cleanly with status 0.
            std::process::ExitCode::SUCCESS
        },
        _ => std::process::ExitCode::SUCCESS,
    }
}

/// Reads and deserializes a `PreInvocationInput` payload from `stdin` and reports
/// whether the payload used Claude Code fields.
#[must_use]
pub(crate) fn read_pre_invocation_context() -> (PreInvocationInput, bool) {
    read_stdin_context("PreInvocation", |d, raw| d.parse_pre_invocation(raw))
}

/// Emits a `PreInvocationResponse` to `stdout` in the resolved harness dialect.
pub(crate) fn write_pre_invocation_for_context(
    resp: &PreInvocationResponse,
    root: &std::path::Path,
    claude_payload: bool,
) {
    let dialect = dialect_for(is_claude_for_context(root, claude_payload));
    if let Ok(json) = dialect.format_pre_invocation(resp) {
        println!("{json}");
    }
}

/// Reads and deserializes a `StopInput` payload from `stdin` and reports whether
/// the payload used Claude Code fields.
#[must_use]
pub(crate) fn read_stop_context() -> (StopInput, bool) {
    read_stdin_context("Stop", |d, raw| d.parse_stop(raw))
}

/// Emits a `StopResponse` to `stdout` in the resolved harness dialect.
pub(crate) fn write_stop_for_context(
    resp: &StopResponse,
    root: &std::path::Path,
    claude_payload: bool,
) {
    let dialect = dialect_for(is_claude_for_context(root, claude_payload));
    if let Ok(json) = dialect.format_stop(resp) {
        println!("{json}");
    }
}

/// Reads and deserializes a `SessionStartInput` payload from `stdin` and reports
/// whether the payload used Claude Code fields.
#[must_use]
pub(crate) fn read_session_start_context() -> (SessionStartInput, bool) {
    read_stdin_context("SessionStart", |d, raw| d.parse_session_start(raw))
}

/// Emits a `SessionStartResponse` to `stdout` in the resolved harness dialect.
pub(crate) fn write_session_start_for_context(
    resp: &SessionStartResponse,
    root: &std::path::Path,
    claude_payload: bool,
) {
    let dialect = dialect_for(is_claude_for_context(root, claude_payload));
    if let Ok(json) = dialect.format_session_start(resp) {
        println!("{json}");
    }
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
        let parsed = dialect_for(is_claude_payload(raw))
            .parse_pre_tool_use(raw)
            .unwrap();
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
