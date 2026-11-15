# Performance & Benchmarks

`TorqKage` is engineered for low-overhead native interception, deterministic parsing/filtering throughput, and lean binary footprints across `tkg-hooks`, `tkg-mcp`, and `tkg`.

---

## 1. CLI & Lifecycle Hook Latency (`hyperfine`)

Measured on **Linux (`x86_64`, Intel Core i5-10300H @ 2.50 GHz)** against `cargo build --release --workspace` binaries (`warmup = 10`):

| Invocation | Mode / Work Performed | Mean ± σ | Min … Max | Peak RSS |
| :--- | :--- | :--- | :--- | :--- |
| `tkg-hooks help` | Cold process spawn & dispatch (`-N`, no shell) | **709.3 ± 147.4 µs** | 574.5 … 2188.8 µs | 4.6 MiB |
| `tkg --status` | Cascaded project load + 10-agent matrix (`-N`) | **4.49 ± 0.39 ms** | 4.07 … 7.87 ms | 8.0 MiB |
| `tkg-hooks pre-tool-use` | 6-stage `hooks.toml` gate pipeline + JSON parse (via `sh`) | **12.16 ± 0.84 ms** | 11.27 … 15.75 ms | 7.2 MiB |
| `tkg-hooks detect-wip` | Turn-by-turn VCS diff check spawning `jj`/`git` (via `sh`) | **14.97 ± 5.49 ms** | 11.76 … 40.71 ms | 40.7 MiB |

`tkg-hooks` intentionally compiles without `tokio`, `rusqlite`, `ratatui`, or `tracing-subscriber` (using a minimal `stderr` subscriber in `tkg-common::diagnostics`), keeping hook process startup sub-millisecond (**~0.71 ms**).

---

## 2. Core Hot-Path Microbenchmarks (`divan`)

Wall-clock latency and heap allocation counts from `cargo bench -p tkg-common --bench core_benches` (`samples = 100`, `iters = 100`, Intel Core i5-10300H):

| Benchmark (`tkg_common`) | Median | Mean | Fastest … Slowest | Allocs (Count / Bytes) |
| :--- | :--- | :--- | :--- | :--- |
| `bench_strip_delimited` | **1.78 µs** | 1.96 µs | 1.70 … 6.45 µs | 3 / 2.15 KB |
| `bench_strip_comments_rust` | **8.14 µs** | 8.58 µs | 7.63 … 22.46 µs | 11 / 1.91 KB |
| `bench_resolve_full_tools_toml` | **9.68 µs** | 11.33 µs | 9.24 … 65.45 µs | 51 / 35.73 KB |
| `bench_strip_tests_rust` | **12.14 µs** | 13.05 µs | 11.97 … 26.11 µs | 15 / 3.03 KB |
| `bench_frontmatter_parse_and_serialize` | **21.99 µs** | 25.92 µs | 20.11 … 64.96 µs | 246 / 75.53 KB |
| `bench_path_glob_and_security_guards` | **29.35 µs** | 32.93 µs | 27.22 … 215.5 µs | 228 / 20.89 KB |

---

## 3. Deterministic Instruction & Cache Metrics (`gungraun` / Valgrind Callgrind)

CPU-independent instruction and cache simulation from `cargo bench -p tkg-common --bench gungraun_bench`:

| Benchmark (`callgrind`) | Instructions | L1 Hits | LL Hits | RAM Hits | Estimated Cycles |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `callgrind_strip_delimited` | **24,541** | 29,258 | 7 | 112 | 33,213 |
| `callgrind_strip_comments` | **104,791** | 134,305 | 6 | 137 | 139,130 |
| `callgrind_strip_tests` | **151,505** | 194,954 | 17 | 257 | 204,034 |
| `callgrind_frontmatter_parse` | **180,892** | 250,442 | 235 | 1,085 | 289,592 |

*(Note: `gungraun_bench` requires `valgrind` + `gungraun-runner` on Linux and is excluded on macOS/Windows cross-platform CI runs.)*

---

## 4. Binary Footprints & Compilation Profiles

Measured workspace binaries (`target/release/` unstripped vs `target/min-size/` stripped fat-LTO `opt-level = "z"`):

| Binary | Role | `release` Size | `min-size` (stripped) | Notes |
| :--- | :--- | :--- | :--- | :--- |
| **`tkg`** | Interactive Ratatui control deck & launcher | 7.7 MiB | **3.7 MiB** | Includes Ratatui 0.30 + SQLite session lookup |
| **`tkg-hooks`** | Lifecycle hook interceptor CLI | 8.1 MiB | **4.0 MiB** | Synchronous CLI without Tokio, SQLite, or TUI |
| **`tkg-mcp`** | JSON-RPC 2.0 MCP daemon | 12 MiB | **5.0 MiB** | Async Tokio runtime + process supervision |

Cargo profiles defined in `Cargo.toml` and driven via `justfile`:

| Profile | Recipe | Settings | Intended Use |
| :--- | :--- | :--- | :--- |
| **`fast`** | `just fast` / `just ft` | `opt-level = 0`, `debug = 0`, `codegen-units = 256`, Cranelift + Mold | Sub-second incremental edit/check/test loops |
| **`release`** | `cargo build --release` | `opt-level = 3`, standard release | Local release testing and benchmarking |
| **`perf`** | `just perf` | `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, stripped | Maximum runtime throughput |
| **`min-size`** | `just min-size <bin>` | `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, stripped | Minimal binary distribution footprint |

---

## 5. Reproducing Benchmarks & Hardware Profiling

```bash
# 1. Divan latency & heap allocation microbenchmarks
cargo bench -p tkg-common --bench core_benches

# 2. Deterministic Callgrind instruction counts (Linux with Valgrind)
cargo bench -p tkg-common --bench gungraun_bench

# 3. CLI startup latency (requires hyperfine + release binaries)
cargo build --release --workspace
hyperfine --warmup 10 -N './target/release/tkg-hooks help' './target/release/tkg --status'
```

For deeper profiling, `perf-analyst` workflows also support `llvm-mca -mcpu=native`, `cargo bloat --release -p tkg-hooks --crates` (`just size-hooks`), `cargo-asm`, and `cargo-flamegraph`.
