#!/bin/sh
# ==============================================================================
# TorqKage Installer & Bundler (POSIX /bin/sh)
#
# Installs precompiled binaries (tkg, tkg-mcp, tkg-hooks) into ~/.local/bin/
# and synchronizes the TorqKage harness plugin bundle, with safe fallback to
# building from source when no precompiled release is available.
# ==============================================================================

set -eu

GITHUB_REPO="JeronimoCapelle/torqkage"
HARNESS="${TKG_HARNESS:-agy}"
PLUGIN_DIR_OVERRIDE=""
TARGET_PLUGIN_DIR=""
LOCAL_BIN_DIR="${HOME}/.local/bin"
XDG_CFG_HOME="${XDG_CONFIG_HOME:-${HOME}/.config}"
GLOBAL_CONFIG_PATH="${XDG_CFG_HOME}/tkg/config.toml"

case "$0" in
    /*) SCRIPT_DIR=$(dirname "$0") ;;
    */*) SCRIPT_DIR=$(cd "$(dirname "$0")" 2>/dev/null && pwd || pwd) ;;
    *) SCRIPT_DIR=$(pwd) ;;
esac

# Options
BUILD_SOURCE=false
FORCE_PRECOMPILED=false
PROFILE="perf"
LOCAL_ONLY=false
SKIP_BUNDLE=false
CONFIG_MODE="ask" # ask | yes | no
RELEASE_TAG="latest"
TMP_CLEANUP_DIR=""

cleanup() {
    if [ -n "${TMP_CLEANUP_DIR}" ] && [ -d "${TMP_CLEANUP_DIR}" ]; then
        rm -rf "${TMP_CLEANUP_DIR}"
    fi
}
trap cleanup EXIT INT TERM

print_usage() {
    cat <<EOF
Usage: install.sh [OPTIONS]

Options:
  --precompiled                  Install precompiled binary release from GitHub (default)
  --build-from-source, --source  Force compiling from source using Cargo
  --profile <perf|release|fast>  Compilation profile when building from source (default: perf)
  --perf                         Shortcut for '--profile perf'
  --fast                         Shortcut for '--profile fast' (uses cargo-fast if available)
  --version <tag>                Specific release tag to download (default: latest)
  --local-only                   Only stage into ./plugins/torqkage/bin/ without global install
  --no-bundle                    Install tkg, tkg-mcp, tkg-hooks into ~/.local/bin/ only
  --harness <name>               Target harness: agy (default), claude, codex, gemini, cursor,
                                 opencode, github-copilot, vscode, windsurf, roo, cline, custom
                                 (also settable via TKG_HARNESS)
  --plugin-dir <path>            Override the global plugin bundle install directory
  --init-config, -y, --yes       Scaffold ${GLOBAL_CONFIG_PATH} without prompting
  --no-config                    Skip scaffolding ${GLOBAL_CONFIG_PATH}
  -h, --help                     Display this help message
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --precompiled)
            FORCE_PRECOMPILED=true
            shift
            ;;
        --build-from-source|--source)
            BUILD_SOURCE=true
            shift
            ;;
        --profile)
            [ "$#" -ge 2 ] || { echo "Error: --profile requires an argument" >&2; exit 1; }
            PROFILE="$2"
            BUILD_SOURCE=true
            shift 2
            ;;
        --perf)
            PROFILE="perf"
            BUILD_SOURCE=true
            shift
            ;;
        --fast)
            PROFILE="fast"
            BUILD_SOURCE=true
            shift
            ;;
        --version)
            [ "$#" -ge 2 ] || { echo "Error: --version requires an argument" >&2; exit 1; }
            RELEASE_TAG="$2"
            shift 2
            ;;
        --local-only)
            LOCAL_ONLY=true
            shift
            ;;
        --no-bundle|--no-symlinks)
            SKIP_BUNDLE=true
            shift
            ;;
        --harness)
            [ "$#" -ge 2 ] || { echo "Error: --harness requires an argument" >&2; exit 1; }
            HARNESS="$2"
            shift 2
            ;;
        --plugin-dir)
            [ "$#" -ge 2 ] || { echo "Error: --plugin-dir requires an argument" >&2; exit 1; }
            PLUGIN_DIR_OVERRIDE="$2"
            shift 2
            ;;
        --init-config|-y|--yes)
            CONFIG_MODE="yes"
            shift
            ;;
        --no-config)
            CONFIG_MODE="no"
            shift
            ;;
        -h|--help)
            print_usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            print_usage >&2
            exit 1
            ;;
    esac
done

normalize_lower() {
    printf '%s' "$1" | tr '[:upper:]' '[:lower:]'
}

resolve_target_plugin_dir() {
    if [ -n "${PLUGIN_DIR_OVERRIDE}" ]; then
        TARGET_PLUGIN_DIR="${PLUGIN_DIR_OVERRIDE}"
        return
    fi
    slug=$(normalize_lower "${HARNESS}" | tr -cd 'a-z0-9_-')
    case "${slug}" in
        agy|antigravity)    TARGET_PLUGIN_DIR="${HOME}/.gemini/config/plugins/torqkage" ;;
        opencode)           TARGET_PLUGIN_DIR="${HOME}/.local/share/opencode/plugins/torqkage" ;;
        claude|claude-code) TARGET_PLUGIN_DIR="${HOME}/.claude/plugins/torqkage" ;;
        cursor|cursor-agent) TARGET_PLUGIN_DIR="${HOME}/.cursor/plugins/torqkage" ;;
        codex|openai-codex|codex-cli) TARGET_PLUGIN_DIR="${HOME}/.codex/plugins/torqkage" ;;
        gemini|gemini-cli)  TARGET_PLUGIN_DIR="${HOME}/.gemini/extensions/torqkage" ;;
        github-copilot|copilot-cli|gh-copilot|copilot|vscode|vs-code|vscode-copilot)
                            TARGET_PLUGIN_DIR="${HOME}/.copilot/plugins/torqkage" ;;
        windsurf|codeium)   TARGET_PLUGIN_DIR="${HOME}/.codeium/windsurf/plugins/torqkage" ;;
        roo|roo-code|roocode) TARGET_PLUGIN_DIR="${HOME}/.roo/plugins/torqkage" ;;
        cline|claude-dev)   TARGET_PLUGIN_DIR="${HOME}/.cline/plugins/torqkage" ;;
        ""|custom)          TARGET_PLUGIN_DIR="${HOME}/.config/tkg/plugins/torqkage" ;;
        *)                  TARGET_PLUGIN_DIR="${HOME}/.${slug}/plugins/torqkage" ;;
    esac
}
resolve_target_plugin_dir

is_agy_harness() {
    case "$(normalize_lower "${HARNESS}")" in
        agy|antigravity) return 0 ;;
        *) return 1 ;;
    esac
}

detect_os() {
    uname -s
}

detect_target() {
    arch_raw=$(uname -m)
    case "${arch_raw}" in
        x86_64|amd64)   target_arch="x86_64" ;;
        aarch64|arm64)  target_arch="aarch64" ;;
        *)
            echo "Error: Unsupported CPU architecture '${arch_raw}'." >&2
            return 1
            ;;
    esac

    os_raw=$(detect_os)
    case "${os_raw}" in
        Linux)
            libc="gnu"
            if command -v ldd >/dev/null 2>&1; then
                if ldd --version 2>&1 | grep -qi "musl"; then
                    libc="musl"
                fi
            else
                libc="musl"
            fi
            printf '%s\n' "${target_arch}-unknown-linux-${libc}"
            ;;
        Darwin)
            printf '%s\n' "${target_arch}-apple-darwin"
            ;;
        *)
            echo "Error: Unsupported operating system '${os_raw}' (expected Linux or Darwin)." >&2
            return 1
            ;;
    esac
}

warn_platform_capabilities() {
    if [ "$(detect_os)" = "Darwin" ]; then
        echo "==> [TorqKage] Notice: Bubblewrap ('bwrap') namespaces are Linux-only and unsupported on macOS."
        echo "--> TorqKage will run tools in In-Band MCP Seatbelt Mode (path confinement, dangerous-file"
        echo "    blocking, and circuit breakers enabled; container host fallback available via project policy)."
    fi
}

fetch_url() {
    url="$1"
    dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "${url}" -o "${dest}" 2>/dev/null
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "${dest}" "${url}" 2>/dev/null
    else
        echo "Error: Either 'curl' or 'wget' is required to download release archives." >&2
        return 1
    fi
}

verify_sha256() {
    archive_path="$1"
    checksum_path="$2"
    expected=$(awk 'NR==1 { print $1 }' "${checksum_path}")
    [ -n "${expected}" ] || return 0

    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "${archive_path}" | awk '{ print $1 }')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "${archive_path}" | awk '{ print $1 }')
    else
        echo "--> Warning: Neither 'sha256sum' nor 'shasum' found; skipping checksum verification." >&2
        return 0
    fi

    if [ "${actual}" != "${expected}" ]; then
        echo "Error: SHA-256 checksum verification failed for ${archive_path}!" >&2
        echo "  Expected: ${expected}" >&2
        echo "  Actual:   ${actual}" >&2
        return 1
    fi
    echo "--> Verified SHA-256 checksum."
}

record_harness_selection() {
    toml_file="${TARGET_PLUGIN_DIR}/project.toml"
    if [ -f "${toml_file}" ]; then
        if grep -q '^harness = ' "${toml_file}"; then
            tmp_toml="${toml_file}.tmp.$$"
            awk -v h="${HARNESS}" '/^harness = / { print "harness = \"" h "\""; next } { print }' "${toml_file}" > "${tmp_toml}"
            mv "${tmp_toml}" "${toml_file}"
        else
            printf '\n[runner]\nharness = "%s"\n' "${HARNESS}" >> "${toml_file}"
        fi
    fi
}

scaffold_global_config() {
    if [ "${CONFIG_MODE}" = "no" ]; then
        return 0
    fi
    if [ -f "${GLOBAL_CONFIG_PATH}" ]; then
        return 0
    fi

    should_create=false
    if [ "${CONFIG_MODE}" = "yes" ]; then
        should_create=true
    elif [ -t 0 ]; then
        printf '--> Scaffold default global config at %s? [y/N] ' "${GLOBAL_CONFIG_PATH}"
        read -r reply || reply=""
        case "$(normalize_lower "${reply}")" in
            y|yes) should_create=true ;;
            *) should_create=false ;;
        esac
    else
        echo "--> Tip: Run with --init-config (or copy config) to scaffold ${GLOBAL_CONFIG_PATH}."
        return 0
    fi

    if [ "${should_create}" = "true" ]; then
        mkdir -p "$(dirname "${GLOBAL_CONFIG_PATH}")"
        cat <<EOF > "${GLOBAL_CONFIG_PATH}"
# TorqKage global configuration (~/.config/tkg/config.toml)
# Loaded before workspace-local .tkg/project.toml overrides.

default_harness = "${HARNESS}"

[sandbox]
# Additional workspace-relative paths mounted read-write in Bubblewrap jails
bind_mounts = []
EOF
        echo "--> Created global configuration scaffold: ${GLOBAL_CONFIG_PATH}"
    fi
}

install_binaries_to_local_bin() {
    bin_source_dir="$1"
    echo "--> Installing TorqKage binaries into ${LOCAL_BIN_DIR}..."
    mkdir -p "${LOCAL_BIN_DIR}"
    for bin_name in tkg tkg-mcp tkg-hooks; do
        rm -f "${LOCAL_BIN_DIR}/${bin_name}"
        cp "${bin_source_dir}/${bin_name}" "${LOCAL_BIN_DIR}/${bin_name}"
        chmod 755 "${LOCAL_BIN_DIR}/${bin_name}"
    done
    case ":${PATH}:" in
        *":${LOCAL_BIN_DIR}:"*) ;;
        *) echo "--> Advisory: Add '${LOCAL_BIN_DIR}' to your PATH to invoke tkg commands directly." ;;
    esac
}

install_bundle_to_system() {
    plugin_manifest_dir="$1"
    bin_source_dir="$2"

    install_binaries_to_local_bin "${bin_source_dir}"

    if [ "${SKIP_BUNDLE}" = "false" ] && [ -n "${plugin_manifest_dir}" ] && [ -f "${plugin_manifest_dir}/plugin.json" ]; then
        echo "--> Synchronizing bundle to ${HARNESS} plugin root: ${TARGET_PLUGIN_DIR}..."
        if [ -L "${TARGET_PLUGIN_DIR}" ]; then
            rm -f "${TARGET_PLUGIN_DIR}"
        fi
        mkdir -p "${TARGET_PLUGIN_DIR}/bin"
        rm -rf "${TARGET_PLUGIN_DIR}/agents" && mkdir -p "${TARGET_PLUGIN_DIR}/agents"
        mkdir -p "${TARGET_PLUGIN_DIR}/assets"

        for f in plugin.json tools.toml hooks.toml hooks.json project.toml README.md; do
            if [ -f "${plugin_manifest_dir}/${f}" ]; then
                cp "${plugin_manifest_dir}/${f}" "${TARGET_PLUGIN_DIR}/"
            fi
        done
        rm -f "${TARGET_PLUGIN_DIR}/mcp_config.json"

        if [ -d "${plugin_manifest_dir}/agents" ]; then
            cp -r "${plugin_manifest_dir}/agents/"* "${TARGET_PLUGIN_DIR}/agents/" 2>/dev/null || true
        fi
        if [ -d "${plugin_manifest_dir}/assets" ]; then
            cp -r "${plugin_manifest_dir}/assets/"* "${TARGET_PLUGIN_DIR}/assets/" 2>/dev/null || true
        fi

        for bin_name in tkg tkg-mcp tkg-hooks; do
            rm -f "${TARGET_PLUGIN_DIR}/bin/${bin_name}"
            ln -sf "${LOCAL_BIN_DIR}/${bin_name}" "${TARGET_PLUGIN_DIR}/bin/${bin_name}"
        done

        record_harness_selection

        if is_agy_harness && command -v agy >/dev/null 2>&1; then
            echo "--> Validating installed plugin with 'agy plugin validate'..."
            agy plugin validate "${TARGET_PLUGIN_DIR}"
        elif ! is_agy_harness; then
            echo "--> Harness '${HARNESS}': skipping 'agy plugin validate'."
        fi
    fi

    scaffold_global_config

    echo "================================================================================"
    echo "                   TORQKAGE INSTALLATION COMPLETE                               "
    echo "================================================================================"
    echo "Binaries Installed:     ${LOCAL_BIN_DIR}/tkg"
    echo "                        ${LOCAL_BIN_DIR}/tkg-mcp"
    echo "                        ${LOCAL_BIN_DIR}/tkg-hooks"
    if [ "${SKIP_BUNDLE}" = "false" ] && [ -d "${TARGET_PLUGIN_DIR}" ]; then
    echo "Plugin Bundle Location: ${TARGET_PLUGIN_DIR}"
    fi
    echo "================================================================================"
}

try_precompiled_install() {
    target=$(detect_target) || return 1
    echo "==> [TorqKage] Checking precompiled release for '${target}' (tag: ${RELEASE_TAG})..."

    if [ "${RELEASE_TAG}" = "latest" ]; then
        base_url="https://github.com/${GITHUB_REPO}/releases/latest/download"
    else
        base_url="https://github.com/${GITHUB_REPO}/releases/download/${RELEASE_TAG}"
    fi
    download_url="${base_url}/torqkage-${target}.tar.gz"
    checksum_url="${base_url}/torqkage-${target}.sha256"
    legacy_checksum_url="${download_url}.sha256"

    TMP_CLEANUP_DIR=$(mktemp -d)
    tar_path="${TMP_CLEANUP_DIR}/torqkage.tar.gz"
    sha_path="${TMP_CLEANUP_DIR}/torqkage-${target}.sha256"

    echo "--> Downloading ${download_url}..."
    if ! fetch_url "${download_url}" "${tar_path}"; then
        echo "--> Notice: Precompiled release asset '${download_url}' not found."
        return 1
    fi

    if fetch_url "${checksum_url}" "${sha_path}" || fetch_url "${legacy_checksum_url}" "${sha_path}"; then
        verify_sha256 "${tar_path}" "${sha_path}" || return 1
    fi

    echo "--> Unpacking release archive..."
    tar -xzf "${tar_path}" -C "${TMP_CLEANUP_DIR}"

    bin_dir="${TMP_CLEANUP_DIR}"
    if [ -d "${TMP_CLEANUP_DIR}/torqkage-${target}" ]; then
        bin_dir="${TMP_CLEANUP_DIR}/torqkage-${target}"
    fi
    if [ -d "${bin_dir}/bin" ]; then
        bin_dir="${bin_dir}/bin"
    fi

    for b in tkg tkg-mcp tkg-hooks; do
        if [ ! -f "${bin_dir}/${b}" ]; then
            echo "--> Notice: Archive did not contain expected binary '${b}'."
            return 1
        fi
    done

    plugin_manifest_dir="${SCRIPT_DIR}/plugins/torqkage"
    if [ "${SKIP_BUNDLE}" = "false" ] && [ ! -f "${plugin_manifest_dir}/plugin.json" ]; then
        echo "--> Downloading plugin manifests from repository..."
        mkdir -p "${TMP_CLEANUP_DIR}/repo"
        if fetch_url "https://github.com/${GITHUB_REPO}/archive/refs/heads/main.tar.gz" "${TMP_CLEANUP_DIR}/main.tar.gz"; then
            tar -xzf "${TMP_CLEANUP_DIR}/main.tar.gz" -C "${TMP_CLEANUP_DIR}/repo" --strip-components=1
            plugin_manifest_dir="${TMP_CLEANUP_DIR}/repo/plugins/torqkage"
        fi
    fi

    install_bundle_to_system "${plugin_manifest_dir}" "${bin_dir}"
    return 0
}

build_from_source_install() {
    workspace_root="${SCRIPT_DIR}"

    if [ ! -f "${workspace_root}/Cargo.toml" ] || [ ! -d "${workspace_root}/plugins/torqkage" ]; then
        echo "==> [TorqKage] Remote source bootstrap requested (no local checkout detected)..."
        if ! command -v git >/dev/null 2>&1; then
            echo "Error: 'git' is required to shallow-clone TorqKage from source." >&2
            exit 1
        fi
        TMP_CLEANUP_DIR=$(mktemp -d)
        echo "--> Shallow cloning TorqKage into ${TMP_CLEANUP_DIR}/torqkage..."
        git clone --depth 1 "https://github.com/${GITHUB_REPO}.git" "${TMP_CLEANUP_DIR}/torqkage"
        workspace_root="${TMP_CLEANUP_DIR}/torqkage"
    fi

    if ! command -v cargo >/dev/null 2>&1; then
        echo "Error: 'cargo' is not installed and no precompiled release was retrieved." >&2
        echo "Install Rust via https://rustup.rs or retry with a published release tag." >&2
        exit 1
    fi

    plugin_dir="${workspace_root}/plugins/torqkage"
    echo "==> [TorqKage] Compiling binaries from source (profile: ${PROFILE})..."

    cd "${workspace_root}"
    case "${PROFILE}" in
        perf)
            cargo build --profile perf -p tkg-mcp -p tkg-hooks -p torqkage
            bin_source_dir="${workspace_root}/target/perf"
            ;;
        release)
            cargo build --release -p tkg-mcp -p tkg-hooks -p torqkage
            bin_source_dir="${workspace_root}/target/release"
            ;;
        fast)
            if command -v cargo-fast >/dev/null 2>&1; then
                cargo-fast build -p tkg-mcp -p tkg-hooks -p torqkage
            else
                cargo build --profile fast -p tkg-mcp -p tkg-hooks -p torqkage
            fi
            bin_source_dir="${workspace_root}/target/fast"
            ;;
        dev|debug)
            cargo build -p tkg-mcp -p tkg-hooks -p torqkage
            bin_source_dir="${workspace_root}/target/debug"
            ;;
        *)
            echo "Error: Unknown profile '${PROFILE}'. Use 'perf', 'release', 'fast', or 'dev'." >&2
            exit 1
            ;;
    esac

    for bin in tkg-mcp tkg-hooks tkg; do
        if [ ! -f "${bin_source_dir}/${bin}" ]; then
            echo "Error: Expected compiled binary '${bin_source_dir}/${bin}' not found!" >&2
            exit 1
        fi
    done

    mkdir -p "${plugin_dir}/bin"
    for bin in tkg-mcp tkg-hooks tkg; do
        rm -f "${plugin_dir}/bin/${bin}"
        cp "${bin_source_dir}/${bin}" "${plugin_dir}/bin/"
        chmod 755 "${plugin_dir}/bin/${bin}"
    done

    if [ "${LOCAL_ONLY}" = "true" ]; then
        if is_agy_harness && command -v agy >/dev/null 2>&1; then
            agy plugin validate "${plugin_dir}"
        fi
        echo "==> [TorqKage] Local workspace bundle staged in ${plugin_dir}/bin/."
        return 0
    fi

    install_bundle_to_system "${plugin_dir}" "${bin_source_dir}"
}

warn_platform_capabilities

if [ "${BUILD_SOURCE}" = "true" ]; then
    build_from_source_install
else
    if try_precompiled_install; then
        exit 0
    fi
    if [ "${FORCE_PRECOMPILED}" = "true" ]; then
        echo "Error: Precompiled installation was requested (--precompiled), but download failed." >&2
        exit 1
    fi
    echo "==> [TorqKage] Precompiled release unavailable; falling back to building from source..."
    build_from_source_install
fi
