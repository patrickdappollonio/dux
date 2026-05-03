# Security policy

> Stub introduced in audit02 phase 25. Phase 26 will replace/extend
> this file with the full threat model, disclosure SLA, and the
> coordinated-disclosure contact details. Do not treat the contents
> below as final policy.

## Reporting a vulnerability

If you believe you have found a security issue in this repository
(`dux-amq-setup`, including the dux fork and the dux-amq overlay),
please report it privately rather than opening a public GitHub issue.

Until Phase 26 lands a formal contact channel, send a private email
describing the issue and (if you have one) a minimal reproducer to
the maintainer listed in the repository's `Cargo.toml` / git history.
A GitHub Security Advisory draft against this repository is also
acceptable.

## Scope

In scope:

- The dux Rust source under `src/`.
- The dux-amq overlay scripts under `dux-amq/` (installer, wrappers,
  helper scripts including the optional encryption helper).
- The CI workflows under `.github/workflows/` insofar as they affect
  release artifact integrity.

Out of scope (report upstream):

- Issues in the upstream `dux` project that this fork has not
  modified — please file with `patrickdappollonio/dux`.
- Issues in upstream `claude`, `codex`, `opencode`, `gemini` CLIs —
  file with their respective vendors.
- Issues in the cloud provider (GCE, AWS, Azure) hosting the VM that
  runs dux-amq.

## Known limitations

- **At-rest encryption.** The cloud provider's default at-rest
  encryption protects against physical-disk theft but does **not**
  defend against a compromised cloud IAM principal who can attach
  the persistent disk elsewhere. For the stronger story, see the
  operator playbook at
  [`docs/operations/encryption-at-rest.md`](docs/operations/encryption-at-rest.md).
  An opt-in helper is shipped at
  `dux-amq/scripts/install-gocryptfs.sh`.

Phase 26 will expand this section with the full known-limitations
catalogue derived from the audit02 threat model.
