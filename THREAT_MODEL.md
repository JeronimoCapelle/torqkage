# TorqKage Threat Model & Trust Boundaries

TorqKage acts as a **fail-closed guardrail and runtime containment layer** (`tkg-hooks` + `tkg-mcp`) placed between an AI coding agent harness (Claude Code, Codex CLI, Gemini CLI, Cursor, Copilot, OpenCode, etc.) and the developer's repository/toolchain.

This document defines what TorqKage protects, how each trust boundary fails closed, and where platform or threat boundaries explicitly stop.

---

## 1. Protected Assets & Adversary Model

### Assets
- **Host Developer Workstation & Build Pipeline Integrity:** Preventing unauthorized modification of build/package manifests (`Cargo.toml`, `build.rs`, `package.json`, lockfiles, `CMakeLists.txt`, `Makefile`, etc.) that execute arbitrary code during subsequent IDE indexing, `cargo check`, or `npm install`.
- **Workspace Filesystem Boundaries:** Keeping agent reads/writes/patches inside declared repository directories and denying path traversal (`..`), symlink escapes, and sensitive locations (`secrets/`, `.env`).
- **Context Window & Developer Feedback Loops:** Halting runaway failure/retry thrashing (2-strike circuit breaker) and pruning noisy compiler/test output before it saturates agent context.
- **Process Tree & Stdio Integrity:** Preventing orphaned background child processes, shell deadlocks, or ANSI/stdio corruption when running wrapped CLI tools over MCP JSON-RPC.

### Threats Considered
1. **Prompt-injected or confused-deputy agent behavior** issuing destructive commands, escaping workspace boundaries, or editing build scripts (`build.rs`, `Cargo.toml`, `package.json`) to trigger code execution outside MCP.
2. **Comment/string-literal policy evasion** (e.g., hiding forbidden identifiers in comments or string literals to bypass naive regex scanners).
3. **Corrupt or malformed policy configurations** (`tools.toml`, `hooks.toml`, `project.toml`) or malformed harness hook payloads (`stdin`).
4. **Runaway / hanging child processes** spawned by MCP actions exhausting file descriptors, timing out silently, or leaving descendant processes alive after parent cancellation.

---

## 2. Trust Boundaries & Enforced Controls

| Boundary | Mechanism | Fail-Closed Behavior | Implementation |
| :--- | :--- | :--- | :--- |
| **Harness Hook Gate (`tkg-hooks`)** | Intercepts `PreToolUse` / `BeforeTool` / `PostToolUse` JSON on `stdin` via dialect adapters | Corrupt JSON or policy parse failures deny/block rather than silently allowing | `crates/tkg-hooks/`, `crates/tkg-common/src/hook_dialect.rs` |
| **MCP Server (`tkg-mcp`)** | Stdio JSON-RPC 2.0 daemon (`#![forbid(unsafe_code)]`) exposing schema-validated CLI actions + In-Band Seatbelt | Corrupt `tools.toml` aborts startup with line diagnostics; unpermitted tools fail closed | `crates/tkg-mcp/src/server.rs`, `guards.rs` |
| **Build Manifest Protection** | `DEFAULT_PROTECTED_MANIFESTS` guard checks writes against `AgentGuard::allowed_files` | Blocks manifest mutation unless explicitly permitted by agent guard allowlist | `crates/tkg-common/src/manifest_guard.rs` |
| **Workspace Path Guard** | Lexical + canonical workspace containment checks + glob/directory policies | Rejects `..` traversal, out-of-workspace paths, and disallowed directories | `crates/tkg-mcp/src/guards.rs`, `crates/tkg-common/src/content_filter.rs` |
| **AST / Comment Filter** | Language-aware comment & string stripping before content/test inspection | Neutralizes comment-smuggled markers and ignores false-positive test/comment noise | `crates/tkg-common/src/content_filter.rs` |
| **OS Sandbox (`bwrap`)** | Linux Bubblewrap namespaces + explicit bind mounts (`enforce_sandbox = true`) | Fails closed when `bwrap` is unavailable unless explicit container/host fallback applies | `crates/tkg-common/src/sandbox.rs` |
| **Process Supervision** | `command_group` (`setpgid`) + non-blocking pipe drain + staged `SIGTERM` → `SIGKILL` | Kills the full descendant process group on timeout or drop (`ChildGuard`) | `crates/tkg-common/src/supervisor.rs` |
| **2-Strike Circuit Breaker** | Persistent `.tkg/state/circuit_breaker.json` + in-band MCP strike tracking | Trips after `DEFAULT_MAX_STRIKES = 2` failures and halts mutating actions until reset | `crates/tkg-common/src/models.rs` |

---

## 3. Known Platform Limitations & Explicit Non-Goals

Honest threat modeling requires naming what TorqKage **does not** defend against:

1. **OS Kernel Namespaces Are Linux-Only (`bwrap`):**
   - Full OS-level filesystem/mount/PID/network sandboxing (`enforce_sandbox = true`) relies on **Linux Bubblewrap (`bwrap`)**.
   - On **macOS and Windows** (or harnesses lacking native `PreToolUse` hooks), enforcement relies on the application-layer **In-Band MCP Seatbelt**, `ManifestGuard`, and path canonicalization rather than kernel-enforced syscall confinement. Commands executed through a harness's *own* unhooked native shell tool on non-Linux hosts bypass `bwrap`.
2. **Harnesses Without `PreToolUse` Hooks (`NATIVE_HOOKS = false`):**
   - Harnesses such as OpenCode, VS Code Copilot Chat, Windsurf, Roo Code, and Cline receive protection when calling tools through **`tkg-mcp`**, but TorqKage cannot intercept direct native shell invocations inside those clients unless the host client exposes pre-tool hooks.
3. **Nested Containers (`is_inside_container`):**
   - Inside unprivileged containers (Docker/Podman/CI) where user namespaces are restricted, `bwrap` nesting is often disallowed by the kernel (`kernel.apparmor_restrict_unprivileged_userns`). TorqKage explicitly detects outer containers via `/proc` and environment markers so callers do not fail unexpectedly inside an already-sandboxed container.
4. **Compromised Host OS Account / Untrusted Local Config Files:**
   - TorqKage protects a developer workspace from an untrusted or error-prone *AI coding agent*, not against malware already running as the same OS user with write access to `~/.cargo/bin`, `.tkg/`, or `.claude/settings.json`.
5. **Transitive C FFI Surface:**
   - All 4 workspace crates compile with `#![forbid(unsafe_code)]`, clippy `as_conversions = "deny"`, and `cargo-deny`. However, optional `sqlite` session discovery links bundled SQLite (`rusqlite` / `libsqlite3-sys`), and Unix process-group signal helpers depend on `nix`/`libc`.

---

## 4. Verification & Assurance Evidence

- **Static & Supply-Chain Gates:** `cargo clippy -- -D warnings`, `cargo hack clippy --feature-powerset`, `cargo deny check`, `cargo machete`, `taplo fmt --check`, `typos`.
- **Dynamic, Property & Fuzz Coverage:** `proptest` generative strategy tests, `insta` snapshot tests, `fuzz/fuzz_targets/fuzz_parsers.rs` + `fuzz_content_filter.rs` (`cargo-fuzz`), and `cargo-mutants` ([`.cargo/mutants.toml`](.cargo/mutants.toml)) targeting security guards and parsers.
- **Native Sanitizers & Coverage:** Scheduled weekly Nightly `ASan`, `cargo-careful`, and `cargo-geiger` ([`.github/workflows/weekly.yml`](.github/workflows/weekly.yml)), plus per-PR `cargo-llvm-cov` reporting ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)).
