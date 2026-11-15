# Quickstart

Get started with `TorqKage` in under 60 seconds.

## Prerequisites

- **Operating System:**
  - **Linux** (`x86_64` or `aarch64`, glibc or musl): optional `bubblewrap` (`bwrap`) enables OS container sandboxing.
  - **macOS / Windows:** Cross-platform CI-tested binaries run with **In-Band MCP Seatbelt** containment when `bwrap` is unavailable.
- **Rust Toolchain:** Rust 1.85+ (Rust 2024 Edition) when building from source.
- **Agent Execution Harness:** Any supported CLI harness installed on `$PATH` (Google Antigravity `agy`, Claude Code `claude`, OpenCode `opencode`, Cursor `cursor`, or a custom CLI runner).
- *(Optional for fast compilation & recipes):* `just`, `clang` / `mold`, `rustc-codegen-cranelift`.

## Installation

### Method 1: Automated POSIX Installer (One-Liner)

Works cleanly via `/bin/sh` on Linux (`x86_64`/`aarch64`, `gnu`/`musl`) and macOS (`Darwin`). Downloads precompiled release archives with `.sha256` checksum verification (falling back cleanly to Cargo source builds when needed) and scaffolds `~/.config/tkg/config.toml`:

```bash
curl -sSL https://raw.githubusercontent.com/JeronimoCapelle/torqkage/main/install.sh | sh
```

Pass `--harness <name>` (e.g., `agy`, `claude`, `opencode`, `cursor`, or a custom slug) and `--init-config` (non-interactive config scaffold):

```bash
curl -sSL https://raw.githubusercontent.com/JeronimoCapelle/torqkage/main/install.sh | sh -s -- --harness claude --init-config
```

### Method 2: From Crates.io

Install the `tkg`, `tkg-mcp`, and `tkg-hooks` binaries directly from crates.io:

```bash
cargo install torqkage tkg-mcp tkg-hooks
```

### Method 3: Build From Source

Clone the repository and compile from source using `--build-from-source` (`--perf` for LTO release or `--fast` for Cranelift + Mold):

```bash
git clone https://github.com/JeronimoCapelle/torqkage.git
cd torqkage
./install.sh --build-from-source --perf
```

The installer will:
1. Download or compile `tkg`, `tkg-mcp`, and `tkg-hooks`.
2. Install binaries into `~/.local/bin/` and sync the harness bundle directory (for example `~/.gemini/config/plugins/torqkage`, `~/.claude/plugins/torqkage`, `~/.local/share/opencode/plugins/torqkage`, or `~/.cursor/plugins/torqkage`).
3. Scaffold `~/.config/tkg/config.toml` (when `--init-config` / interactive confirmation is chosen).
4. Run harness bundle validation when supported.

Or drive development tasks directly with `just`:

```bash
just fast          # Cranelift + Mold quick compile check
just ft            # Cranelift + Mold nextest test loop
just fix           # Fixed-point auto-fix loop (rustc + clippy --fix + fmt + tests)
just verify        # Full CI quality gate locally (fmt, clippy, nextest, doc, deny, machete, typos)
```

## Running the Flight Deck

Launch `tkg` inside any Git or Jujutsu repository:

```bash
tkg
```

### Key Controls

| Key | Action |
| :--- | :--- |
| <kbd>j</kbd> / <kbd>k</kbd> | Navigate agent list, tool catalog, or scroll content |
| <kbd>h</kbd> / <kbd>l</kbd> | Toggle focus between Agent List (left) and Tool List (right) |
| <kbd>Space</kbd> | Toggle tool authorization for selected agent |
| <kbd>Tab</kbd> / <kbd>BackTab</kbd> | Switch between Fleet Matrix, Tool Catalog, and Directives |
| <kbd>Enter</kbd> / <kbd>r</kbd> | Launch or resume session with selected agent |
| <kbd>n</kbd> | Force launch fresh, unresumed session |
| <kbd>b</kbd> / <kbd>F8</kbd> | Toggle Bubblewrap OS container isolation for launched sessions |
| <kbd>w</kbd> | Toggle Jujutsu isolated worktree (`.tkg/workspaces/<agent>`) |
| <kbd>p</kbd> | Cycle agent permission policy (Autonomous / Supervised / Plan) |
| <kbd>c</kbd> / <kbd>Shift+K</kbd> | Reset compiler failure circuit breaker strike counter |
| <kbd>g</kbd> | Toggle tool permission gate (`ALLOW` vs `ASK`) |
| <kbd>F2</kbd> / <kbd>S</kbd> | Atomic Save & Pre-flight Validation |
| <kbd>F4</kbd> | Atomic Rollback unsaved changes to disk/VCS state |
| <kbd>i</kbd> | Run in-deck plugin installer and diagnostics |
| <kbd>?</kbd> / <kbd>F1</kbd> | Open in-deck manual and controls reference |
| <kbd>q</kbd> | Exit control deck |
