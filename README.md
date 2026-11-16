# TorqKage

[![CI](https://github.com/JeronimoCapelle/torqkage/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/JeronimoCapelle/torqkage/actions/workflows/ci.yml)
[![Rust Edition](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Documentation](https://img.shields.io/badge/docs-manual-blue.svg)](https://jeronimocapelle.github.io/torqkage/)
[![MCP Protocol](https://img.shields.io/badge/MCP-2024--11--05-green.svg)](https://modelcontextprotocol.io/)
[![Ratatui](https://img.shields.io/badge/Ratatui-0.30-purple.svg)](https://ratatui.rs/)

### Drop-In Security Guardrails & Sandboxed CLI-to-MCP Runtime for Coding Agents
Wrap any CLI tool into a sandboxed MCP action and add sub-millisecond `PreToolUse` guardrails to **Claude Code**, **OpenAI Codex CLI**, **Gemini CLI**, **Cursor**, **GitHub Copilot**, **VS Code**, **OpenCode**, **Windsurf**, **Roo Code**, **Cline**, or **Google Antigravity** — in one command, with zero workflow rewrite and no TUI required.

Built in Pure Safe Rust (`#![forbid(unsafe_code)]`) with ~0.71 ms `tkg-hooks` cold startup, fail-closed Linux Bubblewrap containerization, and a cross-platform In-Band MCP Seatbelt.

---

## 30-Second Quickstart

### 1. Install Binaries
Install precompiled binaries (`tkg`, `tkg-mcp`, `tkg-hooks`) with `.sha256` verification:

```sh
curl -sSL https://raw.githubusercontent.com/JeronimoCapelle/torqkage/main/install.sh | sh
```

Or install prebuilt binaries via `cargo-binstall` / from source on crates.io:

```bash
cargo binstall torqkage tkg-mcp tkg-hooks   # Fast prebuilt install (no compilation)
cargo install torqkage tkg-mcp tkg-hooks    # Or compile from source
```

### 2. Inject Into Your Existing Agent (`tkg init`)
From any repository, auto-detect your harness or name it explicitly:

```bash
tkg init                    # Auto-detect (.claude, .codex, .gemini, .cursor, .vscode, ...)
tkg init --with-tools       # Also seed starter sandboxed MCP commands (Cargo/Node/Py/Go)
tkg init claude             # Or target explicitly: claude | codex | gemini | cursor | github-copilot | vscode | ...
tkg doctor                  # Verify hook registration, MCP config, and binaries
tkg doctor --demo           # Read-only 1-second smoke demo of live guardrails & circuit breaker
tkg uninstall               # Surgically revert TorqKage entries (--purge to also delete .tkg/)
```

`tkg init` writes only the native MCP + hook entries your harness already understands (`.claude/`, `.codex/`, `.gemini/`, `.cursor/`, `.vscode/`, `.github/hooks/`, `.opencode/`, `.roo/`, etc.) — it does **not** force extra project boilerplate unless needed.

### 3. Wrap Any CLI Tool Into Sandboxed MCP (`tkg add`)
Expose any compiler, test runner, linter, or script to your agent with OS sandboxing and a 2-strike failure circuit breaker:

```bash
tkg add rust_tests --sandbox --circuit-breaker -d "Run workspace unit tests" -- cargo test --workspace
```

This adds a clean `[tools.rust_tests]` entry in `.tkg/tools.toml` while preserving comments and existing settings.

---

## Why Drop TorqKage In Front of Your Agent?

| Agent Pain | How `tkg-hooks` + `tkg-mcp` Solve It Immediately |
| :--- | :--- |
| **Failing command loops** | **Two-strike circuit breaker:** Halts repeated failing mutating commands (compiler/test thrashing) until reviewed or reset (`tkg reset-breaker`). |
| **Dangerous shell & file edits** | **Two-layer containment:** `enforce_sandbox = true` runs commands in fail-closed Linux Bubblewrap namespaces; macOS, Windows, and non-hook clients get the **Universal In-Band MCP Seatbelt** plus protected manifest guards (`Cargo.toml`, `package.json`, lockfiles). |
| **MCP sprawl & token bloat** | **One lightweight Rust MCP daemon (`tkg-mcp`):** Turn any CLI binary into a schema-validated, output-filtered (`[tools.<name>.output_filter]`) MCP tool instead of maintaining separate Node/Python MCP servers. |
| **Multi-tool fragmentation** | **One configuration, 11 harnesses:** Same `.tkg/tools.toml` and guardrails work seamlessly whether your team uses Claude Code, Codex CLI, Gemini CLI, Cursor, Copilot, or VS Code. |

---

## Supported Agent Harnesses

| Harness | ID (`tkg init <id>`) | Native Hooks | Native MCP | Session Resume |
| :--- | :--- | :--- | :--- | :--- |
| **Claude Code** | `claude` | Yes (`.claude/settings.json`) | Yes (`.mcp.json`) | Yes |
| **OpenAI Codex CLI** | `codex` | Yes (`.codex/hooks.json`) | Yes (`.codex/config.toml`) | Yes |
| **Gemini CLI** | `gemini` | Yes (`.gemini/settings.json`) | Yes (`.gemini/settings.json`) | Yes |
| **Cursor** | `cursor` | Yes (`.cursor/hooks.json`) | Yes (`.cursor/mcp.json`) | Yes |
| **GitHub Copilot** | `github-copilot` | Yes (`.github/hooks/torqkage.json`) | Yes (`.vscode/mcp.json`) | — |
| **OpenCode** | `opencode` | — *(In-Band MCP Seatbelt)* | Yes (`opencode.json`) | Yes |
| **VS Code (Copilot Chat)** | `vscode` | — *(In-Band MCP Seatbelt)* | Yes (`.vscode/mcp.json`) | — |
| **Windsurf Cascade** | `windsurf` | — *(In-Band MCP Seatbelt)* | Yes (`~/.codeium/windsurf/mcp_config.json`) | — |
| **Roo Code** | `roo` | — *(In-Band MCP Seatbelt)* | Yes (`.roo/mcp.json`) | — |
| **Cline** | `cline` | — *(In-Band MCP Seatbelt)* | Yes *(VS Code globalStorage)* | — |
| **Google Antigravity** | `agy` | — *(In-Band MCP Seatbelt)* | Yes (`.agents/` + MCP config) | Yes |

List all harnesses from CLI anytime with `tkg init --list`.

---

## Optional Power Features (Use Only When Needed)

Everything below sits cleanly behind opt-in commands — none of it is required to use `tkg init`, `tkg add`, `tkg-hooks`, or `tkg-mcp`.

### 1. Interactive Terminal Control Deck (Optional `--features tui`)
Prefer a visual dashboard to toggle sandboxing, inspect tool schemas, approve permission gates, or resume harness sessions? Build or install with the `tui` feature (`cargo install torqkage --features tui`) and launch explicitly with `tkg ui`:

<p align="center">
  <img src="assets/demo.gif" alt="TorqKage Interactive Terminal Control Deck Demo" width="100%" />
</p>

| Keybinding | Action |
| :--- | :--- |
| <kbd>Enter</kbd> / <kbd>r</kbd> | **Resume session / Launch agent** in the configured harness |
| <kbd>b</kbd> / <kbd>F8</kbd> | **Toggle Bubblewrap sandbox** on the selected tool |
| <kbd>w</kbd> | **Toggle workspace isolation** (`.tkg/workspaces/<agent>` via Jujutsu) |
| <kbd>c</kbd> / <kbd>Shift-K</kbd> | **Reset circuit breaker** counts in `.tkg/state/` |
| <kbd>F2</kbd> | **Validate schemas & run pre-flight checks** |
| <kbd>Tab</kbd> / <kbd>?</kbd> | Switch tabs / open the embedded manual |

### 2. Jujutsu Workspace Isolation & Agent Launcher
Isolate an agent run inside a sparse Jujutsu working copy so edits never dirty your primary working tree:

```bash
tkg launch coder --sandbox --isolated
tkg workspace ensure coder
```

### 3. Advanced Declarative `tools.toml` Schemas & Starter Kits
Beyond `tkg add`, `.tkg/tools.toml` supports regex-validated parameters, mutual-exclusion enums, output filtering, and per-tool Bubblewrap bind mounts (deep-merged across `defaults` < `~/.config/tkg/` < `plugins/torqkage/` & `.tkg/` < CLI/env):

```toml
[tools.my_scanner]
inherit = "base_tools.cargo_base"
description = "Executes security scan with regex-validated arguments"
action = "parameterized_command"
command = ["cargo", "audit"]
enforce_sandbox = true
mutating = false
args = [
    { name = "target", required = false, flag = "--target", regex = "^[a-zA-Z0-9_-]+$", description = "Crate target name" },
    { name = "mode", required = false, default = "advisories", enum = ["advisories", "bans", "licenses"], description = "Audit check mode" }
]

[tools.my_scanner.sandbox]
engine = "bwrap"
allow_network = false
read_only_binds = ["{cwd}/Cargo.lock"]

[tools.my_scanner.output_filter]
keep_regex = ["error|warning|Vulnerability"]
max_lines = 80
```

Pre-configured ecosystem starter kits live in [`docs/templates/`](docs/templates/) (**[Go](https://jeronimocapelle.github.io/torqkage/templates/go-setup.html)**, **[Python](https://jeronimocapelle.github.io/torqkage/templates/python-setup.html)**, **[TypeScript](https://jeronimocapelle.github.io/torqkage/templates/typescript-setup.html)**, **[Rust](https://jeronimocapelle.github.io/torqkage/templates/rust-setup.html)**, **[QA Auditor](https://jeronimocapelle.github.io/torqkage/templates/qa-tester.html)**).

### 4. Optional 10-Agent Reference Fleet (`plugins/torqkage/agents/`)
For teams wanting strict Design Verification (DV) separation between implementers and test/spec reviewers, the repository ships an optional 10-agent reference fleet (`coder`, `test-author`, `doc-author`, `planner`, `copilot`, `finisher`, `spec-checker`, `perf-analyst`, `mini`, `sysadmin`) in [`plugins/torqkage/agents/`](plugins/torqkage/agents/).

### 5. Build From Source & Developer Workflows (`justfile`)
```bash
git clone https://github.com/JeronimoCapelle/torqkage.git
cd torqkage
./install.sh --build-from-source --perf

just fast          # Cranelift + Mold compile check across workspace
just ft            # Cranelift + Mold nextest test loop (`fast` profile)
just perf          # Fat-LTO + target-cpu=native release build
just fix           # Fixed-point auto-fix loop (rustc + clippy --fix <=5 passes + fmt + tests)
just verify        # Full CI gate: fmt, clippy -D warnings, nextest, doctest, deny, machete, typos, docs
```

---

## Benchmarks

Measured on **Linux `x86_64` (Intel Core i5-10300H @ 2.50 GHz)** against the workspace release builds and `tkg-common` benchmark suites ([`core_benches.rs`](crates/tkg-common/benches/core_benches.rs), [`gungraun_bench.rs`](crates/tkg-common/benches/gungraun_bench.rs)):

### CLI & Hook Interception Latency (`hyperfine --warmup 10`)

| Command | Workload | Mean ± σ | Min … Max | Peak RSS |
| :--- | :--- | :--- | :--- | :--- |
| `./target/release/tkg-hooks help` | Cold CLI spawn & dispatch (`-N`) | **709.3 ± 147.4 µs** | 574.5 … 2188.8 µs | 4.6 MiB |
| `./target/release/tkg --status` | Cascaded config load + 10-agent matrix (`-N`) | **4.49 ± 0.39 ms** | 4.07 … 7.87 ms | 8.0 MiB |
| `tkg-hooks pre-tool-use` | 6-stage `hooks.toml` gate pipeline + JSON dialect (via `sh`) | **12.16 ± 0.84 ms** | 11.27 … 15.75 ms | 7.2 MiB |
| `tkg-hooks detect-wip` | Turn-by-turn VCS diff check spawning `jj`/`git` (via `sh`) | **14.97 ± 5.49 ms** | 11.76 … 40.71 ms | 40.7 MiB |

### Core Hot-Path Microbenchmarks (`divan` + `gungraun` Callgrind)

| Benchmark (`tkg_common`) | Median Wall (`divan`) | Mean Wall (`divan`) | Allocs (`divan`) | Instructions (`callgrind`) |
| :--- | :--- | :--- | :--- | :--- |
| `strip_delimited` | **1.78 µs** | 1.96 µs | 3 / 2.15 KB | 24,541 |
| `strip_comments_rust` | **8.14 µs** | 8.58 µs | 11 / 1.91 KB | 104,791 |
| `resolve_full_tools_toml` | **9.68 µs** | 11.33 µs | 51 / 35.73 KB | — |
| `strip_tests_rust` | **12.14 µs** | 13.05 µs | 15 / 3.03 KB | 151,505 |
| `frontmatter_parse_and_serialize` | **21.99 µs** | 25.92 µs | 246 / 75.53 KB | 180,892 *(parse)* |
| `path_glob_and_security_guards` | **29.35 µs** | 32.93 µs | 228 / 20.89 KB | — |

### Stripped Binary Sizes (`--profile min-size`)

| Binary | `release` (unstripped) | `min-size` (stripped Fat LTO) | Architecture Scope |
| :--- | :--- | :--- | :--- |
| **`tkg`** | 7.7 MiB | **3.7 MiB** | Ratatui 0.30 TUI + SQLite session discovery |
| **`tkg-hooks`** | 8.1 MiB | **4.0 MiB** | Lean lifecycle CLI (no Tokio, SQLite, or Ratatui) |
| **`tkg-mcp`** | 12 MiB | **5.0 MiB** | Async Tokio stdio JSON-RPC 2.0 daemon |

Reproduce locally:

```bash
just bench                                          # Divan latency & allocation profiler
just bench-cold-start                               # Hyperfine release cold-start on tkg-hooks (~0.51 ms)
cargo bench -p tkg-common --bench gungraun_bench    # Callgrind instruction counts (Linux + Valgrind)
just cov                                            # Source-based line/region/function coverage (cargo-llvm-cov)
```

See the full breakdown in [Benchmarks & Latency Profiles](docs/src/benchmarks.md).

---

## Documentation

For architecture details, trust boundaries, and guides:

* **[Threat Model & Trust Boundaries (`THREAT_MODEL.md`)](THREAT_MODEL.md)**:
  * Protected assets, fail-closed invariants, and enforcement layers.
  * Honest platform limits (Linux `bwrap` vs. macOS/Windows In-Band MCP Seatbelt) and non-goals.
* **[Architecture Decision Records (`docs/adr/`)](docs/adr/)**:
  * [ADR-0001](docs/adr/0001-opt-in-tui-and-submillisecond-hook-binary.md): Sub-millisecond hook binary & opt-in `--features tui`.
  * [ADR-0002](docs/adr/0002-fail-closed-process-group-supervision.md): Fail-closed process-group supervision (`O_NONBLOCK` + staged `SIGTERM`/`SIGKILL`).
  * [ADR-0003](docs/adr/0003-cross-platform-inband-seatbelt-and-bwrap.md): Linux Bubblewrap vs. cross-platform In-Band MCP Seatbelt.
* **[Canonical System Architecture (`ARCHITECTURE.md`)](ARCHITECTURE.md)**:
  * Invariants and error handling guarantees.
  * Declarative contracts (`tools.toml`, `hooks.toml`, `project.toml`).
  * Process tree supervision and termination handling (`SubprocessSupervisor`).
  * Bubblewrap container sandboxing and loopback network model.
  * Design Verification (DV) methodology.
* **[The TorqKage Engine Manual](https://jeronimocapelle.github.io/torqkage/)**:
  * [Pre-Built Ecosystem Starter Kits](https://jeronimocapelle.github.io/torqkage/templates/overview.html)
  * Setup guides and workflow tutorials.
  * Verification flow and lifecycle hook details.
  * Performance characteristics and resource footprint.

---

## License

Dual-licensed under either of:

- **MIT License** ([`LICENSE-MIT`](LICENSE-MIT))
- **Apache License, Version 2.0** ([`LICENSE-APACHE`](LICENSE-APACHE))

at your option.
