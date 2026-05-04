# Phase 07: CI security gates — cargo-audit, cargo-deny, auditable, SBOM, signing, SHA256SUMS

> Maps to: **P0-I** (full bundle).

## Goal
Add the missing CI/CD security baseline: dependency advisory + license
gating in PR CI, embedded auditability in release binaries, SBOM in
release artifacts, build provenance attestation, and a `SHA256SUMS`
file users can verify against.

## Pre-conditions
- Phase 00 baseline green.
- Phase 06 (SHA pinning) merged — every new action added here must also
  be pinned by SHA.

## Files to touch
- `.github/workflows/pr.yml` — add `security` job.
- `.github/workflows/test.yml` — same security job (or share via
  reusable workflow).
- `.github/workflows/release.yml` — add SBOM, attestation, auditable
  build, SHA256SUMS, optional cosign signing.
- `deny.toml` — NEW (root).
- `Cargo.toml` — no change unless adding a `cargo-auditable` profile.

## Steps

### 7.1 — Write `deny.toml`
Start from the [Embark example](https://github.com/EmbarkStudios/cargo-deny/blob/main/deny.toml)
and trim. Minimum viable:
```toml
[advisories]
db-path = "~/.cargo/advisory-db"
db-urls = ["https://github.com/rustsec/advisory-db"]
yanked = "deny"
ignore = []
# fail PRs on any unaddressed RUSTSEC advisory

[bans]
multiple-versions = "warn"   # warn first; tighten later
wildcards = "deny"
highlight = "all"
deny = []                    # add specific banned crates as needed
skip = [
  # signal-hook 0.3 + 0.4 coexist; documented in audit02 P2-20
  { name = "signal-hook", version = "*" },
]

[licenses]
allow = [
  "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib",
  "Unicode-DFS-2016", "Unicode-3.0",
  "MPL-2.0",                # weak copyleft — review before relaxing
]
exceptions = []
unused-allowed-license = "allow"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
allow-git = []               # NO git deps allowed
```
Run `cargo deny check` locally; iterate until clean. Expect noisy
license complaints first run — fix by either allowlisting (above) or
filing upstream (rare).

### 7.2 — Add the `security` PR job
Add to `pr.yml` (and mirror in `test.yml`):
```yaml
security:
  runs-on: ubuntu-24.04
  permissions:
    contents: read
  steps:
    - uses: actions/checkout@<sha> # v4.2.2
    - uses: dtolnay/rust-toolchain@<sha> # stable
    - uses: Swatinem/rust-cache@<sha> # v2.7.5
      with:
        save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}
    - name: Install cargo-audit + cargo-deny
      run: cargo install --locked cargo-audit cargo-deny
    - name: Audit
      run: cargo audit --deny warnings
    - name: Deny
      run: cargo deny check
```
Make this a required check in branch protection (Phase 24 will document
the GitHub UI side).

### 7.3 — Auditable release build
Edit `release.yml` build job:
```yaml
- name: Install cargo-auditable
  run: cargo install --locked cargo-auditable
- name: Build (auditable)
  run: cargo auditable build --release --target ${{ matrix.target }}
```
Replace the existing `cargo build --release …` step. ~4 KB binary
overhead; `cargo audit bin <binary>` and `trivy fs --scanners vuln` can
now scan release artifacts post-publication.

### 7.4 — Generate SBOM
Add a step:
```yaml
- name: Install cargo-cyclonedx
  run: cargo install --locked cargo-cyclonedx
- name: Generate SBOM
  run: |
    cargo cyclonedx --format json --output-file sbom-${{ matrix.target }}.cdx.json
```
Upload the SBOM as a release asset alongside the tarball.

### 7.5 — Build provenance attestation
Use GitHub's free OIDC-keyless attestation:
```yaml
- name: Attest build provenance
  uses: actions/attest-build-provenance@<sha>
  with:
    subject-path: |
      target/${{ matrix.target }}/release/dux
      sbom-${{ matrix.target }}.cdx.json
```
Add `permissions: { id-token: write, contents: write, attestations: write }`
to the build job (job-level, not workflow-level).

Verifiable downstream via:
```bash
gh attestation verify dux-linux-amd64.tar.gz --owner SiavZ
```

### 7.6 — SHA256SUMS file
After tar steps, in the `upload-install-script` (or a new `package`)
job:
```yaml
- name: Generate SHA256SUMS
  run: |
    cd dist  # wherever assets land
    sha256sum *.tar.gz > SHA256SUMS
- name: Upload SHA256SUMS
  run: gh release upload "$TAG" dist/SHA256SUMS --clobber
```

This closes the loop with `dux-amq/install.sh:29` — the human-derived
hash there can now be replaced by `curl -fsSL .../SHA256SUMS | grep …`.
Document in `dux-amq/install.sh` header + Phase 24 (P2 bundle).

### 7.7 — Reproducible tar (P2-7)
Replace `tar czf …` with:
```yaml
- name: Package (reproducible)
  run: |
    tar --sort=name --owner=0 --group=0 --numeric-owner \
        --mtime='@${{ github.event.release.created_at }}' \
        -czf "dux-${{ matrix.target }}.tar.gz" \
        target/${{ matrix.target }}/release/dux
```
Two reruns of the same tag now produce byte-identical tarballs.

### 7.8 — Optional: cosign sign release blobs
Defer to a follow-up if the github-attest pipeline is enough. If
required (high-trust deploy environments):
```yaml
- uses: sigstore/cosign-installer@<sha>
- name: Sign release assets
  run: |
    for f in dist/*.tar.gz dist/SHA256SUMS dist/*.cdx.json; do
      cosign sign-blob --yes \
        --output-signature "${f}.sig" \
        --output-certificate "${f}.crt" \
        "$f"
    done
```
Upload `.sig` + `.crt` per asset.

## Validation
- `cargo audit` and `cargo deny check` pass locally.
- `gh pr checks` shows the new `security` job green.
- A trial release on a `rc1` tag produces: tarball, SHA256SUMS, SBOM,
  attestation. `gh attestation verify <tarball>` succeeds.
- Two reruns of the same tag produce byte-identical tarballs (`sha256sum`
  matches).

## Acceptance criteria
- [x] `deny.toml` at repo root, passes `cargo deny check`.
- [x] `pr.yml` and `test.yml` have a `security` job running both
      `cargo audit --deny warnings` and `cargo deny check`.
- [x] `release.yml` build uses `cargo auditable build`.
- [x] `release.yml` produces an SBOM per target (cargo-cyclonedx).
- [x] `release.yml` calls `actions/attest-build-provenance` (with
      job-level `id-token: write`).
- [x] Release uploads `SHA256SUMS`.
- [x] tar invocation uses reproducibility flags (`--sort=name --owner=0 --group=0 --numeric-owner --mtime=...`).
- [ ] (Optional) cosign signing wired up — deferred to a follow-up; attestation is sufficient for current threat model.
- [x] PR: `ci(security): cargo-audit/deny + auditable + SBOM + provenance + SHA256SUMS (P0-I)` — landed via PR #2.

## Known pitfalls
- `cargo deny check` on first run will surface license complaints from
  transitive deps. Don't blanket-allow MPL/LGPL — review each.
- `actions/attest-build-provenance` requires `id-token: write` AND
  `attestations: write`. Both must be on the same job.
- `cargo cyclonedx` may produce SBOMs missing C/C++ system deps
  (rusqlite bundled SQLite). Document the limitation; add a `dev-deps`
  separation if downstream consumers complain.
- Reproducible tar relies on `SOURCE_DATE_EPOCH`-equivalent flags; if
  you instead use `tar --posix`, the format differs slightly across
  GNU vs BSD tar. CI is GNU tar (`ubuntu-24.04`); macOS release builds
  must standardize on GNU tar (`brew install gnu-tar` then `gtar`).

## References
- audit02 P0-I.
- cargo-auditable: https://github.com/rust-secure-code/cargo-auditable
- cargo-deny example: https://github.com/EmbarkStudios/cargo-deny/blob/main/deny.toml
- GitHub artifact attestations: docs.github.com/.../using-artifact-attestations
- SLSA framework: slsa.dev
