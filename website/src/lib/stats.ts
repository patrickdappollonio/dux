// Build-time project stats for the homepage: total release-asset downloads
// (Homebrew pulls these too) and all-time npm downloads. Both are fetched once
// at build and baked into the HTML. Either returns `null` on failure, and the
// caller hides that counter.
//
// Hiding rather than failing is deliberate: these are third-party numbers, and
// GitHub's ~60-requests-an-hour unauthenticated limit is a normal thing for a
// contributor to hit. Every hidden counter is announced in the build log by
// `fetchJson`, so nobody has to guess whether it was skipped or broken.
import { fetchJson, githubHeaders } from "./remote-json";
// @ts-expect-error - plain .mjs helper, shared with the plain-Node build scripts
import { unexpectedShapeWarning } from "./remote-failure.mjs";

interface Release {
  assets?: Array<{ download_count?: number }>;
}

export async function getReleaseDownloads(repo: string): Promise<number | null> {
  const releases = await fetchJson<Release[]>(
    `https://api.github.com/repos/${repo}/releases?per_page=100`,
    {
      label: `the release download total for ${repo}`,
      effect: "The Downloads counter (and the combined Total) is hidden",
      headers: githubHeaders(),
    },
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
// lookup fails or the package predates it. It still announces the skip, but its
// stated effect is deliberately reassuring: the epoch bound produces the same
// total, just with a few wasted empty windows, so no figure on the page moves.
async function getNpmFirstPublish(pkg: string): Promise<string> {
  const data = await fetchJson<{ time?: { created?: string } }>(
    `https://registry.npmjs.org/${pkg}`,
    {
      label: `the first-publish date for ${pkg}`,
      effect:
        "The all-time window falls back to npm's stats epoch, which moves no figure on the page",
    },
  );
  const created = data?.time?.created?.slice(0, 10);
  return created && created > NPM_STATS_EPOCH ? created : NPM_STATS_EPOCH;
}

// Total npm downloads across the package's whole lifetime. npm's point API
// caps each query at 18 months, so we sum consecutive (non-overlapping)
// 17-month windows from the first-publish day up to today. Returns null only
// when no window yields data, so the caller hides the counter just as it would
// on a failed lookup.
//
// A PARTIAL result is the hazard: if some windows answer and others do not,
// the figure is a real
// number that is quietly too small, which is worse than a missing counter
// because it looks trustworthy. `fetchJson` warns once for the endpoint, and
// this adds a second line saying how much of the lifetime is actually counted.
export async function getNpmTotal(pkg: string): Promise<number | null> {
  const label = `all-time npm downloads for ${pkg}`;
  const effect = "The npm counter (and the combined Total) is hidden";
  const today = new Date();
  let windowStart = new Date(await getNpmFirstPublish(pkg));
  let total = 0;
  let windows = 0;
  let missed = 0;
  while (windowStart <= today) {
    const windowEnd = new Date(windowStart);
    windowEnd.setMonth(windowEnd.getMonth() + 17);
    if (windowEnd > today) windowEnd.setTime(today.getTime());
    const url = `https://api.npmjs.org/downloads/point/${isoDay(windowStart)}:${isoDay(windowEnd)}/${pkg}`;
    const data = await fetchJson<{ downloads?: number }>(url, { label, effect });
    windows++;
    if (typeof data?.downloads === "number") {
      total += data.downloads;
    } else {
      missed++;
      if (data) {
        console.warn(
          unexpectedShapeWarning({
            label,
            url,
            effect: "That window contributes nothing to the total",
            detail: "the response carried no numeric `downloads`",
          }),
        );
      }
    }
    windowStart = new Date(windowEnd);
    windowStart.setDate(windowStart.getDate() + 1);
  }
  if (missed === windows) return null;
  if (missed > 0) {
    console.warn(
      `site build: the npm counter for ${pkg} is an UNDERCOUNT. ${missed} of ${windows} ` +
        "17-month windows did not return data, so the figure shown covers only the rest " +
        "of the package's lifetime. Rebuild once npm answers to get the real total.",
    );
  }
  return total;
}

export function formatCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
}
