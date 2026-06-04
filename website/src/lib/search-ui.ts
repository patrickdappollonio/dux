// Pure, framework-free logic for the docs search modal. Kept separate from the
// DOM glue in DocsSearch.astro so it can be unit-tested without a browser. The
// `.astro` script imports these helpers and only handles wiring (events, focus,
// rendering).

/** A single sub-result Pagefind returns, anchored to a heading on the page. */
export interface PagefindSubResult {
  title?: string;
  url?: string;
  excerpt?: string;
  anchor?: { id?: string };
}

/** The shape of the object returned by Pagefind's `result.data()`. Only the
 *  fields we consume are typed; Pagefind returns more. */
export interface PagefindData {
  url?: string;
  excerpt?: string;
  meta?: { title?: string };
  sub_results?: PagefindSubResult[];
}

/** A heading-level hit within a page, ready to render as a deep link. */
export interface SearchSubResult {
  title: string;
  url: string;
  excerpt: string;
}

/** A normalized, render-ready search result. */
export interface SearchResult {
  url: string;
  title: string;
  excerpt: string;
  subResults: SearchSubResult[];
}

/**
 * Normalize a built-output URL into a clean site path.
 *
 * Pagefind indexes the generated HTML, so URLs can arrive as
 * `/docs/themes/index.html` or `/docs/themes.html`. We strip the `.html`
 * artifact while preserving any `#anchor` (used by sub-results to deep-link to
 * a heading) and any query string. The result always starts with `/`.
 */
export function normalizeUrl(raw: string | undefined): string {
  if (!raw) return "/";
  // Split off hash and query so we only rewrite the path segment.
  const hashIndex = raw.indexOf("#");
  const hash = hashIndex >= 0 ? raw.slice(hashIndex) : "";
  const withoutHash = hashIndex >= 0 ? raw.slice(0, hashIndex) : raw;
  const queryIndex = withoutHash.indexOf("?");
  const query = queryIndex >= 0 ? withoutHash.slice(queryIndex) : "";
  let path = queryIndex >= 0 ? withoutHash.slice(0, queryIndex) : withoutHash;

  path = path.replace(/index\.html$/i, "").replace(/\.html$/i, "");
  if (!path.startsWith("/")) path = `/${path}`;
  // Collapse any accidental double slashes introduced by stripping.
  path = path.replace(/\/{2,}/g, "/");
  // Keep a trailing slash off unless the path is just the root.
  if (path.length > 1 && path.endsWith("/")) path = path.slice(0, -1);

  return `${path}${query}${hash}`;
}

/** Trim and collapse whitespace in a Pagefind excerpt's surrounding text while
 *  preserving the `<mark>` highlight tags Pagefind injects. */
function cleanExcerpt(excerpt: string | undefined): string {
  return (excerpt ?? "").replace(/\s+/g, " ").trim();
}

/**
 * Convert a raw Pagefind `result.data()` payload into a render-ready
 * `SearchResult`. De-duplicates sub-results by normalized URL (Pagefind can
 * emit the page-top result and a heading result that resolve to the same
 * place) and drops a sub-result that just points back at the page itself.
 *
 * @param data   the object from `await result.data()`
 * @param maxSub maximum heading sub-results to keep (default 3)
 */
export function mapResult(data: PagefindData, maxSub = 3): SearchResult {
  const url = normalizeUrl(data.url);
  const title = (data.meta?.title ?? "").trim() || "Untitled";
  const excerpt = cleanExcerpt(data.excerpt);

  const seen = new Set<string>([url]);
  const subResults: SearchSubResult[] = [];
  for (const sub of data.sub_results ?? []) {
    const subUrl = normalizeUrl(sub.url);
    // Skip sub-results that resolve to the page itself (no anchor) or that we
    // have already added under a different raw form.
    if (seen.has(subUrl)) continue;
    seen.add(subUrl);
    subResults.push({
      title: (sub.title ?? "").trim() || title,
      url: subUrl,
      excerpt: cleanExcerpt(sub.excerpt),
    });
    if (subResults.length >= maxSub) break;
  }

  return { url, title, excerpt, subResults };
}

/**
 * Compute the next selected index for keyboard navigation with wrap-around.
 *
 * @param current the currently selected index (-1 means "nothing selected")
 * @param delta   +1 for down/next, -1 for up/previous
 * @param length  number of selectable items
 * @returns the new index, or -1 when there is nothing to select
 */
export function nextIndex(current: number, delta: number, length: number): number {
  if (length <= 0) return -1;
  // From "nothing selected", ArrowDown picks the first item and ArrowUp the
  // last — the conventional command-palette behavior.
  if (current < 0) return delta > 0 ? 0 : length - 1;
  return (current + delta + length) % length;
}
