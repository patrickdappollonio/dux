// Build-time project stats for the homepage: total release-asset downloads
// (Homebrew pulls these too) and npm downloads over the last month. Both are
// fetched once at build and baked into the HTML. Either returns `null` on
// failure, and the caller hides that counter.
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

export async function getNpmLastMonth(pkg: string): Promise<number | null> {
  const data = await fetchJson<{ downloads?: number }>(
    `https://api.npmjs.org/downloads/point/last-month/${pkg}`,
  );
  return typeof data?.downloads === "number" ? data.downloads : null;
}

export function formatCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
}
