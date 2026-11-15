# Multi-Source Session Resumption

`TorqKage` solves conversational state loss across terminal restarts by delegating session discovery to the unified [`Harness`](../../ARCHITECTURE.md#81-the-4-crates--unified-harness-architecture) capability trait in `tkg-common`.

## Unified `Harness::discover_sessions` Architecture

Depending on `[runner] harness` (or auto-detected workspace markers `.claude`, `.opencode`, `.gemini`, `.cursor/mcp.json`), `tkg` calls `harness.discover_sessions(agent_name, workspace_root)` against the matching backend:

1. **Google Antigravity (`harness = "agy"`):**
   - Queries `~/.gemini/antigravity-cli/conversation_summaries.db` via optional `rusqlite` (`sqlite` feature enabled on `tkg-common` when built for `tkg-ctrl`, omitted in lean `tkg-hooks`).
   - Correlates CLI history in `~/.gemini/antigravity-cli/history.jsonl` matching workspace path.
   - Verifies local database files in `~/.gemini/antigravity-cli/conversations/<id>.db`.

2. **Claude Code (`harness = "claude"`):**
   - Encodes the canonical workspace path into Claude's cross-platform project slug (normalizing path separators, `:`, `?`, and `\\?\` Windows verbatim prefixes).
   - Scans JSONL transcripts in `~/.claude/projects/<encoded-slug>/*.jsonl` alongside fallback records in `.tkg/state/sessions/*.json`.

3. **OpenCode (`harness = "opencode"`):**
   - Discovers workspace sessions in `.opencode/sessions/*.json` and `.opencode/*.json`.
   - Checks global user traces in `~/.local/share/opencode/sessions` and `~/.opencode/sessions`.
   - Extracts session ID, agent filter, message turn counts, and preview descriptions.

4. **Cursor & Custom Harnesses (`harness = "cursor"` / `"custom"`):**
   - Discovers Cursor transcripts (when present) and universal state records in `.tkg/state/sessions/*.json`.

## User Interface

In `tkg`, when a prior session with recorded steps is found:
```text
Resumable (conv: f1bb6355, 50 steps)
[Enter/r: Resume]  [n: New Session]
```
Pressing <kbd>Enter</kbd> or <kbd>r</kbd> invokes the harness's configured resumption flag (for example `agy --conversation <id>`, `opencode --session <id>`, or `claude --resume <id>`), resuming immediately with full conversation history and context.
