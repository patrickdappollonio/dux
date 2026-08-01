// Shared build-time JSON fetch. Results are cached per-URL for the duration of
// the build and every failure mode (network blocked, rate limit, timeout, bad
// status) degrades to `null` so a flaky API never fails the build.
//
// The degradation is deliberate: every caller here reads somebody else's server
// (the GitHub API, the npm registry), and a contributor's build should not stop
// because a third party is down, or because they have spent the ~60
// unauthenticated GitHub requests an hour that everyone on their IP shares. The
// callers hide whatever is missing, which is how the page renders without a
// star badge or a download counter.
//
// It is deliberately NOT silent, though. Every degraded lookup prints one line
// naming the input, the endpoint, the status and what to do about it, so a
// hidden counter can be told apart from a broken one by reading the build log.
// The message text lives in src/lib/remote-failure.mjs.
//
// (The one build input that is NOT allowed to degrade is the homepage web-UI
// figure, because it comes from this repository rather than from a third party.
// That check lives in scripts/check-web-ui.mjs and fails the build outright.)
// @ts-expect-error - plain .mjs helper, shared with the plain-Node build scripts
import { degradationWarning, errorReason, emitDegradation } from "./remote-failure.mjs";

const cache = new Map<string, Promise<unknown>>();

// One warning per input, not per request. `getNpmTotal` walks the package's
// whole lifetime in 17-month windows, so an offline build would otherwise print
// the same "npm is unreachable" line once per window.
const warned = new Set<string>();

export interface RemoteInput {
  /** What is being fetched, in prose: "the star count for owner/repo". */
  label: string;
  /** What the page does without it: "the badge is hidden". */
  effect: string;
  headers?: Record<string, string>;
}

export function fetchJson<T = unknown>(
  url: string,
  input: RemoteInput,
): Promise<T | null> {
  let pending = cache.get(url) as Promise<T | null> | undefined;
  if (!pending) {
    pending = doFetch<T>(url, input);
    cache.set(url, pending);
  }
  return pending;
}

/**
 * Prints a degradation warning, at most once per input label. Exported so a
 * caller that got a 200 carrying the wrong shape can report it in the same
 * voice as a transport failure.
 */
export function warnDegraded(
  input: Pick<RemoteInput, "label" | "effect">,
  details: { url: string; status?: number; statusText?: string; reason?: string },
): void {
  if (warned.has(input.label)) return;
  warned.add(input.label);
  emitDegradation(
    degradationWarning({
      label: input.label,
      effect: input.effect,
      hasToken: hasGithubToken(),
      ...details,
    }),
  );
}

async function doFetch<T>(url: string, input: RemoteInput): Promise<T | null> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 6000);
  try {
    const res = await fetch(url, {
      headers: { "User-Agent": "getdux-site-build", ...(input.headers ?? {}) },
      signal: controller.signal,
    });
    if (!res.ok) {
      warnDegraded(input, { url, status: res.status, statusText: res.statusText });
      return null;
    }
    return (await res.json()) as T;
  } catch (e) {
    warnDegraded(input, { url, reason: errorReason(e, "timed out after 6s") });
    return null;
  } finally {
    clearTimeout(timer);
  }
}

function hasGithubToken(): boolean {
  return Boolean(process.env.GH_TOKEN || process.env.GITHUB_TOKEN);
}

// GitHub API headers, with a token when one is available (CI sets GH_TOKEN) to
// lift the unauthenticated rate limit.
export function githubHeaders(): Record<string, string> {
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  return {
    Accept: "application/vnd.github+json",
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  };
}
