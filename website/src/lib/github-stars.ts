// Build-time GitHub star lookup, baked into the HTML so there are no client-side
// API calls. Degrades to `null` on any failure, because a third party's rate
// limiter must not break a contributor's build; `fetchJson` announces the skip.
import { fetchJson, githubHeaders } from "./remote-json";
// @ts-expect-error - plain .mjs helper, shared with the plain-Node build scripts
import { unexpectedShapeWarning } from "./remote-failure.mjs";

export async function getStars(repo: string): Promise<number | null> {
  const url = `https://api.github.com/repos/${repo}`;
  const label = `the GitHub star count for ${repo}`;
  const effect = "The star badge is omitted";
  const data = await fetchJson<{ stargazers_count?: number }>(url, {
    label,
    effect,
    headers: githubHeaders(),
  });
  if (!data) return null;
  if (typeof data.stargazers_count !== "number") {
    console.warn(
      unexpectedShapeWarning({
        label,
        url,
        effect,
        detail: "the record carried no `stargazers_count`",
      }),
    );
    return null;
  }
  return data.stargazers_count;
}

export function formatStars(n: number): string {
  if (n < 1000) return String(n);
  return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
}
