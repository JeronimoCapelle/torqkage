# TorqKage workspace task runner
set shell := ["sh", "-eu", "-c"]

default:
    @just --list

# Fast compile check across all workspace targets (stable fallback)
check:
    cargo check --workspace --all-targets --all-features

# Max-speed Nightly + Cranelift + Mold dev loop (check, test, clippy, build)
[no-cd]
fast action="check" *args:
    #!/usr/bin/env sh
    set -eu
    export RUSTC_WRAPPER="" CRATE_CC_NO_DEFAULTS=1
    if command -v clang >/dev/null 2>&1; then
        export CC="clang" CXX="clang++" AR="llvm-ar" RANLIB="llvm-ranlib"
        export CFLAGS="-O0 -g0 -fPIC" CXXFLAGS="-O0 -g0 -fPIC"
    fi
    if command -v mold >/dev/null 2>&1; then
        export LDFLAGS="-fuse-ld=mold"
    fi
    if [ -f "$HOME/.cargo/presets/fast.toml" ]; then
        exec cargo +nightly {{action}} --workspace --profile fast --config "$HOME/.cargo/presets/fast.toml" {{args}}
    elif cargo +nightly --version >/dev/null 2>&1; then
        exec cargo +nightly {{action}} --workspace --profile fast --config .cargo/config.fast.toml {{args}}
    else
        exec cargo {{action}} --workspace --profile fast {{args}}
    fi

# Max-speed test loop via Cranelift + Mold (`fast` profile)
ft *args:
    @just fast nextest run {{args}}

# Max-throughput release build (build-std + Fat LTO + target-cpu=native)
[no-cd]
perf action="build" *args:
    #!/usr/bin/env sh
    set -eu
    if [ -f "$HOME/.cargo/presets/perf.toml" ] && cargo +nightly --version >/dev/null 2>&1; then
        TARGET="${CARGO_BUILD_TARGET:-$(rustc +nightly --print host-tuple)}"
        export CRATE_CC_NO_DEFAULTS=1
        export CC="clang" CXX="clang++" AR="llvm-ar" RANLIB="llvm-ranlib"
        export CFLAGS="-O3 -march=native -mtune=native -flto=thin -fomit-frame-pointer -fPIC -DNDEBUG"
        export CXXFLAGS="-O3 -march=native -mtune=native -flto=thin -fomit-frame-pointer -fPIC -DNDEBUG"
        export LDFLAGS="-fuse-ld=lld -Wl,--as-needed"
        exec cargo +nightly {{action}} --profile perf --config "$HOME/.cargo/presets/perf.toml" --target "$TARGET" {{args}}
    else
        exec cargo {{action}} --profile perf {{args}}
    fi

# Minimum binary footprint build (defaults to featherweight `tkg-hooks`)
[no-cd]
min-size bin="tkg-hooks" *args:
    #!/usr/bin/env sh
    set -eu
    if [ -f "$HOME/.cargo/presets/min-size.toml" ] && cargo +nightly --version >/dev/null 2>&1; then
        TARGET="${CARGO_BUILD_TARGET:-$(rustc +nightly --print host-tuple)}"
        export CRATE_CC_NO_DEFAULTS=1
        export CC="clang" CXX="clang++" AR="llvm-ar" RANLIB="llvm-ranlib"
        export CFLAGS="-Oz -flto=thin -ffunction-sections -fdata-sections -fno-unwind-tables -fno-asynchronous-unwind-tables -fomit-frame-pointer -fmerge-all-constants -fPIC -DNDEBUG"
        export CXXFLAGS="-Oz -flto=thin -ffunction-sections -fdata-sections -fno-unwind-tables -fno-asynchronous-unwind-tables -fomit-frame-pointer -fmerge-all-constants -fno-exceptions -fno-rtti -fPIC -DNDEBUG"
        export LDFLAGS="-fuse-ld=lld -Wl,--gc-sections,--icf=all,--strip-all,--as-needed,-z,noseparate-code"
        cargo +nightly build --profile min-size --config "$HOME/.cargo/presets/min-size.toml" --target "$TARGET" --bin {{bin}} {{args}}
        OUT="target/$TARGET/min-size/{{bin}}"
        if [ -f "$OUT" ] && command -v objcopy >/dev/null 2>&1; then
            objcopy --strip-all --remove-section=.comment --remove-section=.note* "$OUT"
        fi
        ls -lh "$OUT"
    else
        cargo build --profile min-size --bin {{bin}} {{args}}
        ls -lh "target/min-size/{{bin}}"
    fi

# Format check and strict Clippy gate across all targets and features
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# AI Coding Agent fixed-point auto-fix loop (rustc + clippy --fix <=5 passes + fmt + verify)
[no-cd]
fix:
    #!/usr/bin/env sh
    set -eu

    snapshot_rs() {
        find crates fuzz -name '*.rs' -type f -exec sha256sum {} + 2>/dev/null | sort | sha256sum
    }

    # 1. Apply machine-applicable rustc compiler fixes first
    cargo fix --allow-dirty --allow-staged --workspace --all-targets --all-features

    # 2. Fixed-point Clippy auto-fix loop (preserves repo lint contract, no global RUSTFLAGS clobbering)
    PASS=1
    while [ "$PASS" -le 5 ]; do
        BEFORE="$(snapshot_rs)"
        cargo clippy --fix --allow-dirty --allow-staged --workspace --all-targets --all-features \
            -- -W clippy::needless_raw_strings -W clippy::semicolon_outside_block -W clippy::try_err
        AFTER="$(snapshot_rs)"
        if [ "$BEFORE" = "$AFTER" ]; then
            echo "==> [fix] Reached fixed point after $PASS pass(es)."
            break
        fi
        PASS=$((PASS + 1))
    done

    # 3. Format cleanly according to rustfmt.toml
    cargo fmt --all

    # 4. Final strict verification
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo nextest run --workspace --all-features
    cargo test --doc --workspace

# Alias for `fix` matching ~/.config/just naming
agent-fix: fix

# Run workspace unit, integration, and doc tests
test *args:
    cargo nextest run --workspace --all-features {{args}}
    cargo test --doc --workspace

# Run LLVM AddressSanitizer + LeakSanitizer on native tests (including C FFI)
asan *args:
    #!/usr/bin/env sh
    set -eu
    TARGET="$(rustc +nightly --print host-tuple)"
    RUSTFLAGS="-Zsanitizer=address -Copt-level=1 -Cdebuginfo=line-tables-only -Clink-arg=-fuse-ld=lld" \
        cargo +nightly test -Zbuild-std --workspace --target "$TARGET" {{args}}

# Run LLVM ThreadSanitizer on native tests
tsan *args:
    #!/usr/bin/env sh
    set -eu
    TARGET="$(rustc +nightly --print host-tuple)"
    RUSTFLAGS="-Zsanitizer=thread -Copt-level=1 -Cdebuginfo=line-tables-only -Clink-arg=-fuse-ld=lld" \
        cargo +nightly test -Zbuild-std --workspace --target "$TARGET" {{args}}

# Run stdlib debug assertions & UB checks (cargo-careful)
careful *args:
    RUSTFLAGS="-Copt-level=1 -Cdebuginfo=line-tables-only -Clink-arg=-fuse-ld=lld" \
        cargo +nightly careful nextest run --workspace {{args}}

# Run supply-chain, unused dependency, and spellcheck audits
audit:
    cargo deny check
    cargo machete
    typos

# Run full CI quality gate locally
verify: lint test audit
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# Run mutation testing focused on security guards and parsers (uses Cranelift fast preset if available)
mutants *args:
    #!/usr/bin/env sh
    set -eu
    if [ -f "$HOME/.cargo/presets/fast.toml" ] && cargo +nightly --version >/dev/null 2>&1; then
        exec cargo +nightly mutants --config .cargo/mutants.toml --profile fast \
            --cargo-arg=--config --cargo-arg="$HOME/.cargo/presets/fast.toml" {{args}}
    else
        exec cargo mutants --config .cargo/mutants.toml {{args}}
    fi

# Inspect release binary sizes and largest crates inside tkg-hooks
size-hooks:
    cargo build --release -p tkg-hooks
    ls -lh target/release/tkg-hooks
    cargo bloat --release -p tkg-hooks --crates

# Clean stale target artifacts older than N days
sweep days="14":
    cargo sweep --time {{days}}
