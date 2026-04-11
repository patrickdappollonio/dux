#!/usr/bin/env bash
set -euo pipefail

REPO="patrickdappollonio/dux"
BINARY="dux"

# Allow overriding the version and install directory via environment variables.
VERSION="${DUX_VERSION:-}"
INSTALL_DIR="${DUX_INSTALL_DIR:-}"

log() { printf '%s\n' "$@"; }
err() { log "$@" >&2; exit 1; }

detect_os() {
    local os
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    case "$os" in
        linux)  echo "linux" ;;
        darwin) echo "darwin" ;;
        *)      err "Unsupported operating system: $os" ;;
    esac
}

detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64)       echo "amd64" ;;
        aarch64|arm64)      echo "arm64" ;;
        *)                  err "Unsupported architecture: $arch" ;;
    esac
}

has_cmd() { command -v "$1" >/dev/null 2>&1; }

http_get() {
    local url="$1"
    if has_cmd curl; then
        curl -sSfL "$url"
    elif has_cmd wget; then
        wget -qO- "$url"
    else
        err "Either curl or wget is required to download files."
    fi
}

http_download() {
    local url="$1" dest="$2"
    if has_cmd curl; then
        curl -sSfL -o "$dest" "$url"
    elif has_cmd wget; then
        wget -qO "$dest" "$url"
    else
        err "Either curl or wget is required to download files."
    fi
}

resolve_version() {
    if [ -n "$VERSION" ]; then
        # Ensure the version starts with 'v'.
        case "$VERSION" in
            v*) echo "$VERSION" ;;
            *)  echo "v$VERSION" ;;
        esac
        return
    fi

    log "Fetching latest release version..."
    local response
    response="$(http_get "https://api.github.com/repos/${REPO}/releases/latest")" \
        || err "Failed to fetch latest release from GitHub API. Set DUX_VERSION to install a specific version."

    # Parse the tag_name from the JSON response without requiring jq.
    local tag
    tag="$(echo "$response" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"
    [ -n "$tag" ] || err "Could not determine latest release version. Set DUX_VERSION to install a specific version."
    echo "$tag"
}

resolve_install_dir() {
    # 1. Explicit override.
    if [ -n "$INSTALL_DIR" ]; then
        echo "$INSTALL_DIR"
        return
    fi

    # 2. ~/.local/bin if it exists and is in PATH.
    local local_bin="$HOME/.local/bin"
    if [ -d "$local_bin" ]; then
        case ":$PATH:" in
            *":$local_bin:"*) echo "$local_bin"; return ;;
        esac
    fi

    # 3. Traditional fallback.
    echo "/usr/local/bin"
}

main() {
    local os arch version install_dir archive url tmpdir

    os="$(detect_os)"
    arch="$(detect_arch)"
    version="$(resolve_version)"
    install_dir="$(resolve_install_dir)"
    archive="${BINARY}-${os}-${arch}.tar.gz"
    url="https://github.com/${REPO}/releases/download/${version}/${archive}"

    log "Installing ${BINARY} ${version} (${os}/${arch}) to ${install_dir}"

    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT

    log "Downloading ${url}..."
    http_download "$url" "${tmpdir}/${archive}"

    tar xzf "${tmpdir}/${archive}" -C "$tmpdir"

    # Install the binary — use sudo only if the target directory is not writable.
    if [ -w "$install_dir" ]; then
        install -m 755 "${tmpdir}/${BINARY}" "${install_dir}/${BINARY}"
    else
        log "Installation directory ${install_dir} is not writable, using sudo..."
        sudo install -m 755 "${tmpdir}/${BINARY}" "${install_dir}/${BINARY}"
    fi

    log ""
    log "${BINARY} ${version} has been installed to ${install_dir}/${BINARY}"

    if ! has_cmd "$BINARY"; then
        log ""
        log "Warning: ${install_dir} is not in your PATH."
        log "Add it to your shell profile:"
        log "  export PATH=\"${install_dir}:\$PATH\""
    fi
}

main
