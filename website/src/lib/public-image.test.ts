import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { pngSize, publicPng } from "./public-image";

// Same resolution the helper uses: the project root is the working directory.
const publicDir = resolve(process.cwd(), "public");

describe("pngSize", () => {
  // og.png is committed and is the social card, whose size is pinned by the
  // og:image:width/height meta tags in Layout.astro. If the parser and those
  // tags ever disagree, one of them is wrong.
  it("reads the real dimensions of a committed PNG", () => {
    const bytes = readFileSync(resolve(publicDir, "og.png"));
    expect(pngSize(bytes)).toEqual({ width: 1200, height: 630 });
  });

  it("rejects a non-PNG", () => {
    expect(pngSize(Buffer.from("not an image at all, truly"))).toBeNull();
  });

  it("rejects a truncated file", () => {
    expect(pngSize(Buffer.from([0x89, 0x50, 0x4e, 0x47]))).toBeNull();
  });
});

describe("publicPng", () => {
  it("describes a PNG that exists", () => {
    const img = publicPng("og");
    expect(img).not.toBeNull();
    expect(img?.src).toBe("/og.png");
    expect(img?.width).toBe(1200);
  });

  // The reason the helper exists: the web-UI screenshots are added later, and
  // until then the section must render without a broken image reference.
  it("returns null for a missing file rather than throwing", () => {
    expect(publicPng("definitely-not-a-real-asset-name")).toBeNull();
  });
});
