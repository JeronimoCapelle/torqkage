# Introduction

Welcome to **The `TorqKage` Engine Manual**.

> ### Sandboxed Meta-MCP Runtime and Declarative Fleet Engine
> Turn any CLI binary, compiler, or script into an isolated, schema-validated MCP tool with declarative TOML configurations.

`TorqKage` is a declarative governance engine, Meta-MCP server, featherweight lifecycle runner, and Ratatui 0.30 interactive flight deck designed as a high-assurance extension layer for popular agent execution harnesses (Claude Code, OpenCode, Cursor, Google Antigravity, and custom runners), with support for more common harnesses in active development.

```
                    ┌──────────────────────────────────────────────┐
                    │          Agent Execution Harness             │
                    │   (Claude Code, OpenCode, Cursor, agy)       │
                    └──────────────────────┬───────────────────────┘
                                           │
                        ┌──────────────────┴──────────────────┐
                        │              TorqKage              │
                        │    (Pure Safe Rust 2024 Workspace)  │
                        └─┬──────────────┬──────────────┬───┬─┘
                          │              │              │   │
        ┌─────────────────┴─┐  ┌─────────┴────────┐  ┌──┴───┴──────────┐  ┌──────────────────┐
        │     tkg      │  │     tkg-mcp      │  │    tkg-hooks    │  │    tkg-common    │
        │ Ratatui TUI Deck  │  │ JSON-RPC Daemon  │  │ Lifecycle Hooks │  │ Shared Primitives│
        └───────────────────┘  └──────────────────┘  └─────────────────┘  └──────────────────┘
```

## Why `TorqKage`?

Autonomous AI coding agents frequently suffer from two fatal failure modes in production systems engineering:
1. **Ambient Shell Poisoning:** Agents run unstructured `bash` commands with full host access, risking accidental `rm -rf`, network leaks, or credential corruption.
2. **Token Exhaustion:** Re-injecting multi-kilobyte interpreted Markdown skills on every turn burns tokens and slows generation.

`TorqKage` replaces ambient, unstructured execution with **deterministic capability perimeters**:
- **Pure Safe Rust:** The entire workspace enforces `#![forbid(unsafe_code)]`.
- **Two-Layer Cross-Platform Containment:**
  - **Linux (`bwrap`):** Ephemeral Bubblewrap container namespaces (`--unshare-all`, private loopback, read-only system roots, credential masking, and granular `[tools.<name>.sandbox]` profiles) toggleable per-session in `tkg` or per-tool (`enforce_sandbox = true`).
  - **macOS, Windows & Non-Hook Harnesses:** Automatic **Universal In-Band MCP Seatbelt** providing workspace confinement, regex argument validation, dangerous-file protection, and automatic in-band circuit breaking even without external lifecycle hooks.
- **4-Tier Cascaded Config & Unified Harness Layer:** Deep-merged TOML configuration (`defaults` < `user global` < `project local` < `CLI/env`) and a capability-driven `Harness` trait spanning Antigravity (`agy`), Claude Code, OpenCode, Cursor, and custom CLI runners.
- **MCP Resources, Prompts & Diagnostics:** Exposes `tkg://rules` and `tkg://status` resources, `tkg-refactor` and `tkg-audit` prompts, declarative `output_filter` pruning, and `-v`/`-vv`/`-vvv` `tracing` logs on `stderr`.
- **Featherweight Native Tooling:** Focused separate binaries (`tkg`, `tkg-mcp`, and lean `tkg-hooks` without `tokio`/`sqlite`/`ratatui`) for fast startup and small footprint.
- **Offline Human Cockpit:** Single-pane Ratatui 0.30 dashboard (`tkg`) for toggling permissions, inspecting tools, resetting breakers, and resuming previous sessions.
