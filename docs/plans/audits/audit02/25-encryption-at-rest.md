# Phase 25: Encryption-at-rest playbook (operator docs)

> Maps to: **T5** in threat model (audit02), audit01 **P2-4**.

## Goal
Document — and provide a one-shot install path for — application-layer
or filesystem-level encryption on top of cloud-provider KMS. The
default GCE/EBS at-rest encryption passes SOC2 baseline but does not
defend against a compromised cloud IAM principal who can attach the
disk elsewhere. For customer-data workloads we need a stronger story.

This phase ships **docs + an optional install script**. No mandatory
behavior change.

## Pre-conditions
- Phase 00 baseline green.
- Phase 24 (SECURITY.md) merged — links here.

## Files to touch
- `docs/operations/encryption-at-rest.md` — NEW playbook.
- `dux-amq/scripts/install-gocryptfs.sh` — NEW (optional helper).
- `dux-amq/README.md` — link from "Production setup" section.
- `SECURITY.md` — link "Known limitations" → here.

## Background research summary
Three options, ranked by lift:

1. **gocryptfs** (file-level FUSE encryption). Simple, single-user.
   `apt install gocryptfs`; mount `/data/state.crypt` → `/data/state`
   at SSH login; passphrase via systemd-creds or Cloud Secret Manager
   fetch. ~5% IO overhead.

2. **LUKS** (block-level). Strongest. Requires reformatting the
   persistent disk. Passphrase from Cloud KMS at boot.

3. **fscrypt** (kernel native; ext4/f2fs). Per-directory keys; needs
   kernel support and a key-wrapping service. Mid-effort.

Recommendation: ship **gocryptfs** as the documented default for
single-user spot VMs (matches the README's deployment story).
Document LUKS as the "production-grade" path for shared/long-lived
infra.

## Steps

### 25.1 — Write the playbook
`docs/operations/encryption-at-rest.md` outline:

1. **Threat model** — what cloud-default KMS protects against (lost
   disk) vs. what it doesn't (compromised IAM principal, stolen
   instance image, GCE/EC2 admin insider). Reference SECURITY.md.
2. **Option A: gocryptfs** — install, key management (Cloud Secret
   Manager fetch on SSH login), mount/unmount workflow, recovery if
   key lost.
3. **Option B: LUKS** — `cryptsetup luksFormat`, key derived from KMS
   blob, automounting via `crypttab` + an `initramfs` hook, recovery.
4. **Performance** — measured overhead on representative dux
   workloads (sqlite writes, JSONL append, git operations).
5. **Operational quirks**:
   - Spot-VM preemption mid-write: same crash-recovery story as
     unencrypted (no different).
   - Backups: do `dux session purge` BEFORE snapshotting if you don't
     want the encrypted blobs in your snapshot store.
   - Doctor (Phase 20) should report whether the mount is encrypted.
6. **GDPR Art 32 note** — encryption is one of several "appropriate
   technical measures"; alone it does not satisfy Art 32 but is part
   of a defensible posture.

### 25.2 — Install helper (gocryptfs)
`dux-amq/scripts/install-gocryptfs.sh`:
```bash
#!/usr/bin/env bash
# OPTIONAL: encrypt /data/state with gocryptfs.
# Run AFTER dux-amq/install.sh and BEFORE first dux launch on a fresh disk.
# Existing data is migrated through a temp ramdisk — only safe with
# enough RAM (~4× /data/state size).
set -euo pipefail

CIPHER_DIR="/data/state.crypt"
CLEAR_DIR="/data/state"
PASS_FILE="${GOCRYPT_PASS_FILE:-/run/credentials/gocrypt.pass}"

command -v gocryptfs >/dev/null || { echo "apt-get install gocryptfs" >&2; exit 1; }

if [[ ! -d "$CIPHER_DIR" ]]; then
  mkdir -p "$CIPHER_DIR"
  gocryptfs -init -passfile "$PASS_FILE" "$CIPHER_DIR"
fi

if mountpoint -q "$CLEAR_DIR"; then
  echo "$CLEAR_DIR already mounted (gocryptfs?)"
else
  gocryptfs -passfile "$PASS_FILE" -allow_other "$CIPHER_DIR" "$CLEAR_DIR"
fi

echo "gocryptfs: $CLEAR_DIR mounted from $CIPHER_DIR"
```
Document key sourcing from Cloud Secret Manager separately.

### 25.3 — Doctor integration
Phase 20's `dux-amq doctor` should detect a gocryptfs mount and
report it. Add a section in the doctor output:
```
== Encryption at rest ==
mount:    /data/state (gocryptfs)
cipher:   AES-256-GCM
master key fingerprint: ab:cd:...
```

### 25.4 — Backup compatibility
Document explicitly:
- `tar` of `/data/state.crypt` snapshots ciphertext; safe to upload.
- `tar` of `/data/state` (mounted) is plaintext — only do this on a
  trusted host with a known clean key.
- `dux session purge --hard` works inside the mount; ciphertext bytes
  are removed via filesystem-level delete (eventual on FUSE).

## Validation
- Manual smoke test on a throwaway VM: install, encrypt, run dux,
  reboot, re-mount, verify state survives.
- Performance test: measure `dux config regenerate` time and sqlite
  write IOPS on encrypted vs. plain. Record results in the playbook.
- `dux-amq doctor` shows the encryption section after the helper ran.

## Acceptance criteria
- [ ] `docs/operations/encryption-at-rest.md` exists and covers options A/B.
- [ ] `dux-amq/scripts/install-gocryptfs.sh` is executable + idempotent.
- [ ] Doctor reports mount type when applicable.
- [ ] SECURITY.md links to the playbook.
- [ ] README "Production setup" notes the option.
- [ ] PR: `docs(security): encryption-at-rest playbook (T5, P2-4)`.

## Known pitfalls
- gocryptfs `-allow_other` may need `user_allow_other` in
  `/etc/fuse.conf` — document.
- Migrating existing data into the encrypted mount is destructive if
  done wrong; prefer a copy-and-swap pattern with verification before
  deleting the plaintext.
- LUKS path requires reboot; not feasible on a long-lived live VM.
  Plan for fresh-VM rollout.
- Keys MUST come from a fetcher (Cloud Secret Manager / Vault); never
  commit the passphrase in any file under git.

## References
- audit02 T5; audit01 P2-4.
- gocryptfs: https://github.com/rfjakob/gocryptfs
- GCE CSEK + LUKS pattern: https://cloud.google.com/compute/docs/disks/customer-supplied-encryption
- AWS EC2 EBS LUKS pattern: AWS blog "Protect data at rest with EBS encryption".
