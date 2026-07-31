#!/usr/bin/env bash
set -euo pipefail

REPO="patrickdappollonio/dux"
BINARY="dux"

# Allow overriding the version and install directory via environment variables.
VERSION="${DUX_VERSION:-}"
INSTALL_DIR="${DUX_INSTALL_DIR:-}"
DUX_TMPDIR=""

log() { printf '%s\n' "$@" >&2; }
err() { log "$@"; exit 1; }
cleanup() {
    [ -n "$DUX_TMPDIR" ] && rm -rf "$DUX_TMPDIR"
}

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

# Detail about the last http_fetch_optional call, for the caller's message.
HTTP_FETCH_DETAIL=""

# Fetch a file that is allowed to be absent, telling "the server says it is not
# there" apart from "the fetch never happened".
#
# http_download cannot make that distinction: it collapses a 404, a DNS failure,
# a refused connection, a TLS error and a proxy error into one non-zero exit. The
# caller then has to guess, and guessing produced a message that asserted a
# specific cause ("this release predates checksums") on the strength of a local
# network error. Three outcomes instead, so the caller can say something true:
#
#   0 -> downloaded into $dest
#   1 -> the server answered, and answered that the file is not there (404/410)
#   2 -> the fetch itself failed, so nothing is known about whether it exists
#
# $dest is removed unless the outcome is 0, so a caller can still treat a
# zero-byte leftover as "absent" without depending on this.
http_fetch_optional() {
    local url="$1" dest="$2"
    local errfile="${dest}.fetch-error"
    HTTP_FETCH_DETAIL=""
    rm -f "$dest" "$errfile"

    if has_cmd curl; then
        # No -f here: --fail makes curl exit 22 for every HTTP error, which is the
        # very conflation being removed. Ask for the status code instead.
        local code status=0
        code="$(curl -sSL --max-time 60 -o "$dest" -w '%{http_code}' "$url" 2>"$errfile")" \
            || status=$?
        if [ "$status" -ne 0 ]; then
            HTTP_FETCH_DETAIL="curl exit ${status}: $(tr '\n' ' ' <"$errfile" 2>/dev/null)"
            rm -f "$dest" "$errfile"
            return 2
        fi
        rm -f "$errfile"
        case "$code" in
            2??)     return 0 ;;
            404|410) rm -f "$dest"; HTTP_FETCH_DETAIL="the server answered HTTP ${code}"; return 1 ;;
            *)       rm -f "$dest"; HTTP_FETCH_DETAIL="the server answered HTTP ${code}"; return 2 ;;
        esac
    elif has_cmd wget; then
        # -nv rather than -q: quiet still suppresses the ERROR text, which is the
        # only thing that explains a transport failure (a refused connection
        # produces no server response for -S to report). -nv keeps the message and
        # drops the progress bar.
        local status=0
        wget -nv -S -O "$dest" "$url" 2>"$errfile" || status=$?
        if [ "$status" -eq 0 ]; then
            rm -f "$errfile"
            return 0
        fi
        # wget exits 8 for any error RESPONSE, so the status alone cannot separate
        # a 404 from a 500. -S puts the response lines on stderr; read the last.
        local code
        code="$(grep -oE 'HTTP/[0-9.]+ [0-9]{3}' "$errfile" 2>/dev/null | tail -1 | sed 's/.* //')"
        if [ -n "$code" ]; then
            HTTP_FETCH_DETAIL="the server answered HTTP ${code}"
        else
            HTTP_FETCH_DETAIL="wget exit ${status}: $(tr '\n' ' ' <"$errfile" 2>/dev/null)"
        fi
        rm -f "$dest" "$errfile"
        case "$code" in
            404|410) return 1 ;;
            *)       return 2 ;;
        esac
    else
        HTTP_FETCH_DETAIL="neither curl nor wget is installed"
        return 2
    fi
}

# Print the SHA-256 of a file as lowercase hex, or return 1 when this machine
# has no way to compute one. Linux ships sha256sum (coreutils); macOS ships
# shasum instead; a minimal container may well have neither, which is why the
# failure is a return value rather than a fatal error.
sha256_of() {
    local file="$1"
    if has_cmd sha256sum; then
        sha256sum "$file" | cut -d' ' -f1
    elif has_cmd shasum; then
        shasum -a 256 "$file" | cut -d' ' -f1
    else
        return 1
    fi
}

# Check a downloaded archive against its published checksum.
#
# What this genuinely buys, stated plainly: it detects a CORRUPTED or TRUNCATED
# download, and it publishes a value you can verify by hand out of band. It is
# NOT protection against a tampered release. Anyone able to replace an archive on
# the release page can replace the checksum file sitting beside it just as
# easily, because both come from the same place over the same channel. Only
# signed artifacts would defend against that, and dux does not sign releases yet.
#
# Outcomes:
#   * checksum present and matching  -> return 0, install proceeds
#   * checksum present and different -> exit 1, nothing is installed
#   * checksum could not be FETCHED  -> warn that the fetch failed, return 1,
#     and the caller proceeds anyway
#   * checksum genuinely absent, or no hashing tool on this machine -> warn
#     loudly and return 1, and the caller proceeds anyway
#
# The fourth argument is the fetch outcome from http_fetch_optional (0 fetched,
# 1 absent, 2 fetch failed); it defaults to 0 so a caller holding a local file
# can pass three arguments. Keeping "the fetch failed" separate from "there is no
# checksum" matters twice over: a message that blames the release for a local DNS
# failure sends the user to the wrong place, and mandatory checksums are
# impossible while the two look identical.
#
# The permissive branches are deliberate and TEMPORARY. Releases published before
# checksums existed carry no .sha256 file, and the install-script CI installs the
# real latest release, so treating a missing checksum as fatal today would break
# both installing older versions and that CI run. Once every supported release
# carries a checksum, make it mandatory by replacing the `return 1` in the "no
# published checksum" branch with a call to `err`. The fetch-failure branch
# should become an `err` at the same time (an unverifiable download is
# unverifiable whatever the reason) but it is a SEPARATE decision with a separate
# message, which is the point of splitting them. The "no hashing tool" branch
# should probably stay a warning even then, since that is a property of the
# user's machine and not of the release.
verify_checksum() {
    local archive="$1" checksum_file="$2" label="$3" fetch_status="${4:-0}"

    if [ "$fetch_status" -eq 2 ]; then
        log ""
        log "WARNING: the checksum for ${label} could not be fetched."
        log "         ${HTTP_FETCH_DETAIL:-the request failed}"
        log "         This is a problem reaching the server (DNS, connectivity, TLS or"
        log "         a proxy), NOT a release that was published without a checksum."
        log "         The download was NOT verified. Re-run the install once the"
        log "         connection works, or check the archive by hand against the"
        log "         checksum on the release page."
        log ""
        return 1
    fi

    if [ ! -s "$checksum_file" ]; then
        log ""
        log "WARNING: no published checksum for ${label}."
        log "         The download was NOT verified. Releases published before dux"
        log "         started emitting checksums do not have one."
        log ""
        return 1
    fi

    # The published file is one line in sha256sum's own format: the hex digest,
    # two spaces, the archive name. Take the first field only.
    local expected
    expected="$(head -1 "$checksum_file" | cut -d' ' -f1 | tr '[:upper:]' '[:lower:]')"

    if [[ ! "$expected" =~ ^[0-9a-f]{64}$ ]]; then
        err "Checksum file for ${label} is malformed (expected 64 hex characters, got '${expected}')." \
            "Refusing to install an archive that cannot be verified against it."
    fi

    local actual
    if ! actual="$(sha256_of "$archive")"; then
        log ""
        log "WARNING: neither sha256sum nor shasum is available on this machine."
        log "         A checksum was published for ${label} but could not be checked."
        log "         Install coreutils (sha256sum) or perl (shasum) to enable verification."
        log ""
        return 1
    fi

    if [ "$actual" != "$expected" ]; then
        err "Checksum mismatch for ${label}." \
            "  expected: ${expected}" \
            "  actual:   ${actual}" \
            "The download is corrupt or does not match what the release publishes." \
            "Nothing has been installed. Try again, and if it keeps failing, report it."
    fi

    log "Checksum verified for ${label} (sha256 ${actual})."
    return 0
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
    local os arch version install_dir archive url checksum_file checksum_status

    os="$(detect_os)"
    arch="$(detect_arch)"
    version="$(resolve_version)"
    install_dir="$(resolve_install_dir)"
    archive="${BINARY}-${os}-${arch}.tar.gz"
    url="https://github.com/${REPO}/releases/download/${version}/${archive}"

    log "Installing ${BINARY} ${version} (${os}/${arch}) to ${install_dir}"

    DUX_TMPDIR="$(mktemp -d)"
    trap cleanup EXIT

    log "Downloading ${url}..."
    http_download "$url" "${DUX_TMPDIR}/${archive}"

    # Fetching the checksum is best effort: releases from before dux published
    # them answer this URL with a 404, and that must not abort the install. It
    # goes through http_fetch_optional rather than http_download so that a 404
    # and a failed request stay distinguishable, and the warning can name the
    # cause it actually observed.
    checksum_file="${DUX_TMPDIR}/${archive}.sha256"
    checksum_status=0
    http_fetch_optional "${url}.sha256" "$checksum_file" || checksum_status=$?

    # A mismatch exits from inside here without installing anything. A missing or
    # unfetchable checksum warns and returns non-zero, which is not a failure of
    # the install.
    verify_checksum "${DUX_TMPDIR}/${archive}" "$checksum_file" "$archive" "$checksum_status" || true

    tar xzf "${DUX_TMPDIR}/${archive}" -C "$DUX_TMPDIR"

    # Install the binary — use sudo only if the target directory is not writable.
    if [ -w "$install_dir" ]; then
        install -m 755 "${DUX_TMPDIR}/${BINARY}" "${install_dir}/${BINARY}"
    else
        log "Installation directory ${install_dir} is not writable, using sudo..."
        sudo install -m 755 "${DUX_TMPDIR}/${BINARY}" "${install_dir}/${BINARY}"
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

# Setting DUX_INSTALL_SH_LIB=1 defines the functions above without installing
# anything, so the checksum logic can be driven directly by
# .github/scripts/test_install_checksum.sh with no network and no release.
# Nothing outside that test should set it.
if [ "${DUX_INSTALL_SH_LIB:-}" != "1" ]; then
    main
fi
