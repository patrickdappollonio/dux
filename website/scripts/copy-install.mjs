#!/usr/bin/env node
// Copies canonical assets from the repo into public/ as a prebuild step so the
// static site stays a single source of truth:
//   - install.sh  -> served at https://getdux.app/install.sh
//   - dux-logo.png -> the conductor-duck logo used across the site
// Run as a prebuild step in `npm run build`.

import { copyFile, access } from "node:fs/promises";
import { constants } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..");
const publicDir = resolve(here, "..", "public");

const assets = [
  { from: resolve(repoRoot, "install.sh"), to: resolve(publicDir, "install.sh") },
  { from: resolve(repoRoot, "assets", "dux-logo.png"), to: resolve(publicDir, "dux-logo.png") },
  { from: resolve(repoRoot, "assets", "dux-screenshot.svg"), to: resolve(publicDir, "dux-screenshot.svg") },
];

for (const { from, to } of assets) {
  try {
    await access(from, constants.R_OK);
  } catch {
    console.error(`copy-install: cannot read ${from}`);
    process.exit(1);
  }
  await copyFile(from, to);
  console.log(`copy-install: ${from} -> ${to}`);
}
