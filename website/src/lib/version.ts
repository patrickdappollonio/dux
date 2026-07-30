// The version the site advertises, and the "what's new" stamp that sits next to
// it. Both live here because both are release-cycle values, so a release only
// ever has one file to think about.
//
// WHY THIS IS DERIVED AND NOT A CONSTANT
//
// The hero pill used to hardcode `v0.5`, which was already two releases stale.
// Every hardcoded alternative has the same failure mode: it goes wrong silently,
// because nothing breaks when a number is merely out of date.
//
// The candidates, and why they lost:
//   - Cargo.toml's `[workspace.package] version` is "0.1.0", a placeholder that
//     the release process does not bump. It is not the release version.
//   - npm/package.json is "0.0.0-dev", same story.
//   - `git describe` needs tags in the checkout. `.github/workflows/pages.yml`
//     checks out a single ref with actions/checkout, so tag availability is an
//     implementation detail of the action, not something the site can rely on.
//   - A named constant here still needs a human to remember it.
//
// So the version is READ FROM THE RELEASE ITSELF, at build time, through the
// same `fetchJson` helper the star count and download counters already use. The
// site deploys on the `release: published` event (see
// .github/workflows/pages.yml), so by the time this build runs, the release this
// page is announcing is already the repository's latest release. There is
// nothing left to forget.
//
// The failure mode is deliberately "show nothing", not "show something old":
// when the lookup fails (offline build, rate limit, timeout) the caller drops
// the version segment from the pill entirely. The pill can be missing a version.
// It can never display a wrong one.
//
// Known limitation: GitHub's `releases/latest` endpoint skips drafts and
// prereleases. Publish a version as a prerelease and the site keeps showing the
// previous stable one, which is the correct answer for a marketing page anyway.
import { fetchJson, githubHeaders } from "./remote-json";

/** Looks like a release tag: `v1`, `v1.2`, `v1.2.3`, optional `-rc.1` suffix. */
const TAG_SHAPE = /^v\d+(\.\d+){0,2}(-[0-9A-Za-z.-]+)?$/;

/**
 * The latest published release tag for `repo` (e.g. `"v0.7.0"`), or `null` on
 * any failure. The shape check keeps a surprising `tag_name` from being rendered
 * as a version string.
 */
export async function getLatestVersion(repo: string): Promise<string | null> {
  const data = await fetchJson<{ tag_name?: string }>(
    `https://api.github.com/repos/${repo}/releases/latest`,
    githubHeaders(),
  );
  const tag = data?.tag_name?.trim();
  return tag && TAG_SHAPE.test(tag) ? tag : null;
}

/**
 * The "what's new in this release" stamp on the hero headline.
 *
 * RETIRING IT: "now with X" is true for one release and quietly false forever
 * after. When the web UI stops being news (roughly one release after v0.7.0, or
 * whenever the next headline feature lands), do one of two things and nothing
 * else:
 *   - point it at the new thing, keeping the same "now with …" shape, or
 *   - set this to `null`, which removes the badge from every place it renders
 *     with no markup changes.
 */
export const WHATS_NEW_BADGE: string | null = "now with remote work";

/** Where the badge sends you: the section that explains what it is announcing. */
export const WHATS_NEW_HREF = "#web";
