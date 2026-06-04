import { describe, it, expect } from "vitest";
import { normalizeUrl, mapResult, nextIndex } from "./search-ui";

describe("normalizeUrl", () => {
  it("strips a trailing index.html", () => {
    expect(normalizeUrl("/docs/themes/index.html")).toBe("/docs/themes");
  });

  it("strips a trailing .html", () => {
    expect(normalizeUrl("/docs/themes.html")).toBe("/docs/themes");
  });

  it("preserves an anchor for heading deep links", () => {
    expect(normalizeUrl("/docs/themes/index.html#opaline-format")).toBe(
      "/docs/themes#opaline-format",
    );
  });

  it("preserves a query string", () => {
    expect(normalizeUrl("/docs/themes.html?x=1")).toBe("/docs/themes?x=1");
  });

  it("ensures a leading slash", () => {
    expect(normalizeUrl("docs/themes")).toBe("/docs/themes");
  });

  it("collapses accidental double slashes", () => {
    expect(normalizeUrl("/docs//themes/index.html")).toBe("/docs/themes");
  });

  it("leaves the root path intact", () => {
    expect(normalizeUrl("/index.html")).toBe("/");
    expect(normalizeUrl("/")).toBe("/");
  });

  it("falls back to root for empty input", () => {
    expect(normalizeUrl(undefined)).toBe("/");
    expect(normalizeUrl("")).toBe("/");
  });
});

describe("mapResult", () => {
  it("normalizes the url, title, and excerpt", () => {
    const result = mapResult({
      url: "/docs/themes/index.html",
      excerpt: "  Pick a   <mark>theme</mark> for dux.  ",
      meta: { title: "Themes" },
      sub_results: [],
    });
    expect(result).toEqual({
      url: "/docs/themes",
      title: "Themes",
      excerpt: "Pick a <mark>theme</mark> for dux.",
      subResults: [],
    });
  });

  it("falls back to 'Untitled' when no title is present", () => {
    expect(mapResult({ url: "/docs/x.html" }).title).toBe("Untitled");
  });

  it("keeps heading sub-results and deep-links them", () => {
    const result = mapResult({
      url: "/docs/themes.html",
      meta: { title: "Themes" },
      sub_results: [
        {
          title: "Opaline format",
          url: "/docs/themes.html#opaline-format",
          excerpt: "TOML <mark>theme</mark> files.",
        },
      ],
    });
    expect(result.subResults).toEqual([
      {
        title: "Opaline format",
        url: "/docs/themes#opaline-format",
        excerpt: "TOML <mark>theme</mark> files.",
      },
    ]);
  });

  it("drops a sub-result that resolves to the page itself", () => {
    const result = mapResult({
      url: "/docs/themes.html",
      meta: { title: "Themes" },
      sub_results: [
        { title: "Themes", url: "/docs/themes/index.html", excerpt: "top" },
        { title: "Opaline", url: "/docs/themes.html#opaline", excerpt: "x" },
      ],
    });
    expect(result.subResults.map((s) => s.url)).toEqual([
      "/docs/themes#opaline",
    ]);
  });

  it("de-duplicates sub-results by normalized url", () => {
    const result = mapResult({
      url: "/docs/a.html",
      meta: { title: "A" },
      sub_results: [
        { title: "S", url: "/docs/a.html#s", excerpt: "1" },
        { title: "S again", url: "/docs/a/index.html#s", excerpt: "2" },
      ],
    });
    expect(result.subResults).toHaveLength(1);
  });

  it("caps sub-results at maxSub", () => {
    const result = mapResult(
      {
        url: "/docs/a.html",
        meta: { title: "A" },
        sub_results: [
          { title: "1", url: "/docs/a.html#1", excerpt: "" },
          { title: "2", url: "/docs/a.html#2", excerpt: "" },
          { title: "3", url: "/docs/a.html#3", excerpt: "" },
          { title: "4", url: "/docs/a.html#4", excerpt: "" },
        ],
      },
      2,
    );
    expect(result.subResults).toHaveLength(2);
  });
});

describe("nextIndex", () => {
  it("returns -1 when there are no items", () => {
    expect(nextIndex(-1, 1, 0)).toBe(-1);
    expect(nextIndex(3, -1, 0)).toBe(-1);
  });

  it("selects the first item from nothing on ArrowDown", () => {
    expect(nextIndex(-1, 1, 3)).toBe(0);
  });

  it("selects the last item from nothing on ArrowUp", () => {
    expect(nextIndex(-1, -1, 3)).toBe(2);
  });

  it("moves forward within bounds", () => {
    expect(nextIndex(0, 1, 3)).toBe(1);
  });

  it("wraps forward past the end", () => {
    expect(nextIndex(2, 1, 3)).toBe(0);
  });

  it("wraps backward past the start", () => {
    expect(nextIndex(0, -1, 3)).toBe(2);
  });
});
