// Regenerates the recolorable-duck favicon assets from public/dux-logo.png: the traced cutout
// silhouette written into DUCK_PATH (`src/lib/favicon.ts`), and public/favicon.png.
//
// The cutout is traced from the logo's ink, not a plain alpha silhouette: the beak, eyes and
// bowtie are dark ink on an opaque body, so an alpha threshold yields a featureless blob. The
// mask is black only where a pixel is both opaque and bright, the lit body, and white
// everywhere else, so potrace's even-odd path reads the ink as negative space.
//
// A manual tool: `jimp` and `potrace` are deliberately not project dependencies. Install them
// for a regeneration and do not save them:
//
//   npm i --no-save jimp potrace
//   node scripts/gen-duck-favicon.mjs
//
// potrace's output is nondeterministic, so a run never reproduces the committed DUCK_PATH byte
// for byte. Validate the emitted duck visually before committing: `duckPath.test.ts` guards
// only against gross corruption, never staleness or a match against a fresh run.

import { createRequire } from "node:module"
import { fileURLToPath } from "node:url"
import { dirname, join } from "node:path"
import { readFile, writeFile } from "node:fs/promises"
import { Jimp } from "jimp"

const require = createRequire(import.meta.url)
const potrace = require("potrace")

const here = dirname(fileURLToPath(import.meta.url))
const webRoot = join(here, "..")
const logoPath = join(webRoot, "public", "dux-logo.png")
const faviconPngPath = join(webRoot, "public", "favicon.png")
const faviconTsPath = join(webRoot, "src", "lib", "favicon.ts")

const CANVAS = 512 // the mask/trace canvas (matches the SVG viewBox)
const FAVICON_SIZE = 128 // the emitted default PNG size
const ALPHA_MIN = 128 // opaque-enough to be duck body
const LUMA_MIN = 70 // bright-enough to be lit body (dark ink falls below)

function luminance(r, g, b) {
  return 0.299 * r + 0.587 * g + 0.114 * b
}

// The tight alpha bounding box of the logo, so the duck fills the square canvas.
function alphaBounds(img) {
  const { width, height, data } = img.bitmap
  let minX = width,
    minY = height,
    maxX = 0,
    maxY = 0
  img.scan(0, 0, width, height, (x, y, idx) => {
    if (data[idx + 3] > 10) {
      if (x < minX) minX = x
      if (y < minY) minY = y
      if (x > maxX) maxX = x
      if (y > maxY) maxY = y
    }
  })
  return { x: minX, y: minY, w: maxX - minX + 1, h: maxY - minY + 1 }
}

// Autocrop to the duck, then centre it on a transparent square so the aspect
// ratio survives the resize.
function squareDuck(img) {
  const b = alphaBounds(img)
  const cropped = img.clone().crop({ x: b.x, y: b.y, w: b.w, h: b.h })
  const side = Math.max(b.w, b.h)
  const square = new Jimp({ width: side, height: side, color: 0x00000000 })
  square.composite(
    cropped,
    Math.floor((side - b.w) / 2),
    Math.floor((side - b.h) / 2),
  )
  return square
}

async function main() {
  const logo = await Jimp.read(logoPath)
  const square = squareDuck(logo)

  // The full-colour default favicon: the autocropped duck at FAVICON_SIZE.
  const favicon = square.clone().resize({ w: FAVICON_SIZE, h: FAVICON_SIZE })
  await writeFile(faviconPngPath, await favicon.getBuffer("image/png"))

  // The trace mask: black = lit body, white = background + dark ink.
  const duck = square.clone().resize({ w: CANVAS, h: CANVAS })
  const mask = new Jimp({ width: CANVAS, height: CANVAS, color: 0xffffffff })
  duck.scan(0, 0, CANVAS, CANVAS, (x, y, idx) => {
    const d = duck.bitmap.data
    const r = d[idx],
      g = d[idx + 1],
      b = d[idx + 2],
      a = d[idx + 3]
    if (a > ALPHA_MIN && luminance(r, g, b) >= LUMA_MIN) {
      const m = mask.bitmap.data
      m[idx] = 0
      m[idx + 1] = 0
      m[idx + 2] = 0
      m[idx + 3] = 255
    }
  })

  const maskBuf = await mask.getBuffer("image/png")
  const svg = await new Promise((resolve, reject) => {
    potrace.trace(
      maskBuf,
      { turdSize: 40, optTolerance: 0.4, threshold: 128 },
      (err, out) => (err ? reject(err) : resolve(out)),
    )
  })

  const match = svg.match(/ d="([^"]+)"/)
  if (!match) {
    throw new Error("potrace produced no path — check the mask/threshold")
  }
  const duckPath = match[1].trim()
  if (!duckPath.startsWith("M")) {
    throw new Error(`traced path does not start with a moveto: ${duckPath.slice(0, 20)}`)
  }

  // Write the path back into favicon.ts's DUCK_PATH constant. The traced path
  // never contains a double-quote, so the `"[^"]*"` capture is safe.
  const src = await readFile(faviconTsPath, "utf8")
  const next = src.replace(
    /const DUCK_PATH =\n {2}"[^"]*"/,
    `const DUCK_PATH =\n  "${duckPath}"`,
  )
  if (next === src && !src.includes(`"${duckPath}"`)) {
    throw new Error("could not locate the DUCK_PATH constant in favicon.ts")
  }
  await writeFile(faviconTsPath, next)

  console.log(
    `Wrote public/favicon.png (${FAVICON_SIZE}px) and DUCK_PATH (${duckPath.length} chars) into src/lib/favicon.ts`,
  )
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
