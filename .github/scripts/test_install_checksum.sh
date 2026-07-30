#!/usr/bin/env bash
#
# Drive install.sh's checksum verification directly, with no network and no
# release.
#
# install.sh only calls main() when DUX_INSTALL_SH_LIB is unset, so sourcing it
# with that variable set gives us verify_checksum and sha256_of as plain
# functions over local files. Each case below builds a real tar.gz, computes a
# real digest, and asserts on the exit status and the message.
#
# The cases mirror the three outcomes the installer has to get right:
#
#   1. correct checksum   -> exit 0, and it says it verified
#   2. wrong checksum     -> non-zero exit and the word "mismatch"; the caller
#                            never reaches its install step
#   3. missing checksum   -> non-zero exit, a visible WARNING, and the caller
#                            proceeds anyway
#
# Plus two more that are easy to get wrong: a malformed checksum file is a hard
# failure rather than a shrug, and a machine with neither sha256sum nor shasum
# warns instead of dying.
#
# What this does NOT cover: the download of the .sha256 from a real release, and
# the workflow steps that produce it. Neither can run here.
#
# Usage: .github/scripts/test_install_checksum.sh [path/to/install.sh]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_SH="${1:-${SCRIPT_DIR}/../../install.sh}"

if [ ! -f "$INSTALL_SH" ]; then
  echo "FAIL: cannot find install.sh at $INSTALL_SH" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

failures=0
pass() { printf 'PASS: %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; failures=$((failures + 1)); }

# Build a real archive, shaped like a release archive (a dux binary at the root).
mkdir -p "$WORK/payload"
printf '#!/bin/sh\necho dux\n' > "$WORK/payload/dux"
chmod 755 "$WORK/payload/dux"
tar czf "$WORK/dux-test.tar.gz" -C "$WORK/payload" dux

ARCHIVE="$WORK/dux-test.tar.gz"

if command -v sha256sum >/dev/null 2>&1; then
  REAL_SUM="$(sha256sum "$ARCHIVE" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then
  REAL_SUM="$(shasum -a 256 "$ARCHIVE" | cut -d' ' -f1)"
else
  echo "SKIPPED: this machine has neither sha256sum nor shasum, so the test" >&2
  echo "         harness cannot compute an expected value to assert against." >&2
  exit 0
fi

# Run verify_checksum in a subshell so an `err` (which exits) is contained.
# Prints combined output; returns the function's exit status.
run_verify() {
  local archive="$1" sums="$2"
  (
    set +e
    # shellcheck disable=SC1090
    DUX_INSTALL_SH_LIB=1 source "$INSTALL_SH"
    verify_checksum "$archive" "$sums" "dux-test.tar.gz"
  ) 2>&1
}

expect() {
  local name="$1" want_status="$2" want_text="$3" got_status=0 out
  out="$(run_verify "$4" "$5")" || got_status=$?

  printf -- '--- %s ---\n' "$name"
  printf '%s\n' "$out"
  printf -- '(exit status %s)\n' "$got_status"

  if [ "$got_status" != "$want_status" ]; then
    fail "$name: expected exit status ${want_status}, got ${got_status}"
    return
  fi
  case "$out" in
    *"$want_text"*) ;;
    *) fail "$name: expected output to contain '${want_text}'"; return ;;
  esac
  pass "$name"
}

# 1. Correct checksum passes.
printf '%s  dux-test.tar.gz\n' "$REAL_SUM" > "$WORK/good.sha256"
expect "correct checksum verifies" 0 "Checksum verified" "$ARCHIVE" "$WORK/good.sha256"

# 2. Wrong checksum is a hard failure. Same length and character class as a real
#    digest, so this is a mismatch and not a malformed-file rejection.
printf '%s  dux-test.tar.gz\n' \
  "0000000000000000000000000000000000000000000000000000000000000000" \
  > "$WORK/bad.sha256"
expect "wrong checksum fails hard" 1 "Checksum mismatch" "$ARCHIVE" "$WORK/bad.sha256"

# 2b. And the caller really does install nothing. main() reaches its install step
#     only after verify_checksum returns, and a mismatch exits the whole shell
#     rather than returning, so prove the statement after the call is unreached.
sentinel_out="$(
  (
    set +e
    DUX_INSTALL_SH_LIB=1 source "$INSTALL_SH"
    verify_checksum "$ARCHIVE" "$WORK/bad.sha256" "dux-test.tar.gz" || true
    echo "REACHED-INSTALL-STEP"
  ) 2>&1
)" || true
case "$sentinel_out" in
  *REACHED-INSTALL-STEP*)
    fail "a mismatch let execution continue to the install step"
    ;;
  *)
    pass "a mismatch stops before the install step, even behind '|| true'"
    ;;
esac

# 3. A missing checksum warns and lets the caller proceed.
expect "missing checksum warns" 1 "WARNING: no published checksum" \
  "$ARCHIVE" "$WORK/does-not-exist.sha256"

# 3b. An empty file is treated exactly like a missing one, because curl and wget
#     can both leave a zero-byte file behind on a 404.
: > "$WORK/empty.sha256"
expect "empty checksum file warns" 1 "WARNING: no published checksum" \
  "$ARCHIVE" "$WORK/empty.sha256"

# 4. A malformed checksum file is a hard failure, not a warning: the release is
#    broken and guessing is worse than stopping.
printf 'not-a-digest  dux-test.tar.gz\n' > "$WORK/malformed.sha256"
expect "malformed checksum file fails hard" 1 "malformed" \
  "$ARCHIVE" "$WORK/malformed.sha256"

# 5. With neither hashing tool on PATH, a present checksum warns rather than
#    failing: that is the user's machine, not a bad download. Simulated by
#    overriding has_cmd, which is the single place install.sh asks.
noimpl_out="$(
  (
    set +e
    DUX_INSTALL_SH_LIB=1 source "$INSTALL_SH"
    has_cmd() { case "$1" in sha256sum|shasum) return 1 ;; *) command -v "$1" >/dev/null 2>&1 ;; esac; }
    verify_checksum "$ARCHIVE" "$WORK/good.sha256" "dux-test.tar.gz"
    echo "STATUS=$?"
  ) 2>&1
)" || true
printf -- '--- no hashing tool available ---\n%s\n' "$noimpl_out"
case "$noimpl_out" in
  *"WARNING: neither sha256sum nor shasum"*)
    pass "no hashing tool warns instead of failing"
    ;;
  *)
    fail "no hashing tool did not produce the expected warning"
    ;;
esac

# 5b. The macOS branch. macOS has no sha256sum, so hide it and confirm the
#     shasum fallback produces the same digest and still verifies. Skipped where
#     shasum is absent rather than silently passing.
if command -v shasum >/dev/null 2>&1; then
  shasum_out="$(
    (
      set +e
      DUX_INSTALL_SH_LIB=1 source "$INSTALL_SH"
      has_cmd() { case "$1" in sha256sum) return 1 ;; *) command -v "$1" >/dev/null 2>&1 ;; esac; }
      verify_checksum "$ARCHIVE" "$WORK/good.sha256" "dux-test.tar.gz"
    ) 2>&1
  )" && shasum_status=0 || shasum_status=$?
  printf -- '--- shasum fallback (sha256sum hidden) ---\n%s\n(exit status %s)\n' \
    "$shasum_out" "$shasum_status"
  if [ "$shasum_status" -eq 0 ] && [ "${shasum_out#*"$REAL_SUM"}" != "$shasum_out" ]; then
    pass "shasum fallback verifies with the same digest"
  else
    fail "shasum fallback did not verify cleanly"
  fi
else
  echo "SKIPPED: no shasum on this machine, macOS fallback branch not exercised." >&2
fi

# 6. The digest install.sh computes agrees with the harness's own.
own_sum="$(
  (
    DUX_INSTALL_SH_LIB=1 source "$INSTALL_SH"
    sha256_of "$ARCHIVE"
  )
)"
if [ "$own_sum" = "$REAL_SUM" ]; then
  pass "sha256_of agrees with the reference digest"
else
  fail "sha256_of returned ${own_sum}, expected ${REAL_SUM}"
fi

# 7. End to end through main(), with the network stubbed out. This is what the
#    unit cases above cannot show: that a mismatch leaves the install directory
#    EMPTY, and that a match actually puts a binary there.
#
#    http_download is the single seam. Overriding it to copy from a local "release
#    directory" exercises the real main(): the same URL construction, the same
#    best-effort .sha256 fetch (a missing file here 404s exactly like a real one),
#    the same verify_checksum call, the same tar and install steps.
mkdir -p "$WORK/release"
cp "$ARCHIVE" "$WORK/release/dux-linux-amd64.tar.gz"
cp "$ARCHIVE" "$WORK/release/dux-linux-arm64.tar.gz"
cp "$ARCHIVE" "$WORK/release/dux-darwin-amd64.tar.gz"
cp "$ARCHIVE" "$WORK/release/dux-darwin-arm64.tar.gz"

run_main_offline() {
  local install_dir="$1"
  (
    set +e
    export DUX_VERSION="v9.9.9"
    export DUX_INSTALL_DIR="$install_dir"
    DUX_INSTALL_SH_LIB=1 source "$INSTALL_SH"
    # Serve from the local release directory instead of GitHub. Returns non-zero
    # for anything absent, which is how a real 404 reaches the caller.
    http_download() {
      local name="${1##*/}"
      [ -f "$WORK/release/$name" ] || return 1
      cp "$WORK/release/$name" "$2"
    }
    main
  ) 2>&1
}

os_now="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch_now="$(uname -m)"
case "$os_now" in linux|darwin) supported_os=1 ;; *) supported_os=0 ;; esac
case "$arch_now" in x86_64|amd64|aarch64|arm64) supported_arch=1 ;; *) supported_arch=0 ;; esac

if [ "$supported_os" -eq 1 ] && [ "$supported_arch" -eq 1 ]; then
  # 7a. Wrong checksum: nothing lands in the install directory.
  for f in "$WORK/release"/*.tar.gz; do
    printf '%s  %s\n' \
      "0000000000000000000000000000000000000000000000000000000000000000" \
      "${f##*/}" > "${f}.sha256"
  done
  mkdir -p "$WORK/bin-mismatch"
  out="$(run_main_offline "$WORK/bin-mismatch")" || true
  printf -- '--- end to end, wrong checksum ---\n%s\n' "$out"
  if [ -e "$WORK/bin-mismatch/dux" ]; then
    fail "end to end mismatch: a binary was installed anyway"
  elif [ -z "$(ls -A "$WORK/bin-mismatch")" ]; then
    pass "end to end mismatch: install directory left empty"
  else
    fail "end to end mismatch: install directory is not empty"
  fi

  # 7b. Correct checksum: the binary lands.
  for f in "$WORK/release"/*.tar.gz; do
    if command -v sha256sum >/dev/null 2>&1; then
      (cd "$WORK/release" && sha256sum "${f##*/}" > "${f##*/}.sha256")
    else
      (cd "$WORK/release" && shasum -a 256 "${f##*/}" > "${f##*/}.sha256")
    fi
  done
  mkdir -p "$WORK/bin-ok"
  out="$(run_main_offline "$WORK/bin-ok")" || true
  printf -- '--- end to end, correct checksum ---\n%s\n' "$out"
  if [ -x "$WORK/bin-ok/dux" ]; then
    pass "end to end match: binary installed"
  else
    fail "end to end match: no binary at $WORK/bin-ok/dux"
  fi

  # 7c. No checksum published at all: warns, and still installs.
  rm -f "$WORK/release"/*.sha256
  mkdir -p "$WORK/bin-nosum"
  out="$(run_main_offline "$WORK/bin-nosum")" || true
  printf -- '--- end to end, no checksum published ---\n%s\n' "$out"
  if [ -x "$WORK/bin-nosum/dux" ] && [ "${out#*"WARNING: no published checksum"}" != "$out" ]; then
    pass "end to end no checksum: warned, and installed anyway"
  else
    fail "end to end no checksum: expected a warning and an installed binary"
  fi
else
  echo "SKIPPED: ${os_now}/${arch_now} is not a platform install.sh supports," >&2
  echo "         so the end-to-end cases cannot run here." >&2
fi

echo
if [ "$failures" -ne 0 ]; then
  echo "${failures} check(s) failed." >&2
  exit 1
fi
echo "All checksum verification checks passed."
