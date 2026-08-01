#!/usr/bin/env node
// Build step / manual refresh for the homepage contributor list.
//
// Fetches the repo's contributors from the GitHub API, downloads and resizes
// each avatar, and rewrites the committed snapshot:
//   - src/data/contributors.json     (login + profile URL + local avatar path)
//   - public/contributors/<login>.png (80px avatar, served at /contributors/…)
//
// Strategy: fresh-with-vendored-fallback. On a fully successful refresh it
// overwrites the tracked files (commit the diff to publish). On ANY failure
// (network blocked, rate limited, a single avatar that won't download) it
// leaves the committed snapshot untouched and exits 0, so a flaky API never
// fails the build: the previously committed snapshot is the fallback. The
// refresh is all-or-nothing, so nothing is written until every avatar is in
// hand.
//
// The fallback is deliberate. GitHub allows roughly 60 unauthenticated API
// requests an hour per IP and this script pages through the contributor list,
// so being rate limited here is ordinary rather than exceptional, and stopping
// a contributor's build over someone else's quota would be the wrong trade.
// What it must never do is fall back QUIETLY: every skipped refresh prints why,
// including the plain statement that a 403 from the GitHub API is rate limiting
// rather than a permissions problem, so a stale snapshot is a reported outcome.
// The wording is shared with the site's own build-time lookups (see
// src/lib/remote-failure.mjs).
//
// Run manually with `npm run contributors`; runs automatically in `npm run
// build` before generate-webp/astro build.

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

// Fetch with a hard timeout that also covers reading the body: `read` consumes
// the response (e.g. r.json() / r.arrayBuffer()) while the abort timer is still
// armed, so a stalled body can't hang the build past the deadline. Returns what
// `read` produced, or null on any failure, explaining the HTTP status or error
// reason so a kept-stale snapshot is diagnosable straight from the build log.
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

// Remove avatar PNGs that are no longer in the current contributor set, so a
// contributor who drops off the list doesn't leave a stale file behind.
// `keepLogins` holds lowercased logins and the comparison is case-insensitive:
// GitHub logins are case-insensitive, so a casing change between runs must not
// delete the file the new snapshot references (notably on case-insensitive
// macOS filesystems, where it is the very same file).
async function pruneStaleAvatars(keepLogins) {
  let entries;
  try {
    entries = await readdir(contributorsDir);
  } catch {
    return; // directory not created yet — nothing to prune
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

  // All avatars are in hand — now commit the refresh.
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

// Any unexpected error (disk full, permission denied, etc.) must not fail the
// build — the whole point is that a bad refresh falls back to the committed
// snapshot. Catch it, warn, and exit 0 so the `&&` build chain continues.
await main().catch((e) => {
  console.warn(
    `fetch-contributors: unexpected local error (${e?.message ?? e}). This is not a network ` +
      `or rate-limit problem: it happened after the data was in hand, so look at disk space ` +
      `and permissions under website/public/contributors. ${EFFECT} and the build continues.`,
  );
});
