# ADR-0002: Fail-Closed Process-Group Supervision for MCP Subprocesses

- **Status:** Accepted
- **Date:** 2026-11-15

## Context
Wrapping arbitrary compiler, test-runner, and build CLIs behind an asynchronous stdio JSON-RPC MCP server (`tkg-mcp`) introduces two classic failure modes:
1. A timed-out or canceled tool leaves background descendant processes holding `stdout`/`stderr` pipes open, deadlocking `wait_with_output()` and hanging the MCP server until the host agent times out.
2. Killing only the direct child PID orphans grandchild processes (e.g., `sh -c`, test workers, dev servers).

## Decision
Isolate and supervise every spawned command through [`SubprocessSupervisor`](../../crates/tkg-common/src/supervisor.rs) (`tkg-common/src/supervisor.rs`):
1. Spawn commands inside a dedicated OS process group via `command_group` (`setpgid`), wrapped in [`ChildGuard`](../../crates/tkg-common/src/supervisor.rs#L17-L50) whose `Drop` terminates lingering process groups automatically.
2. Drain `stdout`/`stderr` using non-blocking file descriptors (`O_NONBLOCK` / polling on Unix) while tracking the timeout deadline, preventing pipe buffer deadlocks.
3. Terminate timed-out groups in two stages: send `SIGTERM` to the entire process group with a 500 ms grace period (`SIGTERM_GRACE_WAIT`), then escalate to `SIGKILL` (`PROCESS_KILL_GRACE_WAIT`).

## Consequences
- Timed-out tools reliably return bounded diagnostics over MCP instead of wedging the stdio transport.
- Background grandchild processes spawned during compilation/tests are reaped alongside their parent group.
- Requires OS-specific timeout paths (`#[cfg(unix)]` `O_NONBLOCK` + process-group signaling vs fallback kill/wait on non-Unix targets).
