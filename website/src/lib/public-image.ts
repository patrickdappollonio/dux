// Build-time lookup for an optional PNG in `public/`: the markup asks whether the
// file is there and renders its figure only when it is, so a screenshot that has
// not been added yet is skipped rather than shipping a broken image reference.
//
// Dimensions are read from the PNG header rather than written down, so they
// always describe the file on disk: a wrong ratio causes layout shift, and a
// capture replaced at a different resolution would otherwise keep the old numbers.
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

// Resolved from the working directory, never `import.meta.url`: Astro bundles
// this module into `dist/.prerender/chunks/`, so a module-relative `../../public`
// resolves inside `dist/` and silently finds nothing. The working directory is
// how Astro located the project at all, and if it is somehow not the project root
// every lookup returns null, so the page degrades to "no screenshots".
const publicDir = resolve(process.cwd(), "public");

export interface PublicImage {
  /** Site-absolute URL of the PNG. */
  src: string;
  /**
   * Site-absolute URL of the `.webp` sibling, or null when it is absent.
   * `scripts/generate-webp.mjs` writes one during `npm run prep`.
   */
  webp: string | null;
  width: number;
  height: number;
}

const PNG_MAGIC = "89504e470d0a1a0a";

/**
 * Intrinsic size of a PNG, read from its IHDR chunk: width and height are
 * big-endian uint32s at offsets 16 and 20. Null for anything that is not a PNG,
 * so a mis-saved JPEG with a `.png` name is treated as "no image".
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
