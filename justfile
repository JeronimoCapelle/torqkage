# TorqKage workspace task runner
set shell := ["sh", "-eu", "-c"]

default:
    @just --list

# Fast compile check across all workspace targets
check:
    cargo check --workspace --all-targets --all-features

# Format code and run strict Clippy checks
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Auto-fix compiler/Clippy lints cleanly and format according to rustfmt.toml
fix:
    cargo fix --allow-dirty --allow-staged --all-targets --all-features
    cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features
    cargo fmt --all

# Run workspace unit, integration, and doc tests
test *args:
    cargo nextest run --workspace --all-features {{args}}
    cargo test --doc --workspace

# Run supply-chain, unused dependency, and spellcheck audits
audit:
    cargo deny check
    cargo machete
    typos

# Run full CI quality gate locally
verify: lint test audit
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# Run mutation testing focused on security guards and parsers
mutants *args:
    cargo mutants --config .cargo/mutants.toml {{args}}

# Inspect release binary sizes and largest crates inside tkg-hooks
size-hooks:
    cargo build --release -p tkg-hooks
    ls -lh target/release/tkg-hooks
    cargo bloat --release -p tkg-hooks --crates

# Clean stale target artifacts older than N days
sweep days="14":
    cargo sweep --time {{days}}
