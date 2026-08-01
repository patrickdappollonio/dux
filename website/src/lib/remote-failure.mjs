// One place that turns a failed build-time lookup into a line a human can act
// on, shared by `src/lib/remote-json.ts` (the star count, download counters and
// release tag) and `scripts/fetch-contributors.mjs` (the contributor snapshot).
//
// WHY THIS EXISTS AT ALL
//
// Every one of those inputs comes from somebody else's server, so every one of
// them DEGRADES rather than failing the build: the counter hides, the badge
// disappears, the contributor snapshot stays at whatever was last committed.
// That is deliberate and stays. GitHub allows roughly 60 unauthenticated API
// requests an hour per IP, and this site makes several per build, so a
// contributor who builds a few times in an hour will be rate limited. Their
// build breaking over someone else's quota would not be a defect in this repo.
//
// What was wrong is that the degradation was SILENT. A hidden counter and a
// broken counter looked identical, so nobody could tell which one they had.
// These messages close that gap: the build still succeeds, but it says out loud
// which input it skipped, what the server answered, and what to do about it.
//
// Plain `.mjs` with no I/O so both the TypeScript site code and the plain-Node
// build script can import it, and so it is unit-testable without a network.

// Setting a token lifts the unauthenticated limit from about 60 requests an
// hour to 5000. CI already does this (see .github/workflows/pages.yml, which
// passes the workflow's own `github.token` as GH_TOKEN), which is why rate
// limiting shows up locally far more often than it does on a deploy.
const TOKEN_ADVICE =
  "Set GH_TOKEN to lift it (CI already does: see .github/workflows/pages.yml); " +
  "locally, `export GH_TOKEN=$(gh auth token)` is enough, and no token scopes " +
  "are needed for public data.";

/** True for a URL served by the GitHub REST API, whose 403 means rate limit. */
/**
 * Emit a degradation line so a human sees it in whatever is reading the build.
 *
 * On a normal terminal that is `console.warn`. Under GitHub Actions it becomes a
 * `::warning::` workflow command, which surfaces the line as an annotation on
 * the run summary and the pull request instead of burying it inside a collapsed
 * step. The deploy is infrequent and succeeds even fully degraded, so without
 * this a homepage could publish with every counter missing and the only trace
 * would be inside a step nobody expands on a green run.
 *
 * Two details that are easy to get wrong. Workflow commands are read from
 * STDOUT, so the annotation goes through `console.log`; `console.warn` writes to
 * stderr and is not parsed. And a command must be one line, so any newline is
 * percent-encoded the way Actions expects rather than silently truncating the
 * message at the first break.
 */
export function emitDegradation(message) {
  if (process.env.GITHUB_ACTIONS === "true") {
    console.log(`::warning::${String(message).replace(/\r/g, "").replace(/\n/g, "%0A")}`)
    return
  }
  console.warn(message)
}

export function isGitHubApi(url) {
  try {
    return new URL(url).hostname === "api.github.com";
  } catch {
    return false;
  }
}

// A GitHub 403 or 429 is the rate limiter. This is worth stating explicitly
// because 403 reads as "you are not allowed", and every other API on the
// internet means exactly that by it. GitHub does not: the repositories this
// site reads are public and need no permission at all.
function isRateLimited(url, status) {
  return isGitHubApi(url) && (status === 403 || status === 429);
}

function statusLine(status, statusText) {
  const text = statusText ? ` ${statusText}` : "";
  return `HTTP ${status}${text}`;
}

/**
 * A readable reason from a thrown fetch error. Node's fetch reports every
 * transport problem as the bare string "fetch failed" and hides the useful part
 * (`getaddrinfo EAI_AGAIN api.github.com`, `certificate has expired`) on
 * `cause`, so unwrapping one level is the difference between a message that
 * identifies the problem and one that only confirms there was one.
 *
 * @param {unknown} e   The thrown value.
 * @param {string} timeoutText  What to say when the abort timer fired.
 */
export function errorReason(e, timeoutText = "timed out") {
  if (e?.name === "AbortError") return timeoutText;
  const message = e?.message ?? String(e);
  const cause = e?.cause?.message;
  return cause && cause !== message ? `${message}: ${cause}` : message;
}

/**
 * The warning printed when a build-time lookup fails and the site degrades.
 *
 * @param {object} f
 * @param {string} f.label   What was being fetched, in prose ("the star count
 *                           for patrickdappollonio/dux"). Named first because
 *                           it is the part that tells a reader which piece of
 *                           the page is now missing.
 * @param {string} f.url     The exact endpoint, so the failure is reproducible
 *                           with curl.
 * @param {string} f.effect  What the site does instead ("the badge is hidden").
 * @param {number} [f.status]      HTTP status, when the server answered.
 * @param {string} [f.statusText]  HTTP status text, when there is one.
 * @param {string} [f.reason]      Error/abort reason, when nothing answered.
 * @param {boolean} [f.hasToken]   Whether a GitHub token was in play.
 * @returns {string} A single build-log line.
 */
export function degradationWarning({
  label,
  url,
  effect,
  status,
  statusText,
  reason,
  hasToken = false,
}) {
  const parts = [`site build: skipping ${label}.`];

  if (typeof status === "number") {
    parts.push(`${statusLine(status, statusText)} from ${url}.`);
    if (isRateLimited(url, status)) {
      parts.push(
        `A ${status} from the GitHub API is RATE LIMITING, not a permissions problem: ` +
          "this data is public and needs no access. Unauthenticated builds share " +
          "about 60 requests an hour per IP.",
      );
      parts.push(
        hasToken
          ? "A token was already in use, so this hit the authenticated limit; wait for the window to reset and build again."
          : TOKEN_ADVICE,
      );
    } else if (status === 404) {
      parts.push(
        "A 404 means the repository, release or package does not exist, is private, " +
          "or has never published anything. That is not rate limiting and a token will " +
          "not change it; check the name in the source.",
      );
    } else if (status >= 500) {
      parts.push(
        "A 5xx is the upstream service failing, not this build; it usually clears on its own.",
      );
    } else {
      parts.push("The server answered, but not with the data this build asked for.");
    }
  } else {
    parts.push(`No response from ${url} (${reason ?? "unknown error"}).`);
    parts.push(
      "Nothing answered at all, so this is connectivity (offline, proxy, firewall, " +
        "or a timeout), not rate limiting and not a missing resource.",
    );
  }

  parts.push(`${effect} and the build continues.`);
  return parts.join(" ");
}

/**
 * The warning for a lookup that SUCCEEDED but did not carry the value the site
 * needs (a release with no `tag_name`, a repo record with no star count). Kept
 * separate from the transport failures above because there is nothing to retry
 * and no token to set: the endpoint answered, the shape was not what we read.
 */
export function unexpectedShapeWarning({ label, url, effect, detail }) {
  return (
    `site build: skipping ${label}. ${url} answered successfully but ${detail}. ` +
    "That is a response-shape problem rather than a network or rate-limit one, so " +
    `retrying will not help. ${effect} and the build continues.`
  );
}
