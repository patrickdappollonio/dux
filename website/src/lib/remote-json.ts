// Shared build-time JSON fetch. Results are cached per-URL for the duration of
// the build and every failure mode (network blocked, rate limit, timeout, bad
// status) degrades to `null` so a flaky API never fails the build.

const cache = new Map<string, Promise<unknown>>();

export function fetchJson<T = unknown>(
  url: string,
  headers: Record<string, string> = {},
): Promise<T | null> {
  let pending = cache.get(url) as Promise<T | null> | undefined;
  if (!pending) {
    pending = doFetch<T>(url, headers);
    cache.set(url, pending);
  }
  return pending;
}

async function doFetch<T>(
  url: string,
  headers: Record<string, string>,
): Promise<T | null> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 6000);
  try {
    const res = await fetch(url, {
      headers: { "User-Agent": "getdux-site-build", ...headers },
      signal: controller.signal,
    });
    if (!res.ok) return null;
    return (await res.json()) as T;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
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
