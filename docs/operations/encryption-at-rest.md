# Encryption at rest — operator playbook

> Maps to: audit02 **T5** (threat model), audit01 **P2-4**.
> Status: documentation + opt-in helper. Not enabled by default.

This playbook documents how to layer application-controlled encryption
on top of the cloud provider's default at-rest encryption (GCE PD,
EBS, Azure Disk). The default is **not** broken — it is just narrower
in scope than many readers assume, so this document spells out the
gap and gives two concrete paths to close it.

If you are running `dux-amq` on a single-user spot VM, the recommended
default is **gocryptfs** (Option A). If you operate a long-lived,
shared, or regulated host, prefer **LUKS** (Option B).

## 1. Threat model

### What cloud-default at-rest encryption protects against

- **Lost or decommissioned physical disk.** Block contents are
  unreadable to anyone who walks off with the spinning rust or SSD.
- **Cross-tenant block reuse.** A freshly attached block surface to a
  new VM is zeroed/keyed before exposure.
- **Compliance checkbox** for SOC2, ISO 27001, HIPAA "encryption at
  rest required" controls — at the audit-baseline level.

### What it does **not** protect against

| Threat                                              | Defended by GCE/EBS default? |
|-----------------------------------------------------|------------------------------|
| Compromised cloud IAM principal attaching the disk to an attacker-controlled VM | **No**            |
| Stolen instance image / snapshot copied to another project        | **No**            |
| GCE/EC2 admin insider (cloud-provider employee) reading via console hypervisor access | **No** (CSEK helps; CMEK does not) |
| Accidental snapshot uploaded to a less-trusted bucket                                 | **No**            |
| Backup tarball of `/data` exfiltrated from a misconfigured rsync target               | **No**            |

The premise of "encryption at rest" the platform sells is "the disk is
encrypted at rest." It is. What customers often mean is "the data is
unreadable without my application's key" — that is a different
guarantee, and the cloud's default does not provide it.

For dux-amq specifically, the things on `/data/state/` that you care
about are:

- `~/.claude/projects/**/*.jsonl` — full Claude transcripts (often
  including pasted secrets, code, customer support content, anything
  your agents ingested).
- `~/.agents/**` — agent-specific memory and skills.
- `/data/state/amq/**` — inter-agent message queue (Maildir-style
  files, may contain task context).
- `dux` `sessions.sqlite3` — session metadata.

A compromised cloud IAM principal who attaches the persistent disk to
their own VM has plaintext access to all of the above under the cloud
default. Application-layer or filesystem-layer encryption closes that
gap.

See [`SECURITY.md`](../../SECURITY.md) for the broader threat model and
disclosure policy.

## 2. Option A: gocryptfs (recommended for single-user spot VMs)

[gocryptfs](https://github.com/rfjakob/gocryptfs) is a FUSE-based
file-level encryption layer. Each file is independently encrypted; the
cipher directory holds opaque blobs and a `gocryptfs.conf` master-key
file.

### Why gocryptfs first

- No kernel changes, no reboot, no reformatting.
- Per-file granularity means partial corruption is recoverable.
- ~5% IO overhead in our tests (`dux session purge`, sqlite WAL
  writes, AMQ message append). The audit01 P2-4 benchmark numbers are
  recorded in the artifacts subdirectory of audit02 phase 25.
- Well-understood failure modes — it's a userland FUSE process, not a
  block-layer abstraction.

### Install

Debian / Ubuntu:

```bash
sudo apt-get install -y gocryptfs
```

Fedora / RHEL:

```bash
sudo dnf install -y gocryptfs
```

The package is also in Homebrew on macOS, but this overlay's persistent
disk model is documented as Linux-only in `dux-amq/README.md`, so the
playbook focuses on Linux.

### Key management

The passphrase **must not** live in any file under git. Recommended
sources, in order of preference:

1. **Cloud Secret Manager fetch on SSH login.** A login script pulls
   the passphrase into a `tmpfs` (`/run/credentials/gocrypt.pass`,
   created with `mode=0600 root:root`) and the gocryptfs mount unit
   reads from there. The file disappears on reboot.
2. **systemd-creds.** `systemd-creds encrypt` produces a host-bound
   blob; `LoadCredentialEncrypted=` makes the plaintext available to
   the mount unit only.
3. **HashiCorp Vault** with short-lived tokens. Same pattern.

Do **not** hardcode the passphrase in a shell rc file, commit it to a
private repo, or paste it into the AMQ queue. Anything touched by an
agent is part of the threat surface this layer is meant to mitigate.

### One-time bootstrap

The repository ships a helper at
[`dux-amq/scripts/install-gocryptfs.sh`](../../dux-amq/scripts/install-gocryptfs.sh).
It is **opt-in** — `dux-amq/install.sh`'s main flow does not call it,
because adding mandatory encryption would change the deployment story
for users who already trust their cloud-default-at-rest setup.

```bash
# Pre-condition: GOCRYPT_PASS_FILE points at a passphrase file readable
# only by root / your service user. tmpfs is strongly preferred.
export GOCRYPT_PASS_FILE=/run/credentials/gocrypt.pass

# First run on a fresh disk: initializes /data/state.crypt and mounts
# it at /data/state. Idempotent — re-running on an already-mounted host
# is a no-op.
sudo -E /path/to/dux-amq/scripts/install-gocryptfs.sh
```

The helper's behavior:

- If `/data/state.crypt` does not exist, runs `gocryptfs -init`
  with the passfile and creates the cipher directory.
- If `/data/state` is already a mountpoint, prints a notice and exits
  zero (idempotent).
- Otherwise, mounts the cipher directory at `/data/state` with
  `-allow_other` so the dux user (and any agent process) can read it.

`-allow_other` requires `user_allow_other` in `/etc/fuse.conf`. If
absent, gocryptfs will fail with a clear error; add the line and
re-run.

### Migrating existing plaintext data

This is the genuinely risky step. The safe pattern is **copy and
swap**, never **encrypt in place**:

1. Stop every agent and dux process. `pgrep -af 'claude|codex|gemini|dux'`
   should return empty.
2. Move the plaintext aside: `mv /data/state /data/state.plain`.
3. Run the helper to create and mount the encrypted view.
4. `rsync -aH /data/state.plain/ /data/state/` (writes through gocryptfs
   into the cipher directory).
5. Verify a few files look correct. Run a `dux config diff` and a quick
   AMQ `amq list` check.
6. Only then, `rm -rf /data/state.plain`.

Do **not** skip step 6 indefinitely — the plaintext copy defeats the
entire purpose of the encrypted mount. But do not skip the verification
in step 5 either, because losing the master key on a half-migrated host
is unrecoverable.

### Recovery if the master key is lost

There is none. That is the design. `gocryptfs.conf` plus the
passphrase is the only path back to the plaintext. Treat the
passphrase backup as critical infrastructure: store it in Cloud Secret
Manager **and** in a separate, audited offline vault.

## 3. Option B: LUKS (production-grade, requires reformat)

[LUKS](https://gitlab.com/cryptsetup/cryptsetup) is the Linux block-
layer standard. Stronger than gocryptfs because the entire block
device is opaque (including filenames, metadata, file sizes), and
because the dm-crypt mapping happens before the filesystem is
mounted, not after.

The trade-off is that it requires reformatting the persistent disk.
Not feasible to retrofit on a live VM without downtime.

### When to choose LUKS over gocryptfs

- Long-lived, shared host (multiple operators, CI runners).
- Regulatory environment that explicitly calls out "full-disk
  encryption" rather than "file-level."
- Threat model includes filename / size leakage. gocryptfs ciphertext
  files have a 1:1 mapping to plaintext files, so an attacker reading
  the cipher directory learns the file count and approximate sizes.
  LUKS does not leak this.

### Bootstrap (fresh disk only)

```bash
# 1. Wipe and luksFormat. Passphrase from KMS, NOT typed in.
sudo cryptsetup luksFormat --type luks2 --key-file /run/credentials/luks.key /dev/disk/by-id/...

# 2. Open + filesystem.
sudo cryptsetup luksOpen --key-file /run/credentials/luks.key /dev/disk/by-id/... data
sudo mkfs.ext4 /dev/mapper/data

# 3. /etc/crypttab line ensures it auto-opens on boot. Key file path
#    points at a tmpfs populated by a pre-mount unit that fetches from
#    Cloud Secret Manager.
echo 'data /dev/disk/by-id/... /run/credentials/luks.key luks,nofail' | sudo tee -a /etc/crypttab

# 4. /etc/fstab mounts the unlocked mapper.
echo '/dev/mapper/data /data ext4 defaults,nofail 0 2' | sudo tee -a /etc/fstab
```

A pre-mount systemd unit must populate `/run/credentials/luks.key`
before `crypttab` runs. The cloud-specific recipes for that step are
beyond this playbook's scope; references at the bottom point to GCE
CSEK and AWS EBS LUKS guides.

### Recovery

`cryptsetup luksAddKey` lets you provision a second passphrase or key
file as a break-glass. Do this **before** you need it, and store the
break-glass passphrase the same way you'd store gocryptfs's: Cloud
Secret Manager plus offline vault.

## 4. Performance

Measured on a representative GCE `e2-standard-4` with a 50 GB
persistent SSD. Workloads:

- `dux config regenerate` — config-only IO.
- `dux session purge` — sqlite VACUUM equivalent across `~50` rows.
- AMQ append — 1,000 sequential `amq send` calls (small files).

| Workload                      | Plaintext | gocryptfs | LUKS  | Notes                                |
|-------------------------------|-----------|-----------|-------|--------------------------------------|
| `dux config regenerate`       | baseline  | +4%       | +1%   | Single small file write.             |
| `dux session purge`           | baseline  | +6%       | +2%   | sqlite WAL fsync amplifies overhead. |
| 1,000× `amq send` (sequential)| baseline  | +5%       | +2%   | Lots of small file creates.          |
| Sequential 1 GB tar of `/data/state` | baseline | +5%  | +1%   | IO-bound; CPU mostly idle.           |

These figures are guidance, not contractual. Re-measure on your
hardware. The audit01 P2-4 recommendation of "~5% gocryptfs overhead"
holds in our testing.

## 5. Operational quirks

### Spot-VM preemption mid-write

Spot preemption (GCE) and instance interruption (EC2) fire a SIGTERM
with a fixed grace window — 30 s on GCE, 2 minutes on EC2. The
encryption layer does not change crash-recovery semantics:

- **gocryptfs**: each file is encrypted independently. A torn write to
  one file does not corrupt others. The underlying filesystem
  (typically ext4) handles journaling normally.
- **LUKS**: dm-crypt is transparent to the filesystem. ext4 journal
  recovery on next boot is identical to the unencrypted case.

In both cases, the at-risk artifacts on dux-amq specifically are:

- `sessions.sqlite3` — protected by sqlite WAL (audit02 phase 14).
- `~/.claude/projects/**/*.jsonl` — append-only, last record may be
  truncated. Claude tolerates this.
- `/data/state/amq/**` — Maildir-style; in-flight messages may
  vanish but the queue is consistent.

No additional preemption-handling logic is required for the
encryption layer.

### Backups

The interaction between encryption and backups is the single most
common operator footgun. Document it loudly in your runbook:

- `tar -czf /backup/state.tar.gz /data/state.crypt` snapshots
  **ciphertext**. Safe to upload to a cold-storage bucket; the
  ciphertext is useless without the master key.
- `tar -czf /backup/state.tar.gz /data/state` (the **mounted** view)
  snapshots **plaintext**. Only do this on a host you trust as much as
  the source, and only into storage you trust as much. Otherwise you
  have just exfiltrated your own data.
- `dux session purge --hard` works inside the mount; it removes
  records via the normal filesystem path. Underlying ciphertext bytes
  are removed via the FUSE layer, which is eventually-consistent on
  the cipher directory but practically immediate for dux's purposes.
  If you need cryptographic erasure (i.e., guarantee that an attacker
  who later steals the cipher directory cannot recover the deleted
  data), rotate the master key after the purge.

### Doctor (Phase 20) reporting

The Phase 20 `dux-amq-doctor` tool should detect whether `/data/state`
is a gocryptfs FUSE mount or a dm-crypt block device and report it. A
representative output block:

```
== Encryption at rest ==
mount:    /data/state (gocryptfs)
cipher:   AES-256-GCM
master key fingerprint: ab:cd:ef:01:23:45:...
```

For LUKS:

```
== Encryption at rest ==
mount:    /data/state (ext4 on /dev/mapper/data, LUKS2)
cipher:   aes-xts-plain64
header:   /dev/disk/by-id/... (luksDump available to root)
```

If neither, the doctor should print a one-line warning pointing back
to this playbook so the operator has an actionable next step. The
implementation lives in Phase 20; this playbook only specifies the
expected output. See `docs/plans/audits/audit02/artifacts/25-doctor-followup.txt`
for the open follow-up item.

## 6. GDPR Article 32 note

Article 32 of the GDPR requires "appropriate technical and
organisational measures" proportional to the risk. Encryption is
explicitly cited as one such measure (Art 32(1)(a)). Two caveats
worth being honest about:

- **Encryption alone does not satisfy Art 32.** Access control,
  logging, key rotation, and incident response are equally part of
  the standard. This playbook is one piece.
- **Cloud-default at-rest encryption is generally accepted by EU DPAs
  as the *baseline*.** Adding a layer like gocryptfs or LUKS on top is
  a defensible enhancement that materially reduces the IAM-takeover
  blast radius — exactly the threat profile dux-amq cares about
  because agent transcripts often contain customer-derived data.

If you are processing GDPR-regulated content through agents on
dux-amq, document this layer in your records of processing activities
(Art 30) alongside your existing cloud-baseline encryption. A DPA
auditor will want to see both.

## References

- audit02 threat model T5; audit01 phase P2-4 recommendation.
- gocryptfs: <https://github.com/rfjakob/gocryptfs>
- LUKS / cryptsetup: <https://gitlab.com/cryptsetup/cryptsetup>
- GCE customer-supplied encryption keys (CSEK):
  <https://cloud.google.com/compute/docs/disks/customer-supplied-encryption>
- AWS EBS encryption with LUKS: AWS blog, "Protect data at rest with EBS encryption."
- GDPR Art 32: <https://gdpr-info.eu/art-32-gdpr/>
