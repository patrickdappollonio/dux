import { createRequire } from "node:module";
import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { deflateSync } from "node:zlib";
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
// Block comments, and line comments that start their own line. Deliberately not
// a parser: a `//` in the middle of a line is usually inside a URL, and leaving
// those alone costs nothing here.
function withoutComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^[ \t]*\/\/.*$/gm, "");
}

const requireScene = createRequire(import.meta.url);
const sceneModule = (stem: string): unknown =>
  requireScene(resolve(scenesDir, `${stem}.js`)) as unknown;

// The one rule that says a picture is empty, tested on pictures built here: the
// live case it exists for (a pure black frame written and reported as a success)
// is not something a test can ask a browser for on demand.
describe("the empty-capture rule", () => {
  const ink = requireScene(
    resolve(repoRoot, "tools/preview-env/screens/ink.js"),
  ) as {
    captureProblem: (png: Buffer) => string | null;
    decodePng: (png: Buffer) => unknown;
  };

  // A minimal PNG: 8-bit RGB, one filter byte per row, no interlacing, which is
  // the shape Chromium writes and all this reader accepts.
  function png(width: number, height: number, paint: (x: number, y: number) => number[]): Buffer {
    const stride = width * 3;
    const raw = Buffer.alloc(height * (stride + 1));
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        const [r, g, b] = paint(x, y);
        const at = y * (stride + 1) + 1 + x * 3;
        raw[at] = r;
        raw[at + 1] = g;
        raw[at + 2] = b;
      }
    }
    return assemble(width, height, deflateSync(raw));
  }

  function assemble(width: number, height: number, idat: Buffer): Buffer {
    const chunk = (type: string, body: Buffer): Buffer => {
      const head = Buffer.alloc(8);
      head.writeUInt32BE(body.length, 0);
      head.write(type, 4, "ascii");
      const tail = Buffer.alloc(4);
      tail.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), body])), 0);
      return Buffer.concat([head, body, tail]);
    };
    const ihdr = Buffer.alloc(13);
    ihdr.writeUInt32BE(width, 0);
    ihdr.writeUInt32BE(height, 4);
    ihdr[8] = 8;
    ihdr[9] = 2;
    return Buffer.concat([
      Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
      chunk("IHDR", ihdr),
      chunk("IDAT", idat),
      chunk("IEND", Buffer.alloc(0)),
    ]);
  }

  // The reader never checks these, so any value would do; a real one keeps the
  // fixtures openable by anything else that wants to look at them.
  function crc32(bytes: Buffer): number {
    let crc = 0xffffffff;
    for (const byte of bytes) {
      crc ^= byte;
      for (let bit = 0; bit < 8; bit++) {
        crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
      }
    }
    return (crc ^ 0xffffffff) >>> 0;
  }

  it("refuses a frame of one flat colour", () => {
    const black = png(64, 64, () => [0, 0, 0]);
    expect(ink.captureProblem(black)).toMatch(/one flat colour/);
  });

  it("refuses a frame with almost nothing in it", () => {
    // A four-pixel mark on a 200-square frame, which is roughly what a lone
    // cursor on an otherwise empty pane comes to: 0.04% against the 0.1% floor.
    const nearlyEmpty = png(200, 200, (x, y) =>
      x < 4 && y < 4 ? [255, 255, 255] : [10, 10, 10],
    );
    expect(ink.captureProblem(nearlyEmpty)).toMatch(/the capture is empty/);
  });

  it("accepts a frame with something in it", () => {
    const painted = png(64, 64, (_x, y) => (y % 4 === 0 ? [255, 255, 255] : [10, 10, 10]));
    expect(ink.captureProblem(painted)).toBeNull();
  });

  it("refuses a truncated stream rather than reading it as black rows", () => {
    const full = png(64, 64, (_x, y) => (y % 4 === 0 ? [255, 255, 255] : [10, 10, 10]));
    const short = assemble(64, 64, deflateSync(Buffer.alloc(64 * (64 * 3 + 1) - 900)));
    expect(() => ink.decodePng(full)).not.toThrow();
    expect(() => ink.decodePng(short)).toThrow(/truncated/);
  });
});

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

  // A scene that drives itself into the wrong state still produces a perfectly
  // valid PNG, and the tool reported success for three of those at once. Every
  // scene therefore states what its own picture must contain, and a new scene
  // cannot ship without saying so.
  it("guards every browser scene against writing the wrong picture", () => {
    const guardless: string[] = [];
    for (const file of scenes) {
      const stem = file.replace(/\.js$/, "");
      if (typeof sceneModule(stem) === "function") continue;
      // Comments first: every scene explains its guards in prose above them, and
      // a test that reads a scene's comments as calls passes a scene that only
      // talks about guarding. This is a cheap presence check either way; the
      // real gate is in run.js, which refuses a scene that asked nothing at
      // runtime and cannot be talked out of it.
      const source = withoutComments(readFileSync(resolve(scenesDir, file), "utf8"));
      // Either style of call site: a destructured `expectNoCover(page)` or a
      // qualified `lib.expectRows(page, ...)`.
      if (!/\bexpect[A-Z]\w*\s*\(/.test(source)) guardless.push(stem);
    }
    expect(guardless).toEqual([]);
  });

  it("names what every terminal UI scene must show", () => {
    const unstated: string[] = [];
    for (const file of scenes) {
      const stem = file.replace(/\.js$/, "");
      const mod = sceneModule(stem);
      if (typeof mod !== "function") continue;
      const expectText = (mod as unknown as { expectText?: unknown }).expectText;
      const ok =
        Array.isArray(expectText) &&
        expectText.length > 0 &&
        expectText.every((needle) => typeof needle === "string" && needle.length > 0);
      if (!ok) unstated.push(stem);
    }
    expect(unstated).toEqual([]);
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
