// Shared build-time JSON fetch, cached per-URL for the build. Every failure mode
// degrades to `null` and the caller hides whatever is missing, because these read
// somebody else's server and a contributor's build should not stop over a third
// party's downtime or rate limit. It is never silent, though: each degraded
// lookup prints one line naming the input, endpoint, status and fix, so a hidden
// counter can be told from a broken one (message text in remote-failure.mjs).
//
// The homepage web-UI figure is the input that may NOT degrade, because it comes
// from this repository; scripts/verify-figure.mjs fails the build outright.
// @ts-expect-error - plain .mjs helper, shared with the plain-Node build scripts
import { degradationWarning, errorReason, emitDegradation } from "./remote-failure.mjs";

const cache = new Map<string, Promise<unknown>>();

// One warning per input, not per request: `getNpmTotal` walks the package's
// lifetime in windows and would otherwise repeat the same line for each.
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
 * caller with a 200 carrying the wrong shape reports in the same voice.
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
