// Build-time GitHub star lookup, baked into the HTML so there are no
// client-side API calls. Degrades to `null` (badge omitted) on any failure.
import { fetchJson, githubHeaders } from "./remote-json";

export async function getStars(repo: string): Promise<number | null> {
  const data = await fetchJson<{ stargazers_count?: number }>(
    `https://api.github.com/repos/${repo}`,
    githubHeaders(),
  );
  return typeof data?.stargazers_count === "number"
    ? data.stargazers_count
    : null;
}

export function formatStars(n: number): string {
  if (n < 1000) return String(n);
  return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
}
