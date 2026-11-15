# Declarative MCP Daemon (`tkg-mcp`)

`tkg-mcp` is a persistent JSON-RPC 2.0 stdio server providing harness-agnostic, language-agnostic declarative tool execution for any MCP-compatible agent client (Google Antigravity, Claude Code, OpenCode, Cursor, or custom runners).

## Native Action Handlers (`ToolAction`)

Instead of shelling out to untrusted scripts, `tkg-mcp` compiles 14 bounded native action handlers:

| Action | Description | Guards Enforced |
| :--- | :--- | :--- |
| `file_read` | Bounded file reader with line ranges and UTF-8 truncation | Path traversal, directory rejection |
| `file_write` | Atomic file creation and overwriting | Path policies, dangerous files, manifest guard |
| `mpatch_edit` | Precision search-and-replace editor | Fuzzy block validation, scoped paths, manifest guard |
| `file_search` | Scoped in-process regex search (`ignore` + `grep-searcher`) | Workspace confinement, dangerous files |
| `static_command` | Pre-configured command with zero dynamic arguments | Bubblewrap jail / process-group timeout |
| `parameterized_command` | Command with strictly validated typed/regex/enum arguments | Shell metacharacter blocking, sandbox profile |
| `cli_manual` | Ground-truth dynamic tool parameter inspector | In-memory schema reflection |
| `project_state` | Summarizes git/jj status, specs, and handoff state | Read-only |
| `workspace_ensure` | Provisions isolated Jujutsu subagent workspace | Jujutsu repository boundary |
| `workspace_inspect` | Inspects diffs inside a subagent sparse workspace | Read-only |
| `workspace_merge` | Merges validated subagent workspace changes back into trunk | Declarative quality gates + rollback |
| `set_active_agent` | Switches active agent persona in `.tkg/state/active_agent.json` | Agent perimeter scoping |
| `record_session_handoff` | Writes structured session handoff metadata | Workspace state boundary |
| `detect_wip` | Reports dirty working copy status | Read-only |

## MCP Resources & Prompts

Alongside `tools/list` and `tools/call`, `tkg-mcp` natively exposes MCP Resources and Prompts:

- **`resources/list` & `resources/read`:**
  - `tkg://rules`: Active agent directives, permitted directories, manifest protections, and content filters.
  - `tkg://status`: Live telemetry JSON including `detected_harness`, `in_band_seatbelt`, active circuit-breaker state, and workspace status.
- **`prompts/list` & `prompts/get`:**
  - `tkg-refactor`: Guided refactoring workflow referencing `tkg://rules` and caller inspection.
  - `tkg-audit`: Verification and state inspection workflow referencing `tkg://status` and `tkg://rules`.

## Caller Identification, Universal In-Band Seatbelt & Output Filtering

- **Dynamic Caller Identification:** `tkg-mcp` identifies the calling harness from MCP `initialize.params.clientInfo.name`, `TKG_HARNESS`, or Linux `/proc/<ppid>/cmdline` fallback.
- **Automatic In-Band Seatbelt:** When `Harness::supports_hooks()` is `false` (for example Cursor, non-hook MCP clients, or platforms without hook wiring), `tkg-mcp` enables `in_band_seatbelt = true`—tracking compiler/test failure signatures automatically across Rust, Python, and Go commands and halting mutating calls once the circuit breaker trips.
- **Declarative Output Filter (`[tools.<name>.output_filter]`):** Applies `keep_regex`, `drop_regex`, and `max_lines` before line/byte truncation to prune noisy compiler or runner output before it consumes context tokens.

## Dynamic Agent Scoping & Tracing

- `tkg-mcp` discovers the active agent via `--agent`, harness environment variables, or `.tkg/state/active_agent` and evaluates calls against `mcp_tools:` frontmatter. Unauthorized tool calls are rejected with an explicit policy error.
- Startup and request diagnostics go cleanly to `stderr` via `tracing` (`-v` / `-vv` / `-vvv` or `RUST_LOG`), keeping `stdout` strictly valid JSON-RPC 2.0.


