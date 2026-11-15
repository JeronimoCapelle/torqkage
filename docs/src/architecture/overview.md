# System Architecture

`TorqKage` is structured as a unified 4-crate Rust 2024 workspace:

```text
crates/
├── tkg-common/     # Core types, unified Harness trait, 4-tier cascade loader, supervisor, bwrap jail, diagnostics
├── tkg-mcp/        # Persistent JSON-RPC 2.0 stdio MCP daemon (tools, resources, prompts, in-band seatbelt)
├── tkg-hooks/      # Featherweight lifecycle hook CLI router (no Tokio, SQLite, or Ratatui dependencies)
└── tkg-ctrl/       # Interactive Ratatui 0.30 terminal control deck & launcher (binary: tkg)
```

## Unified Harness Capability Seam (`tkg_common::harness`)

All harness-specific behavior—CLI flags, directory layout, capability flags (`NATIVE_HOOKS`, `SESSION_RESUME`, `HEADLESS_SPAWN`, `NATIVE_MCP`), wire dialects (`HookDialect`), session discovery (`tkg_common::sessions`), spawn command assembly, and config injection (`tkg_common::harness_install`)—is consolidated behind the [`Harness`](../../ARCHITECTURE.md#81-the-4-crates--unified-harness-architecture) trait:

- **Antigravity (`agy`)**: SQLite + `history.jsonl` session lookup, `ProtoJSON` hooks, `.gemini/` bundle install.
- **Claude Code (`claude`)**: JSONL session discovery under `~/.claude/projects/`, Claude `PreToolUse` / `SessionStart` / `Stop` dialect, `.mcp.json` + `.claude/settings.json` atomic merge.
- **OpenCode (`opencode`)**: JSON session lookup, `ProtoJSON` dialect, `~/.local/share/opencode/plugins/torqkage`.
- **Cursor (`cursor`)**: Transcript discovery, `.cursor/mcp.json` merge; relies on Universal In-Band MCP Seatbelt (`NATIVE_HOOKS` unset).
- **Custom harnesses**: Convention-based `.tkg` / `.<slug>` paths and generic `.tkg/state/sessions/*.json` state tracking.

## The Three Declarative Contracts & 4-Tier Cascaded Config

The suite separates runtime plumbing from repository rules across three TOML contracts:

1. **`tools.toml` (MCP Perimeter):**  
   Declares all MCP tools, template inheritance, argument bounds, `[tools.<name>.sandbox]` profiles, `[tools.<name>.output_filter]` pruning, and permission gates (`require_permission_gate`).
2. **`project.toml` / `config.toml` (Project Perimeter):**  
   Declares quality gates (`[gates.<name>]`), workspace containers, sandbox bind mounts, documentation directories, security allow/deny rules, and `[runner]` / `default_harness`.
3. **`hooks.toml` (Lifecycle Perimeter):**  
   Defines automated interception triggers (Turn-1 wake context injections, pre-invocation WIP notices, `PreToolUse` guard pipeline, and compiler circuit breaker rules).

`CascadedConfigLoader` (`tkg_common::cascade`) layers configuration across four tiers (lowest to highest priority):
1. Hardcoded defaults & repository ecosystem auto-inference
2. User-global config (`~/.config/tkg/config.toml`, `~/.config/tkg/project.toml`, `~/.tkg/project.toml`, and global plugin bundle configs)
3. Workspace-local config (`plugins/torqkage/*.toml`, `<ws>/*.toml`, `<ws>/.tkg/*.toml`)
4. Runtime environment variables & CLI overrides (with optional `merge_strategy = "replace" | "append" | "prepend" | "unique"`)

Malformed TOML in any discovered tier fails closed with `line:col` caret diagnostics.

## Structured Diagnostics & Safe Rust Invariant

- **Pure Safe Rust:** Every crate in `TorqKage` enforces `#![forbid(unsafe_code)]` and `#![deny(clippy::unwrap_used, ...)]` at the workspace lint level.
- **Structured `tracing` to `stderr`:** `tkg`, `tkg-mcp`, and `tkg-hooks` configure `tracing` on `stderr` via `-v`, `-vv`, `-vvv`, or `RUST_LOG` so `stdout` wire protocols (`JSON-RPC` and `ProtoJSON`) and TUI screens remain uncorrupted. `tkg-hooks` uses a zero-`tracing-subscriber` lightweight subscriber by default to preserve fast startup and minimal binary size.
