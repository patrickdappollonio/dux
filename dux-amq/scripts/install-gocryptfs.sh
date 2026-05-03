#!/usr/bin/env bash
# OPTIONAL: encrypt /data/state with gocryptfs.
#
# This script is NOT called from dux-amq/install.sh's main flow. It is
# opt-in because adding mandatory filesystem-level encryption would
# change the deployment story for users who already trust their
# cloud-default at-rest encryption (GCE PD / EBS / Azure Disk).
#
# See: docs/operations/encryption-at-rest.md for the full playbook,
# threat model, and migration pattern. This script implements only the
# bootstrap (init + mount); it intentionally does NOT migrate existing
# plaintext data — that is a copy-and-swap operation the operator must
# perform deliberately, with verification.
#
# Usage:
#   export GOCRYPT_PASS_FILE=/run/credentials/gocrypt.pass
#   sudo -E /path/to/dux-amq/scripts/install-gocryptfs.sh
#
# The passfile MUST be sourced from a fetcher (Cloud Secret Manager,
# systemd-creds, Vault). Never commit a passphrase to git or write it
# to a regular on-disk file. See the playbook section on key
# management.
#
# Idempotent: re-running on a host that is already initialized and
# mounted is a no-op and exits zero.

set -euo pipefail

CIPHER_DIR="${GOCRYPT_CIPHER_DIR:-/data/state.crypt}"
CLEAR_DIR="${GOCRYPT_CLEAR_DIR:-/data/state}"
PASS_FILE="${GOCRYPT_PASS_FILE:-/run/credentials/gocrypt.pass}"

log() { printf '[install-gocryptfs] %s\n' "$*" >&2; }

# --- Pre-flight ------------------------------------------------------------
# `command -v` is the POSIX way; we keep `set -e` so a missing tool fails
# loud with a single-line install hint rather than a cryptic exit.
if ! command -v gocryptfs >/dev/null 2>&1; then
  log "gocryptfs binary not found on PATH."
  log "Install it: sudo apt-get install -y gocryptfs   (Debian/Ubuntu)"
  log "         or sudo dnf install -y gocryptfs       (Fedora/RHEL)"
  exit 1
fi

if ! command -v mountpoint >/dev/null 2>&1; then
  log "mountpoint(1) not found (util-linux). Cannot verify mount state."
  exit 1
fi

if [[ ! -r "$PASS_FILE" ]]; then
  log "Passphrase file not readable: $PASS_FILE"
  log "Set GOCRYPT_PASS_FILE to a path populated by your secret fetcher."
  log "See docs/operations/encryption-at-rest.md for key-management options."
  exit 1
fi

# Refuse to run with a world-readable passfile. The whole point of this
# layer is to keep the master key off the disk where any process can
# read it; a 0644 passfile defeats that. tmpfs / 0600 is the contract.
pass_perms=$(stat -c '%a' "$PASS_FILE" 2>/dev/null || stat -f '%Lp' "$PASS_FILE")
case "$pass_perms" in
  400|600|0400|0600) ;;
  *)
    log "Passphrase file has overly permissive mode $pass_perms — refusing."
    log "Run: chmod 0600 $PASS_FILE   (and confirm owner is the service user)"
    exit 1
    ;;
esac

# --- 1. Initialize cipher directory if absent ------------------------------
# `gocryptfs -init` is itself idempotent in the sense that it refuses to
# re-initialize a directory that already contains a gocryptfs.conf, but
# we still gate on directory existence so a fresh disk doesn't trigger
# a confusing "directory exists, refusing" error from gocryptfs.
if [[ ! -d "$CIPHER_DIR" ]]; then
  log "creating cipher dir $CIPHER_DIR and initializing gocryptfs"
  mkdir -p "$CIPHER_DIR"
  chmod 0700 "$CIPHER_DIR"
  gocryptfs -init -passfile "$PASS_FILE" "$CIPHER_DIR"
elif [[ ! -f "$CIPHER_DIR/gocryptfs.conf" ]]; then
  # Directory exists but isn't a gocryptfs cipher dir. Refuse — the
  # operator probably mis-set GOCRYPT_CIPHER_DIR and we don't want to
  # encrypt-init on top of arbitrary contents.
  log "$CIPHER_DIR exists but has no gocryptfs.conf — refusing to init"
  log "If this is intended, move the directory aside and re-run."
  exit 1
else
  log "cipher dir $CIPHER_DIR already initialized"
fi

# --- 2. Ensure clear-text mountpoint exists --------------------------------
mkdir -p "$CLEAR_DIR"

# --- 3. Mount if not already mounted ---------------------------------------
# `mountpoint -q` is the canonical idempotency check. If the mount is
# already live, we exit zero and report — re-running this script after
# reboot or recovery should be a safe operation.
if mountpoint -q "$CLEAR_DIR"; then
  log "$CLEAR_DIR is already a mountpoint — nothing to do"
  exit 0
fi

log "mounting $CIPHER_DIR -> $CLEAR_DIR"

# `-allow_other` lets non-root processes (the dux user, agent processes)
# read the mount. It requires `user_allow_other` in /etc/fuse.conf;
# gocryptfs will fail with a clear error if that's missing.
gocryptfs -passfile "$PASS_FILE" -allow_other "$CIPHER_DIR" "$CLEAR_DIR"

log "mounted. verify with: mount | grep '$CLEAR_DIR'"
log "see docs/operations/encryption-at-rest.md for migration + backup notes"
