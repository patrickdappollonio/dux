// Build-time project stats for the homepage: total release-asset downloads
// (Homebrew pulls these too) and all-time npm downloads. Both are fetched once
// at build and baked into the HTML. Either returns `null` on failure, and the
// caller hides that counter.
import { fetchJson, githubHeaders } from "./remote-json";

interface Release {
  assets?: Array<{ download_count?: number }>;
}

export async function getReleaseDownloads(repo: string): Promise<number | null> {
  const releases = await fetchJson<Release[]>(
    `https://api.github.com/repos/${repo}/releases?per_page=100`,
    githubHeaders(),
  );
  if (!releases) return null;
  let total = 0;
  for (const release of releases) {
    for (const asset of release.assets ?? []) {
      total += asset.download_count ?? 0;
    }
  }
  return total;
}

// Earliest day npm exposes download statistics for any package.
const NPM_STATS_EPOCH = "2015-01-10";

function isoDay(d: Date): string {
  return d.toISOString().slice(0, 10);
}

// First-publish day for a package, used as the lower bound when summing
// all-time downloads. Falls back to the npm stats epoch when the registry
// lookup fails or the package predates it.
async function getNpmFirstPublish(pkg: string): Promise<string> {
  const data = await fetchJson<{ time?: { created?: string } }>(
    `https://registry.npmjs.org/${pkg}`,
  );
  const created = data?.time?.created?.slice(0, 10);
  return created && created > NPM_STATS_EPOCH ? created : NPM_STATS_EPOCH;
}

// Total npm downloads across the package's whole lifetime. npm's point API
// caps each query at 18 months, so we sum consecutive (non-overlapping)
// 17-month windows from the first-publish day up to today. Returns null only
// when no window yields data, so the caller hides the counter just as it would
// on a failed lookup.
export async function getNpmTotal(pkg: string): Promise<number | null> {
  const today = new Date();
  let windowStart = new Date(await getNpmFirstPublish(pkg));
  let total = 0;
  let seen = false;
  while (windowStart <= today) {
    const windowEnd = new Date(windowStart);
    windowEnd.setMonth(windowEnd.getMonth() + 17);
    if (windowEnd > today) windowEnd.setTime(today.getTime());
    const data = await fetchJson<{ downloads?: number }>(
      `https://api.npmjs.org/downloads/point/${isoDay(windowStart)}:${isoDay(windowEnd)}/${pkg}`,
    );
    if (typeof data?.downloads === "number") {
      total += data.downloads;
      seen = true;
    }
    windowStart = new Date(windowEnd);
    windowStart.setDate(windowStart.getDate() + 1);
  }
  return seen ? total : null;
}

export function formatCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
}
