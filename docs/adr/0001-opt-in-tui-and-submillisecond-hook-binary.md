# ADR-0001: Separate Sub-Millisecond Hook Binary & Opt-In `--features tui`

- **Status:** Accepted
- **Date:** 2026-11-15

## Context
Lifecycle hooks (`PreToolUse`, `BeforeTool`, `PostToolUse`) run synchronously inside coding-agent tool loops (Claude Code, Codex CLI, Gemini CLI, Cursor, Copilot) on **every single tool call**. Any heavyweight dependency initialization (Tokio runtime, Ratatui/Crossterm terminal backends, bundled SQLite) adds tens of milliseconds of latency and megabytes of binary/memory footprint on every hook invocation.

At the same time, TorqKage evolved an interactive Ratatui control deck inside the CLI binary (`torqkage`), which previously shipped unconditionally via default Cargo features and launched on bare `tkg`.

## Decision
1. **Isolate runtime roles cleanly across crates:**
   - [`crates/tkg-hooks`](../../crates/tkg-hooks/Cargo.toml) depends only on `tkg-common` (default features), `serde`, `serde_json`, `toml`, and `tracing` — **no Tokio, no Ratatui, no Crossterm, no SQLite**.
   - [`crates/tkg-mcp`](../../crates/tkg-mcp/Cargo.toml) owns the async Tokio JSON-RPC server.
   - [`crates/tkg-ctrl`](../../crates/tkg-ctrl/Cargo.toml) (`tkg`) owns headless setup/diagnostics (`init`, `doctor`, `add`, `uninstall`) by default.
2. **Gate the TUI behind an opt-in feature and subcommand:**
   - Set `default = []` and `tui = ["dep:ratatui", "dep:crossterm"]` in `crates/tkg-ctrl/Cargo.toml`.
   - Bare `tkg` prints CLI usage; `tkg ui` launches the dashboard only when compiled with `--features tui`.
   - Enforce all feature permutations continuously in CI using `cargo hack clippy --workspace --all-targets --feature-powerset -- -D warnings`.

## Consequences
- `tkg-hooks --help` cold start measures **~515 µs mean** (`3.5 MiB` peak RSS) via `hyperfine`.
- Headless installs (`cargo binstall torqkage`, `cargo install torqkage`) stay small and avoid pulling terminal UI crates for ~93% non-TUI users.
- The TUI remains cleanly isolated behind `#[cfg(feature = "tui")]` (`~7%` of workspace lines) for operators who want it.
