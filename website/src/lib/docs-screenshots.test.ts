import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// The loop this closes: a screenshot in the docs, the journey that produces it,
// and the page that shows it. Any one of the three going missing is a picture
// nobody can reshoot, a journey nobody runs, or an image nobody sees.
const repoRoot = resolve(process.cwd(), "..");
const screensDir = resolve(process.cwd(), "public/screens");
const scenesDir = resolve(repoRoot, "tools/preview-env/screens/scenes");
const docsDir = resolve(process.cwd(), "docs");

// Deliberately empty, and it must stay that way. A screenshot no page shows is
// either a page that lost its illustration or an image that should be deleted;
// parking it here is how that decision gets forgotten.
const UNREFERENCED_ALLOWLIST: string[] = [];

const screenshots = readdirSync(screensDir)
  .filter((name) => name.endsWith(".png"))
  .sort();

const scenes = readdirSync(scenesDir)
  .filter((name) => name.endsWith(".js"))
  .sort();

function docsText(): string {
  return readdirSync(docsDir)
    .filter((name) => name.endsWith(".md"))
    .map((name) => readFileSync(resolve(docsDir, name), "utf8"))
    .join("\n");
}

describe("docs screenshots", () => {
  it("has screenshots to check", () => {
    expect(screenshots.length).toBeGreaterThan(0);
  });

  it("has a scene named after every screenshot", () => {
    const have = new Set(scenes.map((name) => name.replace(/\.js$/, "")));
    const missing = screenshots
      .map((png) => png.replace(/\.png$/, ""))
      .filter((stem) => !have.has(stem));
    expect(missing).toEqual([]);
  });

  it("has a screenshot for every scene", () => {
    const have = new Set(screenshots.map((name) => name.replace(/\.png$/, "")));
    const orphans = scenes
      .map((name) => name.replace(/\.js$/, ""))
      .filter((stem) => !have.has(stem));
    expect(orphans).toEqual([]);
  });

  // The file a scene names is what reshoot.sh writes, so a scene whose `file`
  // disagrees with its own name would quietly overwrite a different picture.
  it("names its own file in every scene", () => {
    const text = scenes.map((name) => ({
      name,
      source: readFileSync(resolve(scenesDir, name), "utf8"),
    }));
    const wrong = text
      .filter(({ name, source }) => {
        const stem = name.replace(/\.js$/, "");
        return !source.includes(`"${stem}.png"`);
      })
      .map(({ name }) => name);
    expect(wrong).toEqual([]);
  });

  it("shows every screenshot on a docs page", () => {
    const text = docsText();
    const unreferenced = screenshots.filter((png) => !text.includes(`/screens/${png}`));
    expect(unreferenced).toEqual(UNREFERENCED_ALLOWLIST);
  });

  it("keeps the unreferenced allowlist empty", () => {
    expect(UNREFERENCED_ALLOWLIST).toEqual([]);
  });

  it("has the file every docs page asks for", () => {
    const have = new Set(screenshots);
    const referenced = [...docsText().matchAll(/\/screens\/([A-Za-z0-9._-]+\.png)/g)].map(
      (match) => match[1],
    );
    const broken = [...new Set(referenced)].filter((png) => !have.has(png)).sort();
    expect(broken).toEqual([]);
  });
});
