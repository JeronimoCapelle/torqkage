# Lifecycle Interceptors (`tkg-hooks`)

`tkg-hooks` is a featherweight native lifecycle interceptor compiled intentionally without `tokio`, `rusqlite`, `ratatui`, or `tracing-subscriber` so hook invocations stay compact and fast on `PreToolUse`, `PreInvocation`, `SessionStart`, and `Stop`.

## Harness Dialect Auto-Detection

`tkg-hooks` supports multiple harness wire protocols transparently via `tkg_common::hook_dialect`:
- **ProtoJSON (`agy`, `opencode`, custom):** Standard TorqKage / Antigravity JSON hook envelopes.
- **Claude Code (`claude`):** Auto-detects Claude payloads (`hook_event_name` / `tool_name`), normalizes `session_id` and `transcript_path`, and emits Claude `hookEventName` / `permissionDecision` / `permissionDecisionReason` outputs (`exit 2` on Deny, `exit 0` on Ask).
- Structured diagnostics go to `stderr` via `-v` / `-vv` / `-vvv` or `RUST_LOG`, leaving `stdout` strictly clean for hook wire payloads.

## Intercepted Events

1. **`inject-wake` (`SessionStart` Turn 1 Wake Injection):**  
   Pre-injects prior session handoff summaries, active Jujutsu/Git status, uncommitted diffs, and project documentation paths directly into the initial model prompt without burning an explicit tool call.
2. **`detect-wip` (`PreInvocation` Change Detection):**  
   Emits ephemeral turn-by-turn notification of working tree modifications; burns zero tokens when clean.
3. **`pre-tool-use` (`PreToolUse` Guard Pipeline):**  
   Executes the 6-stage pipeline (`circuit_breaker`, `manifest_guard`, `path_policies`, `subagent_admission`, `vcs_conflicts`, `tool_policy`) to return `allow`, `deny`, or `ask` before any tool runs.
4. **`record-handoff` (`Stop` Session Termination):**  
   Captures session metadata, modified files, and circuit breaker status to `.tkg/state/handoff.json` and `.tkg/state/last_handoff.md` upon session exit.

## 2-Strike Compiler Circuit Breaker

When an autonomous agent introduces compilation errors, `tkg-hooks` tracks consecutive build failures:
* **Strike 1:** Returns structured compiler diagnostics and warns the agent to focus on fixing compiler errors.
* **Strike 2 (Tripped):** Halts automatic tool execution and mandates human intervention or model escalation to prevent loops and token waste.
* **Reset:** The circuit breaker can be reset from `tkg` with <kbd>c</kbd> or <kbd>Shift+K</kbd>.
