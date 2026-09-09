// Build-time project stats for the homepage: release-asset downloads (Homebrew
// pulls these too) and all-time npm downloads, fetched once and baked into the
// HTML. Either returns `null` on failure and the caller hides that counter, which
// `fetchJson` announces in the build log so a skipped one is never a mystery.
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

// First-publish day for a package, the lower bound when summing all-time
// downloads. Falls back to the npm stats epoch, which produces the same total
// through a few wasted empty windows, so no figure on the page moves.
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

// npm's point API caps each query at 18 months, so lifetime downloads are summed
// over consecutive non-overlapping windows from the first-publish day. Null only
// when no window yields data, so the caller hides the counter.
//
// A partial result is the hazard: some windows answering and others not gives a
// real number that is quietly too small, which looks more trustworthy than a
// missing counter, so this says how much of the lifetime was actually counted.
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
