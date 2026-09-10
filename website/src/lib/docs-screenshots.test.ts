import { createRequire } from "node:module";
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

// Both Markdown flavours the docs directory carries: a page that illustrates
// itself from an .mdx file counts as showing its screenshot.
function docsText(): string {
  return readdirSync(docsDir)
    .filter((name) => name.endsWith(".md") || name.endsWith(".mdx"))
    .map((name) => readFileSync(resolve(docsDir, name), "utf8"))
    .join("\n");
}

// The scenes are CommonJS run by node, not modules this suite's bundler owns, so
// they are loaded the way reshoot.sh loads them. Loading them for real is the
// point: a scene whose `file` disagrees with its own name would quietly
// overwrite a different picture, and reading the source for a substring would
// not notice a name built at runtime.
const requireScene = createRequire(import.meta.url);
const sceneModule = (stem: string): unknown =>
  requireScene(resolve(scenesDir, `${stem}.js`)) as unknown;

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

  it("names its own file in every scene", () => {
    const wrong = scenes
      .map((name) => name.replace(/\.js$/, ""))
      .filter((stem) => {
        const mod = sceneModule(stem) as { file?: unknown };
        return mod.file !== `${stem}.png`;
      });
    expect(wrong).toEqual([]);
  });

  // The two shapes reshoot.sh knows how to drive. A scene that is neither is one
  // the tool will skip or misread, which is a picture nobody can regenerate.
  it("exports one of the two documented scene shapes", () => {
    const malformed: string[] = [];
    for (const file of scenes) {
      const stem = file.replace(/\.js$/, "");
      const mod = sceneModule(stem) as Record<string, unknown>;
      if (typeof mod === "function") {
        // A terminal UI journey: the function tui-shot.sh runs, with the grid
        // and theme it is captured at hung off it.
        const journey = mod as unknown as Record<string, unknown>;
        const ok =
          typeof journey.cols === "number" &&
          typeof journey.rows === "number" &&
          typeof journey.theme === "string";
        if (!ok) malformed.push(`${stem} (terminal UI scene missing cols/rows/theme)`);
        continue;
      }
      const ok = typeof mod.viewport === "string" && typeof mod.shoot === "function";
      if (!ok) malformed.push(`${stem} (browser scene missing viewport/shoot)`);
    }
    expect(malformed).toEqual([]);
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
