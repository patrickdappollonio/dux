#!/usr/bin/env node
// Build step and manual refresh (`npm run contributors`) for the homepage
// contributor list: fetches contributors from the GitHub API, resizes each
// avatar, and rewrites src/data/contributors.json and public/contributors/.
//
// The refresh is all-or-nothing: on any failure it leaves the committed snapshot
// untouched and exits 0, so a rate limit never fails a contributor's build. It
// must never fall back quietly, though, so every skipped refresh prints why, in
// the wording shared with the site's own lookups (src/lib/remote-failure.mjs).

import sharp from "sharp";
import { mkdir, writeFile, readdir, rm } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { normalizeContributors, toSnapshot } from "./lib/contributors-data.mjs";
import { degradationWarning, errorReason, emitDegradation } from "../src/lib/remote-failure.mjs";

const REPO = "patrickdappollonio/dux";
const AVATAR_PX = 80; // stored size; displayed at 40px, so 80px stays crisp at 2× DPR
const PAGE_SIZE = 100; // GitHub max per page
const MAX_PAGES = 10; // safety stop (covers up to 1000 contributors)
const REQUEST_TIMEOUT_MS = 10000;

const here = dirname(fileURLToPath(import.meta.url));
const websiteRoot = resolve(here, "..");
const contributorsDir = resolve(websiteRoot, "public", "contributors");
const jsonPath = resolve(websiteRoot, "src", "data", "contributors.json");

// GitHub API headers, with a token when one is available (CI sets GH_TOKEN) to
// lift the unauthenticated rate limit. Mirrors src/lib/remote-json.ts.
function githubHeaders() {
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  return {
    "User-Agent": "getdux-site-build",
    Accept: "application/vnd.github+json",
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  };
}

// What the site loses when this script gives up, stated once so every warning
// below says the same thing.
const EFFECT = "The committed contributor snapshot is kept unchanged";

function hasToken() {
  return Boolean(process.env.GH_TOKEN || process.env.GITHUB_TOKEN);
}

// The timeout covers reading the body too: `read` consumes the response while
// the abort timer is still armed, so a stalled body cannot hang the build past
// the deadline. Null on any failure, with the status or reason printed.
async function fetchWithTimeout(url, headers, read, label) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    const res = await fetch(url, { headers, signal: controller.signal });
    if (!res.ok) {
      emitDegradation(
        degradationWarning({
          label,
          url,
          effect: EFFECT,
          status: res.status,
          statusText: res.statusText,
          hasToken: hasToken(),
        }),
      );
      return null;
    }
    return await read(res);
  } catch (e) {
    const reason = errorReason(e, `timed out after ${REQUEST_TIMEOUT_MS / 1000}s`);
    emitDegradation(
      degradationWarning({
        label,
        url,
        effect: EFFECT,
        reason,
        hasToken: hasToken(),
      }),
    );
    return null;
  } finally {
    clearTimeout(timer);
  }
}

// Pull every page of contributors. Returns the combined raw records, or null if
// any page fails so the caller can fall back to the committed snapshot.
async function fetchContributors() {
  const headers = githubHeaders();
  const all = [];
  for (let page = 1; page <= MAX_PAGES; page++) {
    const url = `https://api.github.com/repos/${REPO}/contributors?per_page=${PAGE_SIZE}&page=${page}`;
    const batch = await fetchWithTimeout(
      url,
      headers,
      (r) => r.json(),
      `the contributor list for ${REPO} (page ${page})`,
    );
    if (!Array.isArray(batch)) return null; // request failed or unexpected shape
    all.push(...batch);
    if (batch.length < PAGE_SIZE) return all; // short page → last page reached
  }
  // Every page was full and we hit the cap: the list is probably truncated.
  console.warn(
    `fetch-contributors: hit the ${MAX_PAGES}-page cap (${all.length} contributors); the list may be truncated.`,
  );
  return all;
}

// Ask GitHub for 160px (2× the stored AVATAR_PX) so the downscale to AVATAR_PX
// stays crisp; sharp then resizes it to the stored size below.
function avatarSourceUrl(avatarUrl) {
  const sep = avatarUrl.includes("?") ? "&" : "?";
  return `${avatarUrl}${sep}s=${AVATAR_PX * 2}`;
}

// Download one avatar and re-encode it to a square AVATAR_PX PNG. Returns the
// PNG buffer, or null on any failure (network, decode, resize).
async function downloadAvatar(avatarUrl, login) {
  const body = await fetchWithTimeout(
    avatarSourceUrl(avatarUrl),
    { "User-Agent": "getdux-site-build" },
    (r) => r.arrayBuffer(),
    `the avatar for ${login}`,
  );
  if (!body) return null;
  try {
    return await sharp(Buffer.from(body))
      .resize(AVATAR_PX, AVATAR_PX, { fit: "cover" })
      .png()
      .toBuffer();
  } catch (e) {
    console.warn(
      `fetch-contributors: could not process avatar ${avatarUrl} (${e?.message ?? e})`,
    );
    return null;
  }
}

// Remove avatar PNGs no longer in the contributor set. `keepLogins` is lowercased
// and the comparison is case-insensitive: GitHub logins are, so a casing change
// between runs must not delete the file the new snapshot references.
async function pruneStaleAvatars(keepLogins) {
  let entries;
  try {
    entries = await readdir(contributorsDir);
  } catch {
    return; // directory not created yet, nothing to prune
  }
  for (const name of entries) {
    if (!name.endsWith(".png")) continue;
    const login = name.slice(0, -".png".length).toLowerCase();
    if (!keepLogins.has(login)) {
      await rm(resolve(contributorsDir, name), { force: true });
    }
  }
}

async function main() {
  const raw = await fetchContributors();
  if (!raw) {
    // The specific reason (status, rate limit, connectivity) was already
    // printed by fetchWithTimeout; this line only names the consequence.
    console.warn(
      `fetch-contributors: the contributor list could not be refreshed (see the line above). ${EFFECT}, so the homepage strip shows whoever was in it at the last successful refresh.`,
    );
    return;
  }

  const contributors = normalizeContributors(raw);
  if (contributors.length === 0) {
    console.warn(
      "fetch-contributors: the GitHub API answered successfully but no usable contributor " +
        "records came back (every entry was a bot or failed validation). That is a " +
        `response-shape problem, not rate limiting, so retrying will not help. ${EFFECT}.`,
    );
    return;
  }

  // Download everything first. A single failure aborts the whole refresh so the
  // committed snapshot is never replaced with a partial one.
  const avatars = [];
  for (const c of contributors) {
    const png = await downloadAvatar(c.avatarUrl, c.login);
    if (!png) {
      console.warn(
        `fetch-contributors: the refresh is all-or-nothing, so one missing avatar (${c.login}) ` +
          "abandons the whole refresh rather than writing a partial snapshot. " +
          `${EFFECT} and the build continues.`,
      );
      return;
    }
    avatars.push({ login: c.login, png });
  }

  // All avatars are in hand. Now commit the refresh.
  await mkdir(contributorsDir, { recursive: true });
  for (const { login, png } of avatars) {
    await writeFile(resolve(contributorsDir, `${login}.png`), png);
  }
  await pruneStaleAvatars(new Set(contributors.map((c) => c.login.toLowerCase())));

  await mkdir(dirname(jsonPath), { recursive: true });
  await writeFile(jsonPath, `${JSON.stringify(toSnapshot(contributors), null, 2)}\n`);

  console.log(
    `fetch-contributors: wrote ${contributors.length} contributors and avatars.`,
  );
}

// A bad refresh falls back to the committed snapshot, so any unexpected error
// (disk full, permission denied) warns and exits 0 to keep the `&&` chain going.
await main().catch((e) => {
  console.warn(
    `fetch-contributors: unexpected local error (${e?.message ?? e}). This is not a network ` +
      `or rate-limit problem: it happened after the data was in hand, so look at disk space ` +
      `and permissions under website/public/contributors. ${EFFECT} and the build continues.`,
  );
});
