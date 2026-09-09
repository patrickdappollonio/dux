// The version the site advertises and the "what's new" stamp beside it: both are
// release-cycle values, so a release has one file to think about.
//
// The version is read from the release itself at build time rather than written
// down anywhere: the version in Cargo.toml and package.json are placeholders the
// release process does not bump, and `git describe` needs tags the deploy
// workflow's checkout does not guarantee. The site deploys on `release:
// published`, so the latest release is always the one this page announces.
//
// A failed lookup drops the version segment from the pill entirely: the pill can
// be missing a version, never display a wrong one, and never fail the build.
// GitHub's `releases/latest` skips drafts and prereleases, so publishing a
// prerelease keeps the previous stable version on the page.
import { fetchJson, githubHeaders } from "./remote-json";
// @ts-expect-error - plain .mjs helper, shared with the plain-Node build scripts
import { unexpectedShapeWarning } from "./remote-failure.mjs";

/** Looks like a release tag: `v1`, `v1.2`, `v1.2.3`, optional `-rc.1` suffix. */
const TAG_SHAPE = /^v\d+(\.\d+){0,2}(-[0-9A-Za-z.-]+)?$/;

/**
 * The latest published release tag for `repo` (e.g. `"v0.7.0"`), or `null` on
 * any failure. The shape check keeps a surprising `tag_name` from being rendered
 * as a version string.
 */
export async function getLatestVersion(repo: string): Promise<string | null> {
  const url = `https://api.github.com/repos/${repo}/releases/latest`;
  const label = `the latest release tag for ${repo}`;
  const effect = "The hero pill renders without a version";
  const data = await fetchJson<{ tag_name?: string }>(url, {
    label,
    effect,
    headers: githubHeaders(),
  });
  if (!data) return null;
  const tag = data.tag_name?.trim();
  if (!tag || !TAG_SHAPE.test(tag)) {
    console.warn(
      unexpectedShapeWarning({
        label,
        url,
        effect,
        detail: tag
          ? `the tag \`${tag}\` is not version-shaped, so it is not rendered as one`
          : "the release carried no `tag_name`",
      }),
    );
    return null;
  }
  return tag;
}

/**
 * The "what's new in this release" stamp on the hero headline. It names the
 * surface a release added, never how the surface is reached. `null` retires the
 * badge from every place it renders with no markup changes, which is the exit
 * once "now with …" stops being true.
 */
export const WHATS_NEW_BADGE: string | null = "now with a web UI";

/** Where the badge sends you: the section that explains what it is announcing. */
export const WHATS_NEW_HREF = "#surfaces";
