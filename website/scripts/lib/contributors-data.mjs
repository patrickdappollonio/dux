// Pure transforms for the homepage contributor list. No I/O here so the logic
// stays unit-testable; the network/filesystem work lives in
// scripts/fetch-contributors.mjs, which imports these.

// Local served path for a contributor's downloaded avatar. The fetch script
// writes the PNG to public/contributors/<login>.png, which Astro serves from
// the site root at this path.
export function avatarPath(login) {
  return `/contributors/${login}.png`;
}

// GitHub's own username constraint: alphanumerics and single hyphens, 1–39
// chars, no leading hyphen. Enforcing it locally means a login is always safe
// to use as a filename and URL path segment — no path traversal can slip in.
const LOGIN_RE = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/;

// True when `value` is an https URL on exactly `host`. Guards against a
// malformed string throwing out of the filter, and pins avatar fetches /
// profile links to GitHub so a tampered API response can't redirect them
// (no SSRF to internal hosts, no javascript: hrefs).
function isHttpsHost(value, host) {
  try {
    const u = new URL(value);
    return u.protocol === "https:" && u.hostname === host;
  } catch {
    return false;
  }
}

// Transform the raw GitHub `GET /repos/{owner}/{repo}/contributors` response
// into the working list used by the fetch script. Bots and malformed records
// are dropped; contribution order (the API sorts by contributions desc) is
// preserved. Each entry carries the remote `avatarUrl` (for the downloader)
// and the local `avatar` path (for the snapshot/component).
//
// The API response is external input, so every field a downstream consumer
// trusts is validated here at the single choke point: the login must match
// GitHub's username grammar (safe as a filename/URL segment), the profile URL
// must be a github.com link (rendered as an href), and the avatar URL must be
// on GitHub's avatar host (fetched at build time). Anything that fails is
// dropped rather than written to disk or baked into the page.
export function normalizeContributors(records) {
  if (!Array.isArray(records)) return [];
  return records
    .filter(
      (r) =>
        r &&
        typeof r.login === "string" &&
        LOGIN_RE.test(r.login) &&
        typeof r.html_url === "string" &&
        r.html_url.startsWith("https://github.com/") &&
        typeof r.avatar_url === "string" &&
        isHttpsHost(r.avatar_url, "avatars.githubusercontent.com") &&
        r.type !== "Bot" &&
        !r.login.endsWith("[bot]"),
    )
    .map((r) => ({
      login: r.login,
      profileUrl: r.html_url,
      avatarUrl: r.avatar_url,
      avatar: avatarPath(r.login),
    }));
}

// Reduce the working list to the fields persisted in contributors.json and read
// by the component. The remote `avatarUrl` is intentionally dropped: the site
// serves the downloaded local copy, never the GitHub URL.
export function toSnapshot(normalized) {
  return normalized.map(({ login, profileUrl, avatar }) => ({
    login,
    profileUrl,
    avatar,
  }));
}
