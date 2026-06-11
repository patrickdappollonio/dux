#!/usr/bin/env node
// Build step: write a `.webp` sibling next to every raster image (png/jpg/jpeg)
// under `public/`, so the <picture> sources emitted by rehype-prose-images can
// always resolve. Run before `astro build` (which copies `public/` into the
// final output). Converting the handful of chrome images (favicons, og.png)
// alongside content images is harmless — those are referenced by extension in
// <link>/<meta>, so their unused .webp siblings just sit unreferenced.
//
// Kept separate from copy-install.mjs so it can run after it: copy-install
// drops the canonical logo/screenshot into `public/`, then this converts them.

import sharp from "sharp";
import { readdirSync, statSync, existsSync } from "node:fs";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const publicDir = resolve(here, "..", "public");

const RASTER = new Set([".png", ".jpg", ".jpeg"]);

async function convertTree(dir) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      await convertTree(full);
      continue;
    }
    const ext = extname(entry).toLowerCase();
    if (!RASTER.has(ext)) continue;
    const out = `${full.slice(0, -ext.length)}.webp`;
    await sharp(full).webp({ quality: 82 }).toFile(out);
    console.log(`generate-webp: ${entry} -> ${entry.slice(0, -ext.length)}.webp`);
  }
}

if (!existsSync(publicDir)) {
  console.log("generate-webp: no public/ directory, nothing to do.");
} else {
  await convertTree(publicDir);
}
