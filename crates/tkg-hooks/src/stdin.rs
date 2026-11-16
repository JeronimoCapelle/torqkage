//! `stdin`/`stdout` JSON wire helpers for lifecycle hooks.
//!
//! Resolves the active [`Harness`] from
//! environment signals, workspace configuration, and payload shape, then
//! delegates all serialization and exit-code semantics to
//! [`HookDialect`].

use std::io::IsTerminal;

use tkg_common::{
    PreInvocationInput, PreInvocationResponse, PreToolResponse, PreToolUseInput, SessionStartInput,
    SessionStartResponse, StopInput, StopResponse,
    harness::{self, Harness},
    hook_dialect::{HookDialect, WireDialectHint, detect_payload_dialect},
};

fn resolve_harness_from_env() -> Option<Box<dyn Harness>> {
    if std::env::var_os("CLAUDE_PROJECT_DIR").is_some() {
        return Some(Box::new(harness::ClaudeCode));
    }
    if let Ok(raw) = std::env::var("TKG_HARNESS")
        && !raw.trim().is_empty()
    {
        return Some(harness::resolve_client_name(&raw));
    }
    None
}

fn harness_for_hint(hint: WireDialectHint) -> Box<dyn Harness> {
    match hint {
        WireDialectHint::ClaudeCode => Box::new(harness::ClaudeCode),
        WireDialectHint::GeminiCli => Box::new(harness::GeminiCli),
        WireDialectHint::Cursor => Box::new(harness::Cursor),
        WireDialectHint::GithubCopilot => Box::new(harness::GithubCopilot),
    }
}

fn resolve_harness_for_context(
    root: &std::path::Path,
    hint: Option<WireDialectHint>,
) -> Box<dyn Harness> {
    if let Some(h) = hint {
        return harness_for_hint(h);
    }
    if let Some(h) = resolve_harness_from_env() {
        return h;
    }
    harness::runner_for(root).harness()
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

/// Reads a `PreToolUseInput` payload from `stdin` and reports any detected
/// wire dialect hint.
#[must_use]
pub(crate) fn read_pre_tool_use_context() -> (PreToolUseInput, Option<WireDialectHint>) {
    read_stdin_context("PreToolUse", |d, raw| d.parse_pre_tool_use(raw))
}

#[must_use]
fn read_stdin_context<T: Default>(
    label: &str,
    parse: impl FnOnce(&dyn HookDialect, &str) -> Result<T, serde_json::Error>,
) -> (T, Option<WireDialectHint>) {
    let Some(raw) = read_raw_stdin() else {
        return (T::default(), None);
    };
    let hint = detect_payload_dialect(&raw);
    let harness: Box<dyn Harness> = hint.map_or_else(
        || resolve_harness_from_env().unwrap_or_else(|| Box::new(harness::Antigravity)),
        harness_for_hint,
    );
    match parse(harness.hook_dialect(), &raw) {
        Ok(val) => (val, hint),
        Err(err) => {
            tracing::warn!(hook = label, "Failed to parse stdin payload: {err}");
            (T::default(), hint)
        },
    }
}

/// Emits a `PreToolResponse` to `stdout` in the resolved harness dialect and
/// returns the corresponding process exit code.
pub(crate) fn finalize_pre_tool_use(
    resp: &PreToolResponse,
    root: &std::path::Path,
    hint: Option<WireDialectHint>,
    is_config_error: bool,
) -> std::process::ExitCode {
    let dialect = resolve_harness_for_context(root, hint).hook_dialect();
    if let Ok(json) = dialect.format_pre_tool_use(resp) {
        println!("{json}");
    }
    let (exit_code, stderr_notice) = dialect.pre_tool_use_exit(resp, is_config_error);
    if let Some(msg) = stderr_notice {
        eprintln!("{msg}");
    }
    exit_code
}

/// Reads and deserializes a `PreInvocationInput` payload from `stdin` and
/// reports any detected wire dialect hint.
#[must_use]
pub(crate) fn read_pre_invocation_context() -> (PreInvocationInput, Option<WireDialectHint>) {
    read_stdin_context("PreInvocation", |d, raw| d.parse_pre_invocation(raw))
}

/// Emits a `PreInvocationResponse` to `stdout` in the resolved harness dialect.
pub(crate) fn write_pre_invocation_for_context(
    resp: &PreInvocationResponse,
    root: &std::path::Path,
    hint: Option<WireDialectHint>,
) {
    let dialect = resolve_harness_for_context(root, hint).hook_dialect();
    if let Ok(json) = dialect.format_pre_invocation(resp) {
        println!("{json}");
    }
}

/// Reads and deserializes a `StopInput` payload from `stdin` and reports
/// any detected wire dialect hint.
#[must_use]
pub(crate) fn read_stop_context() -> (StopInput, Option<WireDialectHint>) {
    read_stdin_context("Stop", |d, raw| d.parse_stop(raw))
}

/// Emits a `StopResponse` to `stdout` in the resolved harness dialect.
pub(crate) fn write_stop_for_context(
    resp: &StopResponse,
    root: &std::path::Path,
    hint: Option<WireDialectHint>,
) {
    let dialect = resolve_harness_for_context(root, hint).hook_dialect();
    if let Ok(json) = dialect.format_stop(resp) {
        println!("{json}");
    }
}

/// Reads and deserializes a `SessionStartInput` payload from `stdin` and
/// reports any detected wire dialect hint.
#[must_use]
pub(crate) fn read_session_start_context() -> (SessionStartInput, Option<WireDialectHint>) {
    read_stdin_context("SessionStart", |d, raw| d.parse_session_start(raw))
}

/// Emits a `SessionStartResponse` to `stdout` in the resolved harness dialect.
pub(crate) fn write_session_start_for_context(
    resp: &SessionStartResponse,
    root: &std::path::Path,
    hint: Option<WireDialectHint>,
) {
    let dialect = resolve_harness_for_context(root, hint).hook_dialect();
    if let Ok(json) = dialect.format_session_start(resp) {
        println!("{json}");
    }
}
