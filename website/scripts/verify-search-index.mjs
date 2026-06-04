#!/usr/bin/env node
// Post-build guard for the docs search index. Run after `astro build` (the
// Pagefind index is generated into dist/pagefind/). It proves three things so a
// future change can't silently break or re-scope search:
//
//   1. The Pagefind runtime (pagefind.js) actually shipped.
//   2. The docs are in the index (at least one /docs URL).
//   3. The marketing homepage is NOT in the index (docs-only scope holds).
//
// Pagefind stores each indexed page as a gzip-compressed JSON "fragment"; we
// decompress them to read the indexed URLs without booting the WASM runtime.

import { readdirSync, readFileSync, existsSync, statSync } from "node:fs";
import { gunzipSync } from "node:zlib";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const distDir = resolve(here, "..", "dist");
const pagefindDir = join(distDir, "pagefind");

function fail(message) {
  console.error(`verify-search-index: ${message}`);
  process.exit(1);
}

// 1. The runtime must exist, or the modal can never load.
const runtime = join(pagefindDir, "pagefind.js");
if (!existsSync(runtime)) {
  fail(
    `${runtime} is missing. Did the astro-pagefind integration run during \`astro build\`?`,
  );
}

// Recursively collect every Pagefind fragment file (*.pf_fragment).
function findFragments(dir) {
  if (!existsSync(dir)) return [];
  const out = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      out.push(...findFragments(full));
    } else if (entry.endsWith(".pf_fragment")) {
      out.push(full);
    }
  }
  return out;
}

const fragments = findFragments(pagefindDir);
if (fragments.length === 0) {
  fail(
    "no indexed pages found (no *.pf_fragment files). Is `data-pagefind-body` present on the docs <article>?",
  );
}

// Decode each fragment's indexed URL. A decompressed fragment is a short
// `pagefind_*` marker followed by a JSON object, so we parse from the first
// `{` to the last `}`.
const urls = [];
for (const file of fragments) {
  try {
    const raw = gunzipSync(readFileSync(file)).toString("utf8");
    const start = raw.indexOf("{");
    const end = raw.lastIndexOf("}");
    if (start === -1 || end === -1) {
      fail(`fragment ${file} has no JSON payload.`);
    }
    const json = JSON.parse(raw.slice(start, end + 1));
    if (typeof json.url === "string") urls.push(json.url);
  } catch (error) {
    fail(`could not decode fragment ${file}: ${error.message}`);
  }
}

const norm = (u) => u.replace(/index\.html$/i, "").replace(/\.html$/i, "");
const isDocs = (u) => norm(u).includes("/docs");
const isHomepage = (u) => {
  const p = norm(u).split(/[?#]/)[0].replace(/\/+$/, "");
  return p === "" || p === "/";
};

// 2. The docs must be indexed.
if (!urls.some(isDocs)) {
  fail(
    `no /docs pages are indexed. Indexed URLs: ${JSON.stringify(urls)}`,
  );
}

// 3. The homepage must NOT be indexed (docs-only scope).
const leaked = urls.filter(isHomepage);
if (leaked.length > 0) {
  fail(
    `the homepage leaked into the search index (${JSON.stringify(
      leaked,
    )}). Search scope must stay docs-only — only docs pages should carry \`data-pagefind-body\`.`,
  );
}

console.log(
  `verify-search-index: OK — ${urls.length} docs page(s) indexed, homepage excluded.`,
);
