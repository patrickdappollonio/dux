// Build-time lookup for an optional PNG in `public/`.
//
// Why this exists: the web-UI section is built to receive REAL screenshots that
// do not exist in the repository yet. A hardcoded `<img src>` would ship a
// broken image reference until someone remembers to add the file. So the markup
// asks whether the file is there, and renders the figure only when it is.
//
// Dimensions are read from the PNG header rather than written down, so `width`
// and `height` always describe the file that is actually on disk. That matters
// twice: a wrong ratio causes layout shift, and a capture replaced at a
// different resolution would otherwise silently keep the old numbers.
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

// Resolved from the working directory, NOT from `import.meta.url`, and that is
// deliberate: Astro bundles this module into `dist/.prerender/chunks/` before
// running it, so at build time `import.meta.url` is the chunk's path and a
// module-relative `../../public` resolves inside `dist/`. Measured, not guessed;
// the first version of this file did exactly that and silently found nothing.
//
// The working directory is safe to rely on here because it is how Astro located
// this project at all: `astro build` finds `astro.config.mjs`, `src/` and
// `public/` relative to it. If it somehow is not the project root, `publicPng`
// finds no `public/` directory and every lookup returns null, so the page
// degrades to "no screenshots" rather than rendering a broken reference.
const publicDir = resolve(process.cwd(), "public");

export interface PublicImage {
  /** Site-absolute URL of the PNG. */
  src: string;
  /**
   * Site-absolute URL of the `.webp` sibling, or null when it is absent.
   * `scripts/generate-webp.mjs` writes one next to every raster file in
   * `public/` during `npm run prep`, which runs before `astro build`.
   */
  webp: string | null;
  width: number;
  height: number;
}

const PNG_MAGIC = "89504e470d0a1a0a";

/**
 * Intrinsic size of a PNG, read from its IHDR chunk: an 8-byte signature, a
 * 4-byte chunk length, the 4-byte `IHDR` tag, then width and height as
 * big-endian uint32s at offsets 16 and 20.
 *
 * Returns null for anything that is not a PNG, so a mis-saved JPEG with a
 * `.png` name is treated as "no image" instead of being rendered at a
 * nonsensical size.
 */
export function pngSize(bytes: Buffer): { width: number; height: number } | null {
  if (bytes.length < 24) return null;
  if (bytes.subarray(0, 8).toString("hex") !== PNG_MAGIC) return null;
  if (bytes.subarray(12, 16).toString("latin1") !== "IHDR") return null;
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  return width > 0 && height > 0 ? { width, height } : null;
}

/**
 * Describe `public/<name>.png` if it exists and is a readable PNG, else null.
 * `name` is the bare basename, without directories or extension.
 */
export function publicPng(name: string): PublicImage | null {
  const file = resolve(publicDir, `${name}.png`);
  if (!existsSync(file)) return null;
  let size: { width: number; height: number } | null = null;
  try {
    size = pngSize(readFileSync(file));
  } catch {
    return null;
  }
  if (!size) return null;
  return {
    src: `/${name}.png`,
    webp: existsSync(resolve(publicDir, `${name}.webp`)) ? `/${name}.webp` : null,
    ...size,
  };
}
