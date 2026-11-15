# TorqKage

[![CI](https://github.com/JeronimoCapelle/torqkage/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/JeronimoCapelle/torqkage/actions/workflows/ci.yml)
[![Rust Edition](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org/)
[![License: AGPL-3.0-or-later](https://img.shields.io/badge/License-AGPL--3.0--or--later-blue.svg)](LICENSE)
[![Documentation](https://img.shields.io/badge/docs-manual-blue.svg)](https://jeronimocapelle.github.io/torqkage/)
[![MCP Protocol](https://img.shields.io/badge/MCP-2024--11--05-green.svg)](https://modelcontextprotocol.io/)
[![Ratatui](https://img.shields.io/badge/Ratatui-0.30-purple.svg)](https://ratatui.rs/)

### Sandboxed Meta-MCP Runtime and Declarative Fleet Engine
Turn any CLI binary, script, or compiler into a sandboxed, schema-validated MCP tool with declarative, 4-tier cascaded TOML configurations.

Built in Pure Safe Rust (`#![forbid(unsafe_code)]`) with Linux Bubblewrap container isolation, universal cross-platform In-Band MCP Seatbelt containment, sub-millisecond `tkg-hooks` CLI startup (~0.71 ms cold start), and an interactive Ratatui 0.30 terminal dashboard (`tkg`).

<p align="center">
  <img src="assets/demo.gif" alt="TorqKage Interactive Terminal Control Deck Demo" width="100%" />
</p>

---

## About TorqKage

Running autonomous AI coding agents with direct shell access carries real operational risks: destructive commands, credential exposure, unintended network access, dirty git worktrees, and high token costs from repeatedly injecting CLI documentation into model context.

`TorqKage` provides declarative containerization and governance for AI agent fleets. Written in Safe Rust (`#![forbid(unsafe_code)]`), it functions as a Meta-MCP server: any CLI binary, script, or compiler declared in TOML becomes an isolated, schema-validated MCP tool with Bubblewrap container sandboxing, sub-millisecond lifecycle hooks, and per-agent permission allowlists.

The repository includes a 10-agent reference fleet for systems engineering, pre-built starter configurations for Go, Python, TypeScript, and Rust, and an interactive terminal dashboard (`tkg`) for managing permissions, inspecting tools, and resuming sessions.

---

## Why TorqKage

Configuring tool access for autonomous coding agents usually involves an undesirable choice:

1. **MCP sprawl:** Writing and running separate Node.js or Python MCP servers for every linter, compiler, and CLI tool, which increases resource usage and maintenance overhead.
2. **Ambient shell access:** Giving agents direct terminal access, which risks unintended filesystem changes, secret exposure, and burns context tokens repeatedly re-explaining CLI usage.

`TorqKage` bridges this gap. It is a declarative Meta-MCP server and agent engine built for popular agentic execution harnesses—including Claude Code, OpenCode, Cursor, and Google Antigravity (`agy`), with broader support for additional common harnesses in active development. Tools declared in `tools.toml` are exposed as structured MCP actions governed by strict security policies:

| Problem | How TorqKage Addresses It |
| :--- | :--- |
| **Unrestricted shell execution:** Agents run arbitrary commands with host user privileges. | **Two-layer containment:** Linux tools declaring `enforce_sandbox = true` (or granular `[tools.<name>.sandbox]` profiles) run in fail-closed Bubblewrap namespaces; macOS, Windows, and non-hook clients run under the **Universal In-Band MCP Seatbelt**. |
| **Token overhead:** Long prompt descriptions and multi-kilobyte skills burn context on every interaction. | **Native engine + declarative filters:** Parameter validation, `[tools.<name>.output_filter]` pruning, MCP resources (`tkg://rules`, `tkg://status`), and prompts (`tkg-refactor`, `tkg-audit`) eliminate markdown skill bloat. |
| **Lost session context:** Disconnected terminal sessions lose state across harness restarts. | **Unified `Harness` session resumption:** Discovers SQLite, JSONL, and JSON histories across Antigravity, Claude Code, OpenCode, Cursor, and custom runners directly from `tkg` (`Enter` / `r`). |
| **Working tree pollution:** Agent modifications clutter the primary working directory and editor buffers. | **Jujutsu workspace isolation:** Agents work in dedicated, sparse Jujutsu working copies (`.tkg/workspaces/<agent>`) without modifying your active working copy. |
| **Failing command loops:** Agents repeat the same failing compiler or test command indefinitely. | **Two-strike circuit breaker:** Halts mutating calls after consecutive failures (via `tkg-hooks` or in-band inside `tkg-mcp`) until reviewed or reset (`c` / `tkg -r`). |

---

## Quickstart

### 1. Precompiled Binaries (`install.sh` POSIX `/bin/sh`)
Install precompiled binaries (`tkg`, `tkg-mcp`, `tkg-hooks`) with `.sha256` checksum verification (supports Linux `x86_64`/`aarch64` `gnu`/`musl` and macOS):

```sh
curl -sSL https://raw.githubusercontent.com/JeronimoCapelle/torqkage/main/install.sh | sh
```

Select the target agent harness with `--harness` (`agy` default, `claude`, `opencode`, `cursor`, or a custom slug) and scaffold `~/.config/tkg/config.toml` non-interactively:

```sh
curl -sSL https://raw.githubusercontent.com/JeronimoCapelle/torqkage/main/install.sh | sh -s -- --harness claude --init-config
```

```toml
# ~/.config/tkg/config.toml (or <project>/.tkg/project.toml [runner])
default_harness = "claude"
```

### 2. Install via Crates.io
Install the CLI control deck (`tkg`), MCP daemon (`tkg-mcp`), and hook interceptor (`tkg-hooks`) via Cargo:

```bash
cargo install torqkage tkg-mcp tkg-hooks
```

### 3. Build From Source
Clone the repository and build from source (`--perf` for Fat-LTO stripped binaries, or `--fast` for Cranelift + Mold dev loops):

```bash
git clone https://github.com/JeronimoCapelle/torqkage.git
cd torqkage
./install.sh --build-from-source --perf
```

### 4. Launch the Control Deck
Run the interactive terminal dashboard (`-v`/`-vv`/`-vvv` enables structured `tracing` logs on `stderr`):

```bash
tkg            # Interactive Ratatui control deck
tkg -a coder   # Select or launch an agent directly (flags work in any order)
tkg -r         # Reset circuit breaker strike counter
```

---

## Usage Guide

### 1. Terminal Dashboard (`tkg`)
`tkg` provides a terminal interface to inspect tools, configure permission gates, monitor sessions, and launch agents:

| Keybinding | Action |
| :--- | :--- |
| <kbd>Enter</kbd> / <kbd>r</kbd> | **Resume session / Launch agent:** Resume the selected session or launch the active agent in the configured harness |
| <kbd>b</kbd> / <kbd>F8</kbd> | **Toggle Bubblewrap sandbox:** Switch between host execution and jailed Bubblewrap containment |
| <kbd>w</kbd> | **Toggle workspace isolation:** Run sessions in isolated Jujutsu workspaces (`.tkg/workspaces/<agent>`) |
| <kbd>c</kbd> / <kbd>Shift-K</kbd> | **Reset circuit breaker:** Clear tripped failure counts in `.tkg/state/` |
| <kbd>F2</kbd> | **Save & validate:** Validate agent frontmatter against schemas and run pre-flight gates in real time |
| <kbd>Tab</kbd> / <kbd>Shift-Tab</kbd> | Switch panels between the Agent Fleet Matrix, Tool Catalog, Directives, and Manual |
| <kbd>?</kbd> | Open the embedded manual and cheat sheet |
| <kbd>q</kbd> / <kbd>Esc</kbd> | Exit `tkg` |

### 2. Launching Agents
Invoke configured agents from the command line:

```bash
tkg launch coder
tkg launch test-author
tkg launch finisher
```

Or run via the underlying harness directly:

```bash
claude --agent coder      # or opencode --agent coder / agy --agent coder
```

### 3. Developer Workflows & Build Acceleration (`justfile`)
The workspace [`justfile`](justfile) provides Cranelift + Mold dev loops, Fat-LTO production profiles, fixed-point Clippy auto-fixing, sanitizers, and full CI verification:

```bash
just fast          # Cranelift + Mold compile check across workspace
just ft            # Cranelift + Mold nextest test loop (`fast` profile)
just perf          # Fat-LTO + target-cpu=native release build
just min-size      # Stripped opt-level="z" minimal binary build (defaults to tkg-hooks)
just fix           # Fixed-point auto-fix loop (rustc + clippy --fix <=5 passes + fmt + tests)
just verify        # Full CI gate: fmt, clippy -D warnings, nextest, doctest, deny, machete, typos, docs
```

---

## Ecosystem Starter Kits

Pre-configured starter setups are available in [`docs/templates/`](docs/templates/):

| Starter Kit | Target Ecosystem | Tooling & Sandboxing Highlights | Agent Team |
| :--- | :--- | :--- | :--- |
| **[Go Microservices](https://jeronimocapelle.github.io/torqkage/templates/go-setup.html)** | Go 1.22+ (`cmd/`, `internal/`) | Sandboxed `go test`, enum flags (`-short`, `-race`), migration permission gates | `coder`, `reviewer` |
| **[Python & Data Engineering](https://jeronimocapelle.github.io/torqkage/templates/python-setup.html)** | Python 3.12+ (uv, pytest, ruff, mypy) | Sandboxed `pytest` with regex filters, Mypy typing, automated Ruff fixer | `coder`, `tester` |
| **[TypeScript & Fullstack Web](https://jeronimocapelle.github.io/torqkage/templates/typescript-setup.html)** | Node.js / React / Next.js (pnpm, Vitest, tsc) | Sandboxed `vitest` with target scoping, `tsc --noEmit` gate, ESLint check | `frontend`, `reviewer` |
| **[Standard Rust Systems](https://jeronimocapelle.github.io/torqkage/templates/rust-setup.html)** | Rust 2024 Edition (Cargo workspaces) | Sandboxed `cargo test`, enum targets (`--lib`, `--bins`), Clippy gate | `coder`, `doc-author` |
| **[Adversarial QA Auditor](https://jeronimocapelle.github.io/torqkage/templates/qa-tester.html)** | Ecosystem-agnostic | Fuzz testing with LLVM sanitizers (enums), schema verification | `tester` (`tester.md`) |

To use a starter kit, copy its configuration into `.tkg/`:

```bash
mkdir -p .tkg
cp -r docs/templates/go/* .tkg/
tkg
```

---

## Declarative Fleets from Prompts

Because `TorqKage` configurations are declarative TOML and Markdown, an LLM can generate a customized fleet setup from schemas:

```text
"Act as a TorqKage Fleet Architect. I am building a project in <Language/Framework>.
Based on the TorqKage declarative schemas, generate:
1. project.toml: with gates for compilation, unit tests, and linting.
2. tools.toml: declaring test runners and diagnostic tools with enforce_sandbox = true,
   strict regex parameter validation, and enums for mutually exclusive flags.
3. hooks.toml: with a 2-strike compiler circuit breaker.
4. agents/: 2 specialized agent personas with clean allowed_dirs and mcp_tools allowlists."
```

Place the generated files in `.tkg/` and run `tkg` to activate the configuration.

---

## The 10-Agent Reference Fleet

The reference fleet in `plugins/torqkage/` follows a Design Verification (DV) discipline where implementation engineers do not author their own testbenches:

| Agent Profile | Role & Specialty | Permitted Directory Scope | Content & Security Policy |
| :--- | :--- | :--- | :--- |
| **[`coder`](plugins/torqkage/agents/coder.md)** | Systems implementer (Rust, RISC-V asm) | `src/`, `crates/`, `boards/`, `targets/` | Full code mutation; `Cargo.toml` protected by `manifest_guard` |
| **[`test-author`](plugins/torqkage/agents/test-author.md)** | Design Verification (DV) & Formal Test Engineer | `tests/`, `benches/`, `src/` | `tests_only` in `src/` (`mod tests`); formal proofs (Kani, Loom, Miri) |
| **[`doc-author`](plugins/torqkage/agents/doc-author.md)** | Specification & Rustdoc Author | `docs/`, `specs/`, `README.md`, `ARCHITECTURE.md` | `comments_only` in `src/` (in-place `///` rustdoc without logic edits) |
| **[`planner`](plugins/torqkage/agents/planner.md)** | Architecture & Verification Strategy | `docs/`, `ARCHITECTURE.md` | Authoritative architecture RFCs and upfront verification planning |
| **[`copilot`](plugins/torqkage/agents/copilot.md)** | Workflow Coordinator | Read-only across repository | Read-only code inspection; Jujutsu workspace rebase and merge |
| **[`finisher`](plugins/torqkage/agents/finisher.md)** | Release Gatekeeper | Read-only across repository | Pre-release checks (fmt, clippy, tests, diffs) |
| **[`spec-checker`](plugins/torqkage/agents/spec-checker.md)** | Spec & ISA Compliance Auditor | Read-only across repository | AST queries, opcode audits, and spec-to-source traceability |
| **[`perf-analyst`](plugins/torqkage/agents/perf-analyst.md)** | Performance & Benchmark Engineer | `benches/`, `docs/`, `.tkg/artifacts/` | Microbenchmarking (Divan/Criterion), flamegraphs, `llvm-mca` |
| **[`mini`](plugins/torqkage/agents/mini.md)** | Symbolic Math Specialist | Strictly air-gapped | Network-isolated Bubblewrap container using Python/SymPy |
| **[`sysadmin`](plugins/torqkage/agents/sysadmin.md)** | Linux System Administrator | Full system | Supervised shell; requires operator confirmation on mutating actions |

---

## Configuration

Tools and agents can be added or modified without recompiling binaries:

### 1. Adding a Tool in `tools.toml` (4-Tier Cascade)
`TorqKage` deep-merges configuration across 4 tiers (`defaults` < `~/.config/tkg/` < `plugins/torqkage/` & `.tkg/` < CLI/env, supporting `merge_strategy = "replace" | "append" | "prepend" | "unique"`). Declare tools in [`plugins/torqkage/tools.toml`](plugins/torqkage/tools.toml) or `.tkg/tools.toml` with optional `[tools.<name>.sandbox]` profiles and `[tools.<name>.output_filter]` rules:

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

### 2. Defining an Agent
Create [`plugins/torqkage/agents/my-agent.md`](plugins/torqkage/agents/) or `.tkg/agents/my-agent.md`:

```markdown
---
name: my-agent
description: "Specialized auditing agent"
effort: high
mainAgent: true
permissionMode: dont-ask
allowed_dirs: ["src", "tests"]
mcp_tools:
  - read_project_file
  - search_codebase
  - my_scanner
pre_launch_checks:
  - compilation
  - vcs_conflicts
---

You are the specialized auditing agent for this repository.
```

Saved agents appear in `tkg` with full gate enforcement.

---

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
cargo bench -p tkg-common --bench core_benches      # Divan latency & allocation profiler
cargo bench -p tkg-common --bench gungraun_bench    # Callgrind instruction counts (Linux + Valgrind)
```

See the full breakdown in [Benchmarks & Latency Profiles](docs/src/benchmarks.md).

---

## Documentation

For architecture details, system invariants, and guides:

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

Licensed under the **GNU Affero General Public License v3.0 or later** ([AGPL-3.0-or-later](LICENSE)).
