# ADR-0003: Cross-Platform Defense via Linux Bubblewrap & In-Band MCP Seatbelt

- **Status:** Accepted
- **Date:** 2026-11-15

## Context
Developers run coding agents across Linux, macOS, and Windows, and across harnesses with vastly different extensibility surfaces:
- Some harnesses (Claude Code, Codex CLI, Gemini CLI, Cursor, GitHub Copilot) expose native `PreToolUse` / `BeforeTool` lifecycle hooks.
- Others (OpenCode, VS Code Copilot Chat, Windsurf, Roo Code, Cline, Antigravity) expose native MCP servers without `PreToolUse` hooks.
- Kernel mount/PID/network namespace confinement via Bubblewrap (`bwrap`) is Linux-only.

Making `bwrap` mandatory everywhere would block macOS/Windows developers; falling back silently to unconstrained execution would silently weaken guarantees on Linux.

## Decision
Layer defense-in-depth cleanly without misrepresenting OS guarantees (documented in [`THREAT_MODEL.md`](../../THREAT_MODEL.md)):
1. **Linux (`enforce_sandbox = true`):** Fail closed if `bwrap` is unavailable (`test_missing_bwrap_fails_closed_without_explicit_host_fallback`), unless inside a verified outer container with `allow_host_fallback = true`.
2. **All Platforms & Hookless MCP Clients (`tkg-mcp` In-Band Seatbelt):** Enforce strict argument regex/enum validation, workspace path canonicalization (`guards.rs`), `DEFAULT_PROTECTED_MANIFESTS` mutation checks (`manifest_guard.rs`), AST/comment-aware content normalization (`content_filter.rs`), and the 2-strike failure circuit breaker directly inside `tkg-mcp` and `tkg-hooks`.

## Consequences
- Every supported harness receives fail-closed MCP validation, manifest protection, and loop braking on Linux, macOS, and Windows — even with zero `.tkg/*.toml` config files.
- OS-level kernel namespace confinement remains Linux-only and is documented honestly as a platform boundary rather than implied to exist on macOS or Windows.
