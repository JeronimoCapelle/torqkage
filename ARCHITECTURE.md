# TorqKage: System Architecture & Invariant Manual

**Document Version:** 1.1.0  
**Status:** Canonical & Permanent Architecture Specification  
**Authority:** Root repository manual for human developers and autonomous AI agents (`coder`, `planner`, `spec-checker`, `finisher`, `copilot`).

> **Quickstart & Usage:** For installation, terminal workflows, and the interactive control deck guide, see [README.md](README.md).  
> **Online Engine Manual:** For full documentation, guides, and tutorials, visit the [TorqKage Engine Manual](https://jeronimocapelle.github.io/torqkage/).

---

## 1. System Invariants & Non-Negotiable Directives

All autonomous agents and human contributors operating within this repository or deploying `TorqKage` MUST adhere to the following invariants:

1. **Manifest Integrity (Strict Prohibition):**
   - **NEVER** autonomously edit `Cargo.toml`, `Cargo.lock`, or `deny.toml`.
   - Modifying dependencies or build configurations without human review is prohibited to prevent dependency bloat, accidental malicious package injection, and proc-macro execution vectors.
   - If a new dependency is required, the agent must ask the operator for permission with the package name, requested version, and justification before modifying manifests.
2. **Compiler & Linter Cleanliness:**
   - The workspace enforces `#![forbid(unsafe_code)]`, `#![deny(clippy::correctness, clippy::suspicious, clippy::unwrap_used)]` (with `.clippy.toml` test exemptions cleanly isolated to tests), and curated high-signal pedantic/nursery rules in `Cargo.toml`.
   - **Zero compiler, clippy, or rustdoc warnings** (`RUSTDOCFLAGS="-D warnings"`) across `--workspace --all-targets --all-features`.
   - `allow_attributes_without_reason = "deny"`: every `#[allow(...)]` attribute requires an explicit `reason = "..."`.
3. **Robust Error Handling:**
   - **Zero unhandled `unwrap()` or `expect()` calls** in production code. Errors use typed enumerations (`thiserror`, `TkgError::from_toml` with `line:col` caret snippets) or explicit propagation; configuration loading always fails closed on syntax/parse errors.
   - Zero silent `drop(...)` suppression of fallible I/O or VCS mutations in production paths.
4. **Subprocess Supervision & Hang Immunity:**
   - **Never** invoke raw synchronous `Command::new(...).output()` without timeout bounding.
   - All external processes execute through `SubprocessSupervisor::run_supervised` with bounded deadlines (`tkg_common::limits`), Unix `O_NONBLOCK` + `nix::poll` non-blocking pipe draining, staged `SIGTERM` -> grace -> `SIGKILL` `-pgid` process-group teardown, and bounded post-exit drain against detached grandchildren.
5. **Pure Safe Rust & Structured Tracing:**
   - The entire workspace enforces `#![forbid(unsafe_code)]` with zero raw FFI blocks.
   - All binaries initialize `tkg_common::diagnostics::init_stderr_tracing` (`-v`/`-vv`/`-vvv` or `RUST_LOG`) directed strictly to `stderr`, keeping `stdout` wire protocols (`JSON-RPC` and `ProtoJSON`) and TUI output clean.
6. **Thread Safety & Rust 2024 Compliance:**
   - **Zero** `std::env::set_var` or `remove_var` calls in production or test code. Parameterize environment lookups to prevent multithreaded data races.
7. **Fast Verification & Quality Gate Standard:**
   - Use `just fast` / `just ft` (Cranelift + Mold), `just fix` (fixed-point `rustc` + `clippy --fix` + `fmt` + test loop), and `just verify` (`fmt`, `clippy -D warnings`, `nextest`, doctests, `cargo deny check`, `cargo machete`, `typos`, `cargo doc`) before committing.

---

## 2. The Three Declarative Contracts

`TorqKage` enforces a strict separation between **Platform Plumbing**, **Project Environment**, and **Agent Tooling**:

```
+-----------------------------------------------------------------------------------+
| 1. tools.toml (The Agent's Hands)                                                 |
|    - Declares MCP tools exposed to agents, base templates, actions, and limits.   |
|    - Language-agnostic declarative catalog with codeless gate execution.          |
+-----------------------------------------------------------------------------------+
| 2. hooks.toml (The Runtime's Rules)                                               |
|    - Declares lifecycle interception pipeline (<1ms native Rust execution).       |
|    - Circuit breaker strike logic, manifest mutation guards, conflict sentries.  |
|    - Declarative Turn 1 wake context injections ([[session_start.injections]]).   |
+-----------------------------------------------------------------------------------+
| 3. project.toml (The Codebase's Identity)                                         |
|    - Declarative contracts for the target repository.                             |
|    - Pre-invocation gates ([gates.<name>]), workspace containers, build cache     |
|      bind mounts, documentation paths, and security allow/deny rules.             |
+-----------------------------------------------------------------------------------+
```

### Hierarchy & 4-Tier Cascaded Merge (`tkg_common::cascade`)
Configuration loading uses `CascadedConfigLoader` across four precedence tiers (lowest to highest) rather than whole-file replacement:
1. **Built-in Invariants & Ecosystem Auto-Detection:** Embedded defaults + disk manifest inference (`Cargo.toml`, `build.zig`, `go.mod`, `pyproject.toml`, `package.json`, `meson.build`, `CMakeLists.txt`).
2. **User Global:** `~/.config/tkg/config.toml` (supports top-level `default_harness` shorthand), `~/.config/tkg/project.toml`, `~/.tkg/project.toml`, and installed plugin bundle configs.
3. **Workspace Local:** `<ws>/plugins/torqkage/*.toml`, `<ws>/*.toml`, `<ws>/.tkg/*.toml`.
4. **Runtime Env & CLI Overrides:** `--agent`/`-a`, `-r`/`--reset-breaker`, `--harness`, dotted-key overrides, and `-v`/`-vv`/`-vvv` verbosity flags.

- Tables deep-merge recursively; arrays support `merge_strategy = "replace" | "append" | "prepend" | "unique"`.
- Malformed TOML or invalid JSON configs always **fail closed** with line-numbered caret diagnostics (`TkgError::from_toml`), never falling back silently to permissive defaults.

### 2.1 Engine vs. Reference Fleet Separation
`TorqKage` is fundamentally an **extensible, capability-governed engine and sandboxed MCP runtime**. The 10-agent fleet described in [Section 3](#3-agent-fleet-perimeters--blast-boundaries) represents the flagship reference implementation tailored for low-level systems engineering and Silicon Design Verification.

Any developer or autonomous AI can define bespoke fleets and tool catalogs for different languages and domains (e.g. Go, Python, TypeScript, or custom DSLs) simply by declaring a `.tkg/` directory with `project.toml`, `tools.toml`, `hooks.toml`, and agent Markdown profiles (`agents/*.md`). Pre-built ecosystem starter kits are maintained in `docs/templates/` and documented in the [Engine Manual](https://jeronimocapelle.github.io/torqkage/templates/overview.html).

---

## 3. Agent Fleet Perimeters & Blast Boundaries

Each agent in `plugins/torqkage/agents/` operates under the **Air-Gapped Least-Privilege Model**, with boundaries and pre-launch checks unified directly in each agent's Markdown frontmatter:

| Agent Profile | Role & Responsibility | Permitted Directory & File Scope | Path & Content Policies | Mandatory Pre-Invocation Gates | Available Tooling & Operational Scope |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **`coder.md`** | Production systems implementation (high-level Rust to bare-metal RISC-V & asm) | `allowed_dirs = ["src", "crates", "boards", "layout", "targets"]`<br>`disallowed_dirs = ["tests", "benches", "docs"]` | Masked manifests; asks operator for `Cargo.toml` dependency edits (`manifest_guard`) | `vcs_conflicts` | Full code mutation in source trees; compiler diagnostics, `mpatch_edit`, `write_file`, `search_codebase`, `read_project_file`, `cargo_check`, `cargo_test`, `cargo_clippy`. |
| **`copilot.md`** | Chief of Staff & Workflow Orchestrator | Strictly read-only code inspection (`disallowed_dirs = ["*"]`) | Read-only code inspection; workspace Jujutsu merge/rebase permitted | None | High-level fleet coordination across 4-phase lifecycle, subagent delegation, handoff inspection, Jujutsu workspace rebase/merge (zero direct code mutation). |
| **`doc-author.md`** | Technical docs & specification author | `allowed_dirs = ["docs", "specs", "src"]`<br>`allowed_files = ["README.md", "ARCHITECTURE.md"]`<br>`disallowed_dirs = ["tests", "benches"]` | `comments_only` path policy in `src/` (in-place `///` rustdoc without modifying code logic) | `vcs_conflicts` | API reference docs, pedagogical rustdoc (`///`, `//!`), register bitfield layouts, ASCII diagrams, RFCs, module structure, `cargo doc --no-deps`. |
| **`test-author.md`** | Design Verification (DV) & Formal Test Engineer | `allowed_dirs = ["tests", "benches", "src"]`<br>`disallowed_dirs = ["docs"]` | `tests_only` in `src/` (`mod tests` syntactic filter); asks operator for test deps | `vcs_conflicts` | Test runners (`nextest`, `mutants`, `insta`, `fuzz`), formal verification (`kani`), concurrency permutation (`loom`), memory safety (`miri`), compiler explanations, differential inspection (`llvm-objdump`). |
| **`planner.md`** | Architecture & Verification Strategy Rubber-Duck | `allowed_dirs = ["docs", ".scratchpad"]`<br>`allowed_files = ["ARCHITECTURE.md"]`<br>`disallowed_dirs = ["src", "tests", "benches"]` | Authoritative design documents & mandatory upfront verification plans | None | Read-only codebase queries, architecture editor, mandatory upfront verification strategy selection. Pre-injected with workspace crate topology. |
| **`finisher.md`** | Adversarial release gatekeeper | Strictly read-only across workspace (`disallowed_dirs = ["*"]`) | Zero mutation; pre-release gauntlet scorecard enforcement | `fmt`, `compilation`, `tests`, `vcs_conflicts` | Semantic diff review, pre-injected wake diff & changelog preview, binary size audit (`cargo_size`), supply-chain audit (`cargo_deny`), mutation scoring, mandated proof checks. |
| **`spec-checker.md`** | Spec-vs-code & ISA compliance auditor | Strictly read-only across workspace (`disallowed_dirs = ["*"]`) | Zero mutation; cross-references implementation against specs & ISA contracts | `vcs_conflicts` | Spec searches, ISA opcode checks, AST grep queries, active diff inspection (`inspect_workspace_diff`), clippy, cargo-deny, semver-checks. |
| **`perf-analyst.md`** | Microarchitectural profiler & benchmark engineer | `allowed_dirs = ["benches", "docs", ".tkg/artifacts"]`<br>`disallowed_dirs = ["src", "tests"]` | Microbenchmarking, execution traces, latency distributions | `compilation`, `vcs_conflicts` | Profilers, Divan/Criterion benchmarks, flamegraphs, asm inspection, llvm-mca, hyperfine, bloat, section size & symbol mapping (`cargo-size`, `cargo-nm`). |
| **`mini.md`** | Narrow symbolic math & calculation specialist | Strictly air-gapped (`disallowed_dirs = ["*"]`) | Ephemeral scratchpad only; zero disk mutations | None | `eval_symbolic_math` (Python/SymPy) inside isolated container only. |
| **`sysadmin.md`** | High-reasoning Arch Linux system administrator | `allowed_dirs = ["*"]` (full system access) | Supervised shell; interactive operator confirmation (`ASK` / `[y/n]`) on all state-altering tools | None | Host troubleshooting, systemd, pacman inspection, `run_command`, `ask_question`. |

### Workspace Isolation
- Each agent operates in its own sparse Jujutsu workspace at `.tkg/workspaces/<agent>/`.
- Agent modifications remain cleanly isolated from the operator's active `@` commit and open editor buffers.
- The shared build cache (e.g. `target/`) is symlinked to ensure compilation artifacts are shared without disk bloat.

---

## 4. The Four-Phase Fleet Lifecycle & Silicon Design Verification (DV) Framework

In low-level systems engineering (such as RISC-V processor cores, kernel runtimes, and lock-free concurrency engines), **formal correctness** and **performance optimization** are distinct disciplines with opposing feedback loops. `TorqKage` organizes autonomous agents into four structured phases:

```
  PHASE 1: CONTRACT & AUDIT           PHASE 2: IMPLEMENTATION & DV          PHASE 3: AUDIT & PROFILING
┌───────────────────────────┐       ┌───────────────────────────┐       ┌───────────────────────────┐
│        1. PLANNER         │       │         3. CODER          │       │      5. PERF ANALYST      │
│  - Debates architecture   │       │  - Sandboxed in src/**    │       │  - Latency & Throughput   │
│  - Mandates VERIFIERS     │       │  - Iterates to green tests│       │  - Cache lines & Flamelog │
│    (Loom, Kani, Miri)     │       └─────────────┬─────────────┘       │  - asm & llvm-mca ports   │
└─────────────┬─────────────┘                     │                     └─────────────┬─────────────┘
              │                                   ▼                                   │
              ▼                     ┌───────────────────────────┐                     ▼
┌───────────────────────────┐       │  4. VERIFICATION (DV)     │       ┌───────────────────────────┐
│      2. SPEC CHECKER      │       │  - Edge-case unit tests   │       │       6. FINISHER         │
│  - Audits ARCH against    │       │  - Kani proofs / Miri     │       │  - Mutation test audit    │
│    official specs & ISAs  │       │  - Loom concurrency tests │       │  - Mandated proof check   │
│  - Functional coverage bin│       │  - Differential oracles   │       │  - Clippy & Git diff audit│
└───────────────────────────┘       └───────────────────────────┘       └───────────────────────────┘
```

### Core Engineering Invariants:
1. **Upfront Verification Planning (Zero Timing Paradox):**
   - The Planner MUST declare the `Required Verification Strategy` in `ARCHITECTURE.md` during inception:
     * *Pure logic & bit-twiddling:* Kani formal proof (`cargo_kani`).
     * *Atomics & channels:* Loom C11 memory permutation harness (`cargo_loom`).
     * *Raw pointers & `#![no_std]` buffers:* Miri pointer provenance & Tree Borrows (`cargo_miri`).
     * *Instruction decoders (e.g. RISC-V):* Differential execution against reference models or `llvm-objdump` (`llvm_objdump`).
   - Late-stage verification tool discovery at release is strictly forbidden.
2. **The Silicon Design Verification (DV) Split:**
   - Under hardware discipline (ASIC/RISC-V design), the engineer who writes implementation logic **never** authors the verification testbench.
   - `test-author` (DV Engineer) authors adversarial test harnesses to actively break the system.
   - `coder` has read-only access to `tests/` and write access strictly to `src/**`, preventing test tampering or assertion weakening.
3. **Differential Oracles Over Hardcoded Asserts:**
   - For instruction sets and binary protocols, tests generate instruction bitstreams, run them through custom execution logic, and assert register state equality against an external disassembler (`llvm-objdump`) or reference simulator.
4. **Functional Coverage Bins Over Line Coverage:**
   - Tests must target state-space coverage bins (e.g. shift operations with sign-bit 1 vs 0, page-boundary memory crossings, interrupt cycle transitions, privilege register bitfields) rather than superficial line counts.
5. **Mutation Testing Gate (`cargo mutants`):**
   - Autonomous tests are validated via mutation scoring (`cargo_mutants`). Any test suite where synthetic mutants survive is rejected as ineffective/tautological.
6. **Purification of Performance Profiling:**
   - `perf-analyst` strictly measures throughput, latency distributions, CPU execution pipeline ports (`llvm_mca`), disassembly (`cargo_asm`), cache misses (`cargo_perf`), and binary size (`cargo_bloat`). It is entirely decoupled from correctness/UB tools.
7. **Unified Systems Implementation & Documentation Separation:**
   - Systems implementation across all abstraction tiers—from high-level business logic down to bare-metal RISC-V initialization, linker scripts (`link.ld`, `memory.x`), section allocation (`.text`, `.data`, `.bss`), and inline assembly—is unified in `coder`.
   - Technical rustdoc (`///`), architectural RFCs, register bitfield diagrams, and specifications are authored by `doc-author`.
8. **Deterministic Pre-Release Gauntlet (`pre_release_gauntlet`):**
   - Rather than executing 6–8 sequential CLI commands across multiple LLM turns, the release gatekeeper executes `pre_release_gauntlet` in a single shot. This evaluates VCS conflicts, unit/integration tests, clippy `-D warnings`, rustfmt check, ancillary targets, and doc build in one pass, outputting a consolidated scorecard.

---

## 5. Subagent Admission & Quality Gatekeeping

When `invoke_subagent` is called, `tkg-hooks` evaluates the `subagent_admission` gate before the subagent can boot:
1. Target agent's frontmatter `pre_launch_checks: [...]` is extracted.
2. Gates are evaluated:
   - Built-in VCS conflict check (`vcs_conflicts` blocks invocation if merge conflict markers are detected).
   - Built-in workspace cleanliness check (`workspace_clean`).
   - Declarative project gates matched against `project.toml [gates.<name>]` (e.g. `compilation`, `tests`, `linter`, `fmt`). Any gate check (such as code formatters or test suites) can be configured purely in TOML without modifying compiled Rust binaries.
3. If any gate check fails:
   - Admission is **denied** before burning LLM tokens.
   - Diagnostic output is returned to the calling agent with specific failure lines to guide remediation.

### 5.1 Declarative Wake Context Injections (`[[session_start.injections]]`)

To prevent autonomous agents from wasting 5–10 LLM turns and tens of thousands of tokens running repetitive CLI commands on wake-up:
1. `hooks.toml` defines declarative wake injections matching agent names or patterns:
   - **`copilot`**: Injects active Jujutsu working copy and commit status (`jj status --no-pager`) so it greets the user with immediate situational awareness.
   - **`planner`**: Injects workspace crate topology (`cargo tree --depth 0 --workspace`) so it immediately knows all member crates and paths.
   - **`spec-checker`**: Injects uncommitted file modifications (`git status --short`) pinpointing modified files awaiting spec auditing.
   - **`finisher`**: Injects active workspace diff summary (`jj diff --summary`), parent revision stats (`jj show @- --stat`), git status (`git status --short`), and recent unreleased Conventional Commits changelog (`git-cliff`).
2. When any matched agent is invoked, `tkg-hooks` intercepts `SessionStart` (Turn 1), executes the configured command under strict timeouts, bounds the output line count, and injects the live ground truth directly into the agent's wake prompt.
3. The agent starts Turn 1 with immediate awareness of essential ground truth without firing a single MCP tool call.

---

## 6. Security & Filesystem Confinement

Enforced across all tools (`read_project_file`, `write_file`, `modify_code`, `search_codebase`):

1. **Path Traversal Defense:** Absolute paths outside the workspace and relative escapes (`../`) are rejected.
2. **Sensitive File Protection:**
   - Generic credentials (`password`, `secret`, `token`, `credential`, `api_key`).
   - VCS internals (`.git`, `.jj`, `.hg`, `.svn`).
   - SSH private keys (`id_rsa`, `id_ed25519`, `id_ecdsa`).
   - Shell profiles (`.bashrc`, `~/.zshrc`, `.profile`).
   - Sensitive cryptographic extensions (`.key`, `.pem`, `.pfx`, `.p12`, `.keystore`).
   - Configurable per-project via `project.toml [security]`.
3. **Manifest Mutation Guard:**
   - Intercepts mutating tools targeting build scripts (`build.rs`, `Cargo.toml`, `package.json`, `CMakeLists.txt`, `Makefile`).
   - Neutralizes procedural macro and build-script host code execution vulnerabilities.

---

## 7. Directory Layout of the Repository

```text
torqkage/
├── ARCHITECTURE.md     # Canonical system architecture and invariant manual (this file)
├── README.md           # Repository overview, quickstart, configuration, and benchmarks
├── Cargo.toml          # Workspace root manifest (strict workspace lints, fast/perf/min-size profiles)
├── deny.toml           # Supply-chain security, RustSec advisories, and license policy
├── justfile            # Workspace automation (fast/perf/min-size, fix loop, nextest, sanitizers, verify)
├── install.sh          # POSIX /bin/sh multi-arch installer, .sha256 verifier, and config scaffolder
├── .clippy.toml        # Test-only lint exemptions for strict production safety
├── .typos.toml         # Source & docs spellcheck exceptions
├── .github/workflows/  # CI (ci.yml), macOS/Windows cross-platform checks (cross.yml), weekly (weekly.yml)
├── docs/               # mdBook User & Engine Manual + ecosystem starter kits (docs/templates/)
├── fuzz/               # cargo-fuzz harnesses (excluded from main workspace builds)
├── crates/             # Native 4-crate Rust 2024 workspace
│   ├── tkg-common/     # Core domain models, Harness trait, cascade, sessions, sandbox, supervisor, diagnostics, benches/
│   ├── tkg-mcp/        # Persistent MCP daemon (stdio JSON-RPC 2.0, 14 actions, resources, prompts, in-band seatbelt)
│   ├── tkg-hooks/      # Featherweight lifecycle interceptor CLI (avoids tokio, rusqlite, ratatui)
│   └── tkg-ctrl/       # Interactive Ratatui 0.30 TUI dashboard (binary: tkg), language-agnostic catalog, launcher
└── plugins/
    └── torqkage/       # Self-contained plugin & reference 10-agent fleet bundle
        ├── plugin.json # Host manifest
        ├── hooks.json  # Host lifecycle hook bindings
        ├── project.toml# Declarative project contracts and multi-language scaffolds
        ├── hooks.toml  # Lifecycle interception pipeline and circuit breaker rules
        ├── tools.toml  # Declarative MCP tool catalog, sandbox profiles, and output filters
        ├── README.md   # Plugin documentation and customization cookbook
        ├── bin/        # Local build/helper shims (cargo-fast, staged binaries)
        └── agents/     # Fleet specifications (coder, copilot, doc-author, finisher, etc.)
```

---

## 8. High-Assurance Systems Architecture: The 4 Crates, Unified Harness & Sandboxing

### High-Level Topology

```mermaid
flowchart TD
    subgraph Host ["Host & Human Interaction"]
        Operator["Human Operator (Terminal)"] <-->|"Ratatui 0.30 TUI"| Ctrl["tkg (Control Deck)"]
        Ctrl -.->|"Harness::discover_sessions()"| Sessions[("SQLite / JSONL / JSON\nSession Histories")]
    end

    subgraph RuntimeBridge ["Orchestration & Sandbox Bridge"]
        Ctrl -->|"Harness::build_spawn_cmd()\n(Raw mode suspended)"| HarnessCLI["Execution Harness Runtime\n(agy / claude / opencode / cursor / custom)"]
        Ctrl -.->|"jj workspace add .tkg/workspaces/<agent>"| JJWorkspaces[".tkg/workspaces/<agent>\n(Jujutsu Isolation)"]
    end

    subgraph PluginServices ["Lifecycle & Plugin Services"]
        HarnessCLI <-->|"JSON-RPC 2.0 over stdio\n(tools, resources, prompts, seatbelt)"| MCPDaemon["tkg-mcp (Tool Daemon)"]
        HarnessCLI <-->|"HookDialect stdin / stdout\n(ProtoJSON / Claude JSON)"| HooksRunner["tkg-hooks (Ephemeral CLI)"]
    end

    subgraph DeclarativeState ["4-Tier Cascaded Configs & State"]
        ToolsTOML["tools.toml\n(Tools, SandboxProfile, OutputFilter)"] -.->|"Cascaded load"| MCPDaemon
        ToolsTOML -.->|"Queried for mutating & permission flags"| HooksRunner
        AgentMD["agents/*.md\n(YAML frontmatter: mcp_tools, pre_launch_checks)"] -.->|"Dynamic Scoping"| MCPDaemon
        AgentMD -.->|"Pre-Launch Quality Gates"| HooksRunner
        AgentMD -.->|"F2 Save & Validate"| Ctrl
    end

    subgraph ExecutionLayer ["Supervised Execution & Containment Layer"]
        MCPDaemon -->|"SubprocessSupervisor\n(O_NONBLOCK + nix::poll + pgid)"| Supervisor["tkg-common::SubprocessSupervisor"]
        Supervisor -->|"Linux bwrap --unshare-all\n(fail-closed unless allow_host_fallback)"| BwrapJail["Bubblewrap Sandbox\n([tools.<name>.sandbox])"]
        Supervisor -.->|"macOS / Windows / Non-Hook Harness"| Seatbelt["Universal In-Band Seatbelt"]
        BwrapJail --> NativeTools["Native Binaries (cargo, jj, etc.)"]
        MCPDaemon <-->|".tkg/state/circuit_breaker.json"| CircuitBreaker[("Circuit Breaker Ledger")]
        HooksRunner <-->|".tkg/state/handoff.json"| HandoffState[("Session Handoff Bridge")]
    end
```

### 8.1 The 4 Crates & Unified Harness Architecture
1. **`tkg-common` (Systems Foundation):**
   - **`Harness` Capability Seam (`harness.rs`):** Unifies static CLI flags, `HarnessCapabilities` (`NATIVE_HOOKS`, `SESSION_RESUME`, `HEADLESS_SPAWN`, `NATIVE_MCP`), `HookDialect` (`ProtoJSON`, `ClaudeCode`), cross-platform session discovery (`sessions.rs`), config injection (`harness_install.rs` — `.mcp.json` + `.claude/settings.json`, `.cursor/mcp.json`, OpenCode/Antigravity bundles), and spawn command assembly for `Antigravity`, `ClaudeCode`, `OpenCode`, `Cursor`, and `CustomHarness`.
   - **`CascadedConfigLoader` (`cascade.rs`):** 4-tier deep TOML merge (`merge_strategy` = `replace`/`append`/`prepend`/`unique`), `default_harness` shorthand normalization, and line-numbered caret parse errors (`TkgError::from_toml`).
   - **`SubprocessSupervisor` (`supervisor.rs`):** Bounded deadline execution with Unix `O_NONBLOCK` + `nix::poll` pipe draining, staged `SIGTERM` -> grace -> `SIGKILL` `-pgid` process-group termination, and bounded post-exit drain preventing detached grandchild pipe hangs.
   - **`JujutsuWorkspace` (`jujutsu.rs`):** Dynamic management of isolated Jujutsu workspaces at `.tkg/workspaces/<agent>/`, snapshotting, and gated rebase/squash merging with rollback (`jj op restore`).
   - **`ContentFilter` (`content_filter.rs`):** Language-agnostic syntactic path policies (`comments_only`, `tests_only`, `delimited`).
   - **`Diagnostics` (`diagnostics.rs`) & `TkgStatePaths` (`state.rs`):** `-v`/`-vv`/`-vvv` & `RUST_LOG` structured `tracing` to `stderr`, atomic file-locked `.tkg/state/` ledgers.
2. **`tkg-mcp` (Declarative Model Context Protocol Daemon):**
   - Implements native Model Context Protocol (MCP `2024-11-05`) over stdio JSON-RPC (`tools/*`, `resources/list` + `resources/read` for `tkg://rules` & `tkg://status`, and `prompts/list` + `prompts/get` for `tkg-refactor` & `tkg-audit`).
   - Identifies the calling harness via `initialize.params.clientInfo.name`, `TKG_HARNESS`, or Linux `/proc/<ppid>/cmdline` fallback, automatically enabling the **Universal In-Band MCP Seatbelt** (`in_band_seatbelt`) when `NATIVE_HOOKS` is absent or on macOS/Windows.
   - Applies `[tools.<name>.output_filter]` (`keep_regex`, `drop_regex`, `max_lines`) prior to output truncation and executes commands under `[tools.<name>.sandbox]` profiles.
3. **`tkg-hooks` (Featherweight Interception Runner):**
   - Kept as a lean, separate binary intentionally free of `tokio`, `rusqlite`, `ratatui`, and `tracing-subscriber` for minimal binary size and sub-millisecond cold CLI dispatch (`pre-tool-use`, `detect-wip`, `check-conflicts`, `check-subagent`, `enforce-tool-policy`, `record-handoff`, `inject-wake`).
   - Auto-detects `ProtoJSON` vs Claude Code `PreToolUse`/`SessionStart`/`Stop` payloads and emits matching dialects cleanly to `stdout`.
4. **`tkg-ctrl` (Interactive Ratatui 0.30 Terminal Deck — Binary: `tkg`):**
   - Standalone terminal dashboard providing interactive Fleet Matrix navigation, language-agnostic `tools.toml` category tabs (no hardcoded ecosystem enums), real-time circuit breaker inspection/reset (`c` / `Shift-K` or CLI `-r`), and cross-harness session resumption.

### 8.2 Granular Sandboxing & Fail-Closed Container Detection
- Tools opt into isolation via `enforce_sandbox = true`, `sandbox = true`, or a granular `[tools.<name>.sandbox]` profile (`engine = "bwrap" | "none"`, `allow_network`, `unshare_pid`, `read_only_binds`, `writable_binds`, `env_passthrough`, supporting `{cwd}` and `{home}` placeholders).
- On Linux, `build_bubblewrap_command_with_profile` isolates namespaces (`--unshare-all`, private loopback when `allow_network = false`), mounts `/usr`/`/bin`/`/lib`/`/etc` read-only, masks credentials to `/dev/null`, and binds declared caches rw.
- **Fail-Closed Guarantee:** Missing or non-functional `bwrap` (e.g. inside nested containers or non-Linux hosts) **fails closed** unless the tool explicitly configures `allow_host_fallback = true`. Outer container detection inspects `TKG_OUTER_SANDBOX`, harness sandbox env vars, container files (`/.dockerenv`, `/run/.containerenv`), `/proc/*/cgroup` runtime segments, `/proc/self/mountinfo` storage markers, nested `NSpid`, and container init comms (`tini`, `dumb-init`, `*bwrap*`).

### 8.3 External Tool Integrations
- Optional external tools (such as Tree-sitter AST code graph engines or systems trace visualizers) can be discovered via `$PATH` when installed on the host system.

### 8.4 Native MCP Access Points & Execution Guarantees
The `tkg-mcp` daemon replaces ambient shell access with 14 native action handlers (`ToolAction`):
1. **`file_read`:** Bounded file reader with line-range slicing (`start_line`, `end_line`) and UTF-8-safe byte caps.
2. **`file_write`:** Atomic writer enforcing directory perimeters, manifest protection, VCS conflict gates, and syntactic path policies.
3. **`mpatch_edit`:** Precision fuzzy block patcher (`<<<< SEARCH / ==== / >>>> REPLACE`) eliminating full-file rewrites while verifying manifest protections and VCS conflicts.
4. **`file_search`:** In-process regex/substring search across repository files and project doc trees without external dependencies.
5. **`static_command`:** Parameterless invocation of fixed audit binaries under process-group supervision and optional `bwrap` sandboxing.
6. **`parameterized_command`:** Bounded execution with strict per-argument regex/type/enum allowlists, conflict checks, and `[tools.<name>.sandbox]` / `output_filter` rules.
7. **`cli_manual`:** In-process system manual provider synthesizing `navi` cheat sheets, `tldr` summaries, manpage synopsis blocks, `--help` buffers, and compiler references.
8. **`project_state`:** Synthesizes active VCS revision, dirty file status, Jujutsu working copy commit, quality gate health, and previous session handoff context.
9. **`workspace_ensure`:** Provisions, switches to, or validates an isolated Jujutsu sparse workspace for a subagent (`.tkg/workspaces/<agent>/`).
10. **`workspace_inspect`:** Inspects working copy diffs and file modifications between subagent sparse workspaces and the main working tree.
11. **`workspace_merge`:** Absorbs or merges changes from an isolated subagent workspace back into trunk with declarative gate checks and rollback.
12. **`set_active_agent`:** Dynamically switches the active agent persona in `.tkg/state/active_agent.json` to update tool allowances and path policies on the fly.
13. **`record_session_handoff`:** Records structured session summary metadata into `.tkg/state/`.
14. **`detect_wip`:** Inspects uncommitted VCS working-copy modifications on demand.

---

## 9. Deep Systems Verification & Hardware Analysis Toolchain

`TorqKage` incorporates a battle-tested systems engineering and silicon verification toolchain:

| Domain | Tools & Frameworks | Architectural Purpose |
| :--- | :--- | :--- |
| **Compiler & Fast Path** | Cranelift, Mold, `cargo fast`, `sccache`, `zigbuild` | Sub-second dev iteration; Cranelift code generation + Mold linking for instant checks. |
| **Formal Verification** | `kani`, `miri` | Bit-precise model checking and SAT/SMT proofs (`kani`); Stacked/Tree Borrows UB detection (`miri`). |
| **Concurrency Testing** | `loom` | Exhaustive permutation testing of atomic memory orderings and lock-free data structures. |
| **Property & Mutation** | `proptest`, `cargo-fuzz`, `cargo-mutants`, `cargo-careful` | Generative property testing, libFuzzer fuzzing, mutation coverage, and extra UB checks. |
| **Trace & Microarchitecture** | `perfetto-mcp-rs`, `graphtrail`, `perf`, `llvm-mca`, `samply`, `flamegraph` | Kernel scheduler switch analysis (`.pftrace`), AST code intelligence, pipeline latency, CPU profiling. |
| **Benchmarking & Memory** | `hyperfine`, `criterion`, `divan`, `dhat`, `valgrind`, `bacon` | Statistical CLI benchmarks, microbenchmarks, heap allocation profiling, memory leak detection. |
| **Low-Level & Hardware** | `gdb`, `lldb`, `strace`, `qemu`, `nasm`, `zig`, `cargo-asm`, `cargo-bloat` | Kernel syscall tracing, ISA emulation, assembly inspection, and binary footprint analysis. |
| **Supply Chain & API Gates**| `cargo-deny`, `cargo-audit`, `cargo-machete`, `cargo-semver-checks`, `cargo-public-api`, `cargo-hack` | License/CVE compliance, unused dependency elimination, semver drift detection, and multi-target checks. |

---

## 10. Zero-Skill Architecture & Sandboxing Continuum

### 10.1 The Zero-Skill Policy (`NO SKILL.md`)
Interpreted markdown skills (`SKILL.md`) and unconfined bash wrappers exhibit severe defects in production:
- **Token Inefficiency:** Injecting multi-kilobyte markdown definitions consumes 2,000 to 4,000+ baseline tokens per conversational turn.
- **Unconfined Injection Surface:** Bash-based skills execute in ambient shells without argument bounds, timeouts, or output truncation.
- **`TorqKage` Solution:** Completely bans interpreted markdown skills. All agent capabilities are declared as native, compiled MCP handlers or bounded static commands with strict JSON Schemas, reducing prompt overhead to zero baseline tokens and guaranteeing deterministic execution bounds.

### 10.2 Four-Tier Security Continuum
```
[ Level 1: Operator Cockpit (tkg) ]
  └── Policy switches, model effort controls, interactive ASK gate enforcement
        │
[ Level 2: Runtime Engine (harness --sandbox) ]
  └── Host LLM & Web Search connectivity retained; local filesystem mutations constrained
        │
[ Level 3: Precision Tool Isolation (bwrap --unshare-all) ]
  └── enforce_sandbox = true: Private mount/PID/IPC/network namespaces, loopback only, zero raw sockets
        │
[ Level 4: Path & Manifest Guard (manifest_guard & ContentFilter) ]
  └── Cargo.toml / .env / SSH keys protected; inner-file boundary filters (mod tests only)
```

---

## 11. Operational Protocols & Fleet Governance

The operational boundaries defined in [Section 3: Agent Fleet Perimeters & Blast Boundaries](#3-agent-fleet-perimeters--blast-boundaries) are enforced through declarative runtime protocols:

### 11.1 The Dependency Request Protocol
When an implementing agent (`coder` or `test-author`) requires a third-party dependency:
1. Direct editing of `Cargo.toml`, `Cargo.lock`, or `deny.toml` is intercepted and blocked by `manifest_guard`.
2. The agent asks the human operator directly with the package name, requested version, features, and justification.
3. Once the operator approves, the dependency can be added to the manifest.

### 11.2 Jujutsu Workspace Isolation & Clean State Invariants
- Each agent operates in its own sparse Jujutsu working copy at `.tkg/workspaces/<agent>/`.
- Agent modifications remain cleanly isolated from the operator's active `@` commit and open editor buffers.
- Merging and rebasing workspace revisions is governed by `copilot` and the `SubagentAdmissionGate` without mutating active work-in-progress.
