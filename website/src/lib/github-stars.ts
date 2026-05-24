// Build-time GitHub star lookup. Counts are fetched once per repo while the
// site builds and baked into the HTML, so there are no client-side API calls
// and no per-visitor rate limits. The numbers refresh on each deploy.
//
// Every failure mode degrades gracefully to `null` (network blocked, rate
// limited, repo moved/missing), and the caller simply omits the badge rather
// than failing the build. In CI a token (GH_TOKEN / GITHUB_TOKEN) lifts the
// rate limit; locally the unauthenticated limit is plenty for a short list.

const cache = new Map<string, Promise<number | null>>();

export function getStars(repo: string): Promise<number | null> {
  let pending = cache.get(repo);
  if (!pending) {
    pending = fetchStars(repo);
    cache.set(repo, pending);
  }
  return pending;
}

async function fetchStars(repo: string): Promise<number | null> {
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 5000);
  try {
    const res = await fetch(`https://api.github.com/repos/${repo}`, {
      headers: {
        Accept: "application/vnd.github+json",
        "User-Agent": "getdux-site-build",
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
      },
      signal: controller.signal,
    });
    if (!res.ok) return null;
    const data = await res.json();
    return typeof data.stargazers_count === "number"
      ? data.stargazers_count
      : null;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

export function formatStars(n: number): string {
  if (n < 1000) return String(n);
  return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
}
