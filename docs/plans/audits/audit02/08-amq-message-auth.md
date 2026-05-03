# Phase 08: AMQ message authentication (HMAC-signed envelope)

> Maps to: **P0-K** (no auth between AMQ peers; T2 in threat model).

## Goal
Prevent one compromised dux pane from spoofing `--me <other>` to inject
arbitrary text into a sibling pane via `amq wake … --inject-mode raw`.
Add a per-VM HMAC secret; sign every outgoing message; verify on receive;
drop unsigned/wrong-MAC messages.

This phase has TWO tracks: an **interim wrapper-side envelope** that we
control end-to-end, and an **upstream issue/PR** to land native auth in
`avivsinai/agent-message-queue`. Land the wrapper-side first; track
upstream with an issue link.

## Pre-conditions
- Phase 00 baseline green.
- Phase 02 (install.sh idempotency) merged — uses the same secret-init
  pattern.
- Independent of Phases 03–07.

## Files to touch
- `dux-amq/scripts/amq-secret-init.sh` — NEW.
- `dux-amq/scripts/amq-send-signed` — NEW (wrapper around `amq send`).
- `dux-amq/scripts/amq-receive-verify` — NEW (filter that verifies and
  re-emits clean payloads, used by `amq wake --inject-via`).
- `dux-amq/wrappers/{claude,codex,gemini}-amq` — switch from
  `--inject-mode raw` to `--inject-via amq-receive-verify`.
- `dux-amq/install.sh` — call `amq-secret-init.sh`; install the helpers.
- `dux-amq/tests/amq-auth.bats` — NEW.
- Upstream: file an issue at `avivsinai/agent-message-queue` linking
  this phase as the proposed protocol.

## Threat model recap
- Attacker = a single compromised pane (e.g. via prompt-injected web
  fetch + YOLO). Can read/write `/data/state/amq/`.
- Without auth: attacker writes a file into `/data/state/amq/<victim>/inbox/`
  and the victim's `amq wake --inject-mode raw` types it into Claude.
- With per-VM HMAC: attacker can still write the file, but the
  signature will fail (attacker doesn't have the secret IF we keep it
  out of agent-readable paths — hard, since pane processes themselves
  must sign). **Compromise model**: this phase is best-effort against
  prompt-injection-induced shell command execution; it does NOT
  protect against an attacker who has full shell ACL. Document.

## Steps

### 8.1 — Generate the per-VM secret
`dux-amq/scripts/amq-secret-init.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail
SECRET_PATH="${AMQ_SECRET_PATH:-$HOME/.local/share/dux-amq/amq-secret}"
mkdir -p "$(dirname "$SECRET_PATH")"
if [[ ! -f "$SECRET_PATH" ]]; then
  # 256 bits; base64 for portability.
  head -c 32 /dev/urandom | base64 -w0 > "$SECRET_PATH"
  chmod 0600 "$SECRET_PATH"
  printf 'amq-secret-init: wrote %s (mode 0600)\n' "$SECRET_PATH" >&2
fi
```
Called from `install.sh` after AMQ binary is in place.

### 8.2 — Sign on send
`dux-amq/scripts/amq-send-signed`:
```bash
#!/usr/bin/env bash
# Usage: amq-send-signed --to <peer> --me <self> --body <text>
#   Adds an HMAC-SHA256 envelope so receivers can verify origin.
set -euo pipefail

TO=""; ME=""; BODY=""
while (( $# )); do
  case "$1" in
    --to)   TO="$2"; shift 2;;
    --me)   ME="$2"; shift 2;;
    --body) BODY="$2"; shift 2;;
    *) printf 'usage: amq-send-signed --to PEER --me SELF --body TEXT\n' >&2; exit 2;;
  esac
done
[[ -n "$TO" && -n "$ME" && -n "$BODY" ]] || { echo "missing args" >&2; exit 2; }

SECRET_PATH="${AMQ_SECRET_PATH:-$HOME/.local/share/dux-amq/amq-secret}"
[[ -r "$SECRET_PATH" ]] || { echo "amq secret not readable: $SECRET_PATH" >&2; exit 1; }
SECRET=$(cat "$SECRET_PATH")

NONCE=$(head -c 12 /dev/urandom | base64 -w0)
TS=$(date -u +%FT%TZ)
PAYLOAD="$ME|$TO|$TS|$NONCE|$BODY"
MAC=$(printf '%s' "$PAYLOAD" | openssl dgst -sha256 -hmac "$SECRET" -binary | base64 -w0)

# Envelope format: literal "DUX1" + tab-separated fields.
ENVELOPE="DUX1	$ME	$TO	$TS	$NONCE	$MAC	$BODY"

amq send --to "$TO" --me "$ME" "$ENVELOPE"
```

### 8.3 — Verify on receive (used by `--inject-via`)
`dux-amq/scripts/amq-receive-verify`:
```bash
#!/usr/bin/env bash
# Reads one AMQ message body on stdin; if valid, writes the clean
# payload to /dev/tty (so amq wake's TTY-reattach path picks it up).
# Drops invalid messages with a stderr warning.
#
# Replay protection: nonce file under $XDG_RUNTIME_DIR/dux-amq/seen-nonces;
# rejects nonces seen in the last 24 h.
set -euo pipefail

SECRET_PATH="${AMQ_SECRET_PATH:-$HOME/.local/share/dux-amq/amq-secret}"
[[ -r "$SECRET_PATH" ]] || { echo "[amq-verify] secret missing" >&2; exit 1; }
SECRET=$(cat "$SECRET_PATH")
NONCES="${XDG_RUNTIME_DIR:-/tmp}/dux-amq/seen-nonces"
mkdir -p "$(dirname "$NONCES")"; touch "$NONCES"

read -r line
IFS=$'\t' read -r MAGIC SENDER RECEIVER TS NONCE MAC BODY <<<"$line"
if [[ "$MAGIC" != "DUX1" ]]; then
  printf '[amq-verify] dropping unsigned message from %s\n' "${SENDER:-?}" >&2
  exit 0
fi

# Replay window: 60 s skew tolerance, 24 h nonce dedup.
NOW=$(date -u +%s); MSG_TS=$(date -u -d "$TS" +%s 2>/dev/null || echo 0)
if (( NOW - MSG_TS < -60 || NOW - MSG_TS > 86400 )); then
  printf '[amq-verify] dropping stale/future message ts=%s\n' "$TS" >&2
  exit 0
fi
if grep -qx "$NONCE" "$NONCES"; then
  printf '[amq-verify] replay rejected nonce=%s\n' "$NONCE" >&2
  exit 0
fi

PAYLOAD="$SENDER|$RECEIVER|$TS|$NONCE|$BODY"
EXPECT=$(printf '%s' "$PAYLOAD" | openssl dgst -sha256 -hmac "$SECRET" -binary | base64 -w0)
if [[ "$MAC" != "$EXPECT" ]]; then
  printf '[amq-verify] HMAC mismatch from %s\n' "$SENDER" >&2
  exit 0
fi

# Persist nonce; trim file weekly to bound size.
printf '%s\n' "$NONCE" >> "$NONCES"

# Emit the clean body for amq's --inject-via stdout-to-tty pipeline.
printf '%s\n' "$BODY"
```

Make both scripts `chmod 0755` and install to `$LOCAL_BIN/`.

### 8.4 — Wire wrappers
In each wrapper, replace:
```bash
amq wake --me "$ME" --root "$ROOT" --inject-mode raw </dev/tty >/dev/null 2>&1 &
```
with:
```bash
# Audit02 P0-K: route incoming messages through amq-receive-verify so
# unsigned / replay / wrong-MAC messages are dropped before injection.
amq wake --me "$ME" --root "$ROOT" --inject-via amq-receive-verify \
  </dev/tty 2>"$HOME/.local/share/dux-amq/wake-$ME.log" &
```

Confirm the AMQ binary's `--inject-via` actually pipes the message to
the executable's stdin and re-injects from stdout (audit01 phase 07
research notes this is the documented behavior; verify against the
pinned AMQ version's `--help`).

### 8.5 — Update peer messaging across the codebase
Audit existing `amq send` call sites in our own scripts/skills and
replace with `amq-send-signed`. The AMQ-skills npm package may invoke
`amq send` directly — file an upstream issue to support our envelope
or, in the meantime, configure it to use our wrapper.

### 8.6 — Tests
`dux-amq/tests/amq-auth.bats`:
```bash
@test "amq-receive-verify drops unsigned messages" {
  echo "plain text not signed" | bin/amq-receive-verify
  # exit 0, but stderr should mention "dropping unsigned"
}
@test "amq-receive-verify accepts well-signed message" {
  ./bin/amq-secret-init.sh
  msg=$(./bin/amq-send-signed --me alice --to bob --body "hello" --print-only)
  echo "$msg" | bin/amq-receive-verify
  # stdout should be exactly "hello"
}
@test "amq-receive-verify rejects replay" {
  msg=$(./bin/amq-send-signed --me alice --to bob --body "hi" --print-only)
  echo "$msg" | bin/amq-receive-verify  # first ok
  echo "$msg" | bin/amq-receive-verify  # second drops replay
}
@test "amq-receive-verify rejects MAC mismatch" {
  msg=$(./bin/amq-send-signed --me alice --to bob --body "hi" --print-only)
  bad=${msg/hi/EVIL}
  echo "$bad" | bin/amq-receive-verify
  # stderr "HMAC mismatch", stdout empty
}
```
Add `--print-only` flag to `amq-send-signed` for tests so the envelope
is printed instead of sent.

### 8.7 — Upstream issue
File `avivsinai/agent-message-queue#NEW`: "Native HMAC envelope for
inter-agent auth", linking this phase as the proposed protocol. Track
in audit02 artifacts as `08-upstream-issue.txt`.

## Validation
- `make overlay-test` green; bats tests pass.
- Manual: with two panes on a test VM, `amq send --me alice "$victim"
  evil`. The victim should see "[amq-verify] dropping unsigned" in
  `wake-<me>.log` and Claude should NOT see "evil".
- `cat ~/.local/share/dux-amq/amq-secret` is mode 0600.

## Acceptance criteria
- [ ] `amq-secret-init.sh` writes 32-byte base64 secret, mode 0600.
- [ ] `amq-send-signed` builds DUX1 envelope with HMAC-SHA256.
- [ ] `amq-receive-verify` validates magic, freshness (60 s skew, 24 h
      window), nonce dedup, MAC.
- [ ] Wrappers use `--inject-via amq-receive-verify`.
- [ ] `wake-<me>.log` captures stderr (no `>/dev/null`).
- [ ] 4 bats tests pass.
- [ ] Upstream issue filed; URL recorded in `artifacts/08-upstream-issue.txt`.
- [ ] PR: `feat(amq): HMAC envelope auth + replay protection (P0-K, T2)`.

## Known pitfalls
- `openssl dgst -hmac` syntax differs between OpenSSL 1.x and 3.x. Test
  on Ubuntu 24.04's default (3.0) AND on macOS LibreSSL.
- `XDG_RUNTIME_DIR` may not exist on macOS — fall back to `/tmp` (the
  script does). On macOS `/tmp` survives reboot? No — `/tmp` is cleared.
  Acceptable for nonce dedup.
- AMQ's `--inject-via` may pass message metadata via env vars rather
  than stdin — verify against the pinned binary's `--help` BEFORE
  finalizing the script. If different, adjust.
- Replay window of 24 h is generous; tighten to 1 h for sensitive
  deployments via env override.
- This phase **does not** protect against an attacker with arbitrary
  shell execution as the dux user (they can read the secret). It DOES
  protect against an attacker with only filesystem-write access to
  `/data/state/amq/` — narrower threat, but real (e.g. privilege
  separation between the LLM tool sandbox and the wrapper, if any).

## References
- audit02 P0-K, T2.
- AMQ wake source `internal/cli/wake_*.go` (verify --inject-via).
- HMAC-SHA256 RFC 2104.
- Replay-protection nonce + timestamp pattern (e.g. AWS SigV4).
