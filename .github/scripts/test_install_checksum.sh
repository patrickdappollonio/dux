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
# Section 8 additionally drives the REAL http_fetch_optional against a local
# HTTP fixture. Every other case here either hands verify_checksum a fetch
# outcome by hand or stubs http_fetch_optional out entirely, so the function that
# DECIDES that outcome, the one whole reason the installer can tell "this release
# has no checksum" from "the network is broken", never executed under test at
# all. It is correct today; nothing would have said when it stopped being.
#
# What this does NOT cover: the real GitHub release endpoint, and the workflow
# steps that produce the .sha256 files. Neither can run here.
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
FIXTURE_PID=""
cleanup() {
  if [ -n "$FIXTURE_PID" ] && kill -0 "$FIXTURE_PID" 2>/dev/null; then
    kill "$FIXTURE_PID" 2>/dev/null || true
    wait "$FIXTURE_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

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

# 3c. A checksum that could not be FETCHED is a different outcome from one that
#     was never published, and must not be reported as the latter. Every failure
#     mode used to collapse into "this release predates checksums", which is an
#     assertion about the RELEASE made on the strength of a local network error:
#     wrong, and it points the user away from the actual cause. It also blocks the
#     documented path to making checksums mandatory, since a transient DNS blip
#     would be indistinguishable from a legacy release.
#
#     verify_checksum takes the fetch outcome as its fourth argument:
#       0 = fetched, 1 = the server said it is not there, 2 = the fetch failed.
run_verify_status() {
  local archive="$1" sums="$2" fetch_status="$3"
  (
    set +e
    # shellcheck disable=SC1090
    DUX_INSTALL_SH_LIB=1 source "$INSTALL_SH"
    verify_checksum "$archive" "$sums" "dux-test.tar.gz" "$fetch_status"
  ) 2>&1
}

fetchfail_out="$(run_verify_status "$ARCHIVE" "$WORK/does-not-exist.sha256" 2)" \
  && fetchfail_status=0 || fetchfail_status=$?
printf -- '--- checksum fetch failed (transport) ---\n%s\n(exit status %s)\n' \
  "$fetchfail_out" "$fetchfail_status"
if [ "$fetchfail_status" = "0" ]; then
  fail "a failed checksum fetch must not report success"
elif [ "${fetchfail_out#*"could not be fetched"}" = "$fetchfail_out" ]; then
  fail "a failed checksum fetch must say the fetch failed, got: ${fetchfail_out}"
elif [ "${fetchfail_out#*"before dux"}" != "$fetchfail_out" ]; then
  fail "a failed checksum fetch must NOT blame the release for predating checksums"
else
  pass "a failed checksum fetch is reported as a fetch failure, not a missing checksum"
fi

# 3d. And the genuine absence still reads as an absence, so the two messages are
#     really distinct rather than one message rewritten.
absent_out="$(run_verify_status "$ARCHIVE" "$WORK/does-not-exist.sha256" 1)" || true
printf -- '--- checksum genuinely absent (404) ---\n%s\n' "$absent_out"
if [ "${absent_out#*"no published checksum"}" != "$absent_out" ] \
  && [ "${absent_out#*"could not be fetched"}" = "$absent_out" ]; then
  pass "a genuine absence still reads as an absence"
else
  fail "a genuine absence must keep the 'no published checksum' wording"
fi

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
#    http_download and http_fetch_optional are the two seams. Overriding both to
#    copy from a local "release directory" exercises the real main(): the same URL
#    construction, the same best-effort .sha256 fetch (a missing file here reports
#    outcome 1, exactly as a real 404 does), the same verify_checksum call, the
#    same tar and install steps.
#
#    BOTH have to be stubbed. Stubbing only http_download leaves the checksum
#    fetch reaching the real network, which quietly turned the mismatch case below
#    into a no-checksum case that installed the binary anyway.
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
    # Absent here means the server said 404 (outcome 1), not a failed request.
    http_fetch_optional() {
      local name="${1##*/}"
      HTTP_FETCH_DETAIL=""
      rm -f "$2"
      if [ ! -f "$WORK/release/$name" ]; then
        HTTP_FETCH_DETAIL="the server answered HTTP 404"
        return 1
      fi
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

  # 7d. The checksum fetch fails at the transport layer (no DNS, refused
  #     connection, TLS error, proxy error). The archive itself downloaded fine,
  #     so the install still proceeds, but the warning has to name the real cause
  #     instead of asserting something false about the release.
  #
  #     http_fetch_optional is the seam main() uses for the checksum, separate
  #     from http_download precisely so the caller can tell these apart; here it
  #     reports outcome 2 (the fetch did not happen).
  run_main_fetchfail() {
    local install_dir="$1"
    (
      set +e
      export DUX_VERSION="v9.9.9"
      export DUX_INSTALL_DIR="$install_dir"
      DUX_INSTALL_SH_LIB=1 source "$INSTALL_SH"
      http_download() {
        local name="${1##*/}"
        [ -f "$WORK/release/$name" ] || return 1
        cp "$WORK/release/$name" "$2"
      }
      http_fetch_optional() {
        HTTP_FETCH_DETAIL="simulated: could not resolve host github.com"
        return 2
      }
      main
    ) 2>&1
  }

  mkdir -p "$WORK/bin-fetchfail"
  out="$(run_main_fetchfail "$WORK/bin-fetchfail")" || true
  printf -- '--- end to end, checksum fetch failed ---\n%s\n' "$out"
  if [ ! -x "$WORK/bin-fetchfail/dux" ]; then
    fail "end to end fetch failure: the archive downloaded, so the install should proceed"
  elif [ "${out#*"could not be fetched"}" = "$out" ]; then
    fail "end to end fetch failure: expected a warning naming the failed fetch"
  elif [ "${out#*"before dux"}" != "$out" ]; then
    fail "end to end fetch failure: must not claim the release predates checksums"
  else
    pass "end to end fetch failure: warned about the fetch, installed anyway"
  fi
else
  echo "SKIPPED: ${os_now}/${arch_now} is not a platform install.sh supports," >&2
  echo "         so the end-to-end cases cannot run here." >&2
fi

# 8. The REAL http_fetch_optional, against a local HTTP fixture.
#
# This is the function every other case above stubs out, and it is the one that
# decides which of the three outcomes the caller sees. Its whole job is to keep
# "the server says this file is not there" apart from "the fetch never happened",
# so an installer never again asserts that a release predates checksums on the
# strength of a DNS failure. Getting that wrong is silent: the install still
# succeeds, only the explanation is a lie.
#
# The fixture is python3's own http.server, already a CI-runner dependency for
# smoke_archive.sh, bound to 127.0.0.1 on a kernel-assigned port. Nothing here
# leaves the loopback interface. Both implementations are exercised, curl and
# wget, by hiding the other from has_cmd, which is the single place install.sh
# asks.
if ! command -v python3 >/dev/null 2>&1; then
  echo "SKIPPED: no python3, so the local HTTP fixture cannot run and" >&2
  echo "         http_fetch_optional stays uncovered on this machine." >&2
else
  cat > "$WORK/fixture.py" <<'PY'
import http.server, socketserver, sys, threading

BODY = b"0000000000000000000000000000000000000000000000000000000000000000  dux-test.tar.gz\n"

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/ok.sha256":
            self.send_response(200)
            self.send_header("Content-Length", str(len(BODY)))
            self.end_headers()
            self.wfile.write(BODY)
        elif self.path == "/gone.sha256":
            self.send_error(410)
        elif self.path == "/boom.sha256":
            self.send_error(500)
        else:
            self.send_error(404)

    def log_message(self, *args):
        pass

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
    with open(sys.argv[1], "w") as f:
        f.write(str(httpd.server_address[1]))
    httpd.serve_forever()
PY
  python3 "$WORK/fixture.py" "$WORK/fixture.port" &
  FIXTURE_PID=$!

  fixture_ready=""
  for _ in $(seq 1 50); do
    if [ -s "$WORK/fixture.port" ]; then
      fixture_ready="yes"
      break
    fi
    sleep 0.1
  done

  if [ -z "$fixture_ready" ]; then
    fail "the local HTTP fixture never reported a port"
  else
    FIXTURE_PORT="$(cat "$WORK/fixture.port")"
    BASE="http://127.0.0.1:${FIXTURE_PORT}"
    # A port nothing listens on, for the transport-failure case. Bound by the
    # fixture's own kernel-assigned port plus one is not safe; use a port the
    # fixture is definitely not on and that nothing else here binds.
    DEAD="http://127.0.0.1:1"

    # Run the real http_fetch_optional with exactly one download tool visible.
    # Prints "STATUS=<n> DETAIL=<text> SIZE=<bytes|absent>" so one assertion can
    # read the outcome, the message and whether $dest survived.
    run_fetch() {
      local tool="$1" url="$2" dest="$WORK/fetched.out"
      (
        # shellcheck disable=SC1090
        DUX_INSTALL_SH_LIB=1 source "$INSTALL_SH"
        # AFTER the source, not before: install.sh's own `set -euo pipefail` runs
        # when it is sourced, so an outcome of 1 or 2 (which is the entire point
        # of this function) would otherwise kill the subshell before it reports.
        set +e
        has_cmd() {
          case "$1" in
            curl|wget) [ "$1" = "$tool" ] ;;
            *) command -v "$1" >/dev/null 2>&1 ;;
          esac
        }
        status=0
        http_fetch_optional "$url" "$dest" || status=$?
        if [ -f "$dest" ]; then
          size="$(wc -c <"$dest" | tr -d ' ')"
        else
          size="absent"
        fi
        printf 'STATUS=%s DETAIL=%s SIZE=%s\n' "$status" "$HTTP_FETCH_DETAIL" "$size"
      ) 2>/dev/null
    }

    # want_status, want_size ("absent" or "content"), and a fragment the detail
    # must contain ("" for no requirement).
    expect_fetch() {
      local name="$1" tool="$2" url="$3" want_status="$4" want_size="$5" want_detail="$6"
      local out status size
      out="$(run_fetch "$tool" "$url")"
      status="${out#STATUS=}"; status="${status%% *}"
      size="${out##*SIZE=}"
      printf -- '--- %s (%s) ---\n%s\n' "$name" "$tool" "$out"
      if [ "$status" != "$want_status" ]; then
        fail "${name} (${tool}): expected outcome ${want_status}, got ${status}"
        return
      fi
      if [ "$want_size" = "absent" ] && [ "$size" != "absent" ]; then
        fail "${name} (${tool}): \$dest must be removed unless the fetch succeeded, got ${size} bytes"
        return
      fi
      if [ "$want_size" = "content" ] && { [ "$size" = "absent" ] || [ "$size" -eq 0 ]; }; then
        fail "${name} (${tool}): a successful fetch must leave the body in \$dest, got ${size}"
        return
      fi
      if [ -n "$want_detail" ] && [ "${out#*"$want_detail"}" = "$out" ]; then
        fail "${name} (${tool}): expected the detail to mention '${want_detail}', got: ${out}"
        return
      fi
      pass "${name} (${tool})"
    }

    for tool in curl wget; do
      if ! command -v "$tool" >/dev/null 2>&1; then
        echo "SKIPPED: no ${tool} on this machine, so its branch of" >&2
        echo "         http_fetch_optional is not exercised." >&2
        continue
      fi
      # 200: the file is there, and the body lands in $dest.
      expect_fetch "a published checksum downloads" "$tool" "$BASE/ok.sha256" 0 content ""
      # 404 and 410: the server ANSWERED, and answered that it is not there. This
      # is the only outcome that may be reported as "no published checksum".
      expect_fetch "a 404 is a definite absence" "$tool" "$BASE/nope.sha256" 1 absent "404"
      expect_fetch "a 410 is a definite absence" "$tool" "$BASE/gone.sha256" 1 absent "410"
      # 500: the server answered, but said nothing about existence. Reporting this
      # as an absence is the false claim the three outcomes exist to prevent.
      expect_fetch "a 500 is not an absence" "$tool" "$BASE/boom.sha256" 2 absent "500"
      # No server at all: a transport failure, and the detail has to carry
      # something the user can act on rather than an empty string.
      expect_fetch "a refused connection is a fetch failure" "$tool" "$DEAD/ok.sha256" 2 absent "$tool"
    done
  fi
fi

echo
if [ "$failures" -ne 0 ]; then
  echo "${failures} check(s) failed." >&2
  exit 1
fi
echo "All checksum verification checks passed."
