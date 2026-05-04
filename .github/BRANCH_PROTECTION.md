# Branch protection — expected server-side configuration

Audit02 (P0-I bundle, P0-H, audit02 §27.3 spot-check, SECURITY.md cadence rule)
mandates the following branch protection on `main`. This file is the source of
truth; configure via `gh api` (recipe at the bottom of this file) or via the
GitHub UI (Settings → Branches → Protection rules → main).

## main

- **Require pull request before merging**: yes
  - Required approvals: 1 (CODEOWNERS-aware)
  - Dismiss stale approvals on push: yes
  - Require approval from CODEOWNERS: yes
- **Required status checks** (the contexts currently enforced on `main`;
  verify with `gh api /repos/SiavZ/dux-amq-setup/branches/main/protection`):
  - `Test (ubuntu-24.04)` — `cargo test --all-features` from `.github/workflows/test.yml`
  - `Test (macos-14)` — same matrix entry, Apple Silicon runner
  - `Security` — bundles `cargo audit` + `cargo deny check` (Phase 07)
  - `shell` — `shellcheck` + `bats` from `.github/workflows/overlay-ci.yml`
  - `Strict mode (require branches up to date before merging)`: yes
- **Recommended (not currently required) but run on every PR**:
  - `Format` (cargo fmt --check) from `.github/workflows/pr.yml`
  - `Clippy (ubuntu-24.04)` and `Clippy (macos-14)` (cargo clippy -D warnings)
  - These run on every PR but are not in the required-contexts list. Add
    them to the protection if your team treats them as merge gates — the
    recipe at the bottom of this file shows the syntax.
- **Disallow force push**: yes (covers force-push to main + delete)
- **Disallow deletion**: yes
- **Require signed commits on main**: opt-in if/when GPG enrollment is in place
- **Require linear history**: no (we use merge commits for `audit02/integration`)
- **Lock branch**: no (push allowed via PRs only)

## Tag protection

- Pattern: `v*` and `dux-amq-v*`
- Restrict who can push: maintainers only (CODEOWNERS)

## Default workflow permissions

- Settings → Actions → General → Workflow permissions: **Read repository contents and packages permissions**
- Allow GitHub Actions to create and approve pull requests: **off**

## Configuration recipe (idempotent)

```bash
# Apply the currently-enforced contexts. To also gate on Format/Clippy,
# add the matching `-f 'required_status_checks[contexts][]=...'` lines.
gh api -X PUT \
  -H "Accept: application/vnd.github+json" \
  /repos/SiavZ/dux-amq-setup/branches/main/protection \
  -f required_status_checks[strict]=true \
  -f 'required_status_checks[contexts][]=Test (ubuntu-24.04)' \
  -f 'required_status_checks[contexts][]=Test (macos-14)' \
  -f 'required_status_checks[contexts][]=Security' \
  -f 'required_status_checks[contexts][]=shell' \
  -f required_pull_request_reviews[required_approving_review_count]=1 \
  -f required_pull_request_reviews[dismiss_stale_reviews]=true \
  -f required_pull_request_reviews[require_code_owner_reviews]=true \
  -f enforce_admins=false \
  -f required_linear_history=false \
  -f allow_force_pushes=false \
  -f allow_deletions=false \
  -F restrictions=null
```

**Verify after applying:**
```bash
gh api /repos/SiavZ/dux-amq-setup/branches/main/protection | jq .
```
