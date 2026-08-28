const fs = require("fs")
const path = require("path")
const puppeteer = require("puppeteer-core")

const [ansiPath, pngPath, colsArg, rowsArg, fontPath, cropArg] = process.argv.slice(2)
const cols = Number(colsArg)
const rows = Number(rowsArg)

if (!ansiPath || !pngPath || !Number.isInteger(cols) || !Number.isInteger(rows) || !fontPath) {
  console.error("usage: node tui-shot.js <input.ansi> <output.png> <cols> <rows> <font.woff2> [crop]")
  process.exit(64)
}
if (cropArg && cropArg !== "sidebar") {
  console.error(`unknown crop mode: ${cropArg} (the only mode is "sidebar")`)
  process.exit(64)
}

const chrome = process.env.CHROME
if (!chrome) {
  console.error("CHROME must point to a Chromium executable")
  process.exit(64)
}

const DEVICE_SCALE = 2
// The capture's own frame, in CSS pixels, around the terminal grid. A CROP does
// not get one: everything outside a crop's cell rect is other cells of the same
// screen, not background, so a margin would drag in half a row of the header
// and half a column of the next pane. A crop lands flush on its cell edges.
const FRAME = 18

// The preferred face. Commercial and NOT vendored: the harness names it and
// lets the host resolve it, so a machine without it simply falls back to the
// bundled Dux Mono stack (a warning, never a failure). The exact family name
// is what `fc-list` reports.
const PREFERRED_FAMILY = "MonoLisa Nerd Font Mono"
const DUX_STACK = '"Dux Mono Symbols", "Dux Mono", "Dux Mono Fill", monospace'
const FONT_STACK = `"${PREFERRED_FAMILY}", ${DUX_STACK}`

// Why 12.5 and not a round number: xterm's DOM renderer lays every cell out at
// the font's own advance, unrounded. At any size whose advance does not land on
// a WHOLE DEVICE pixel, Chromium antialiases the left and right edge of every
// glyph box, and a row of `▄`/`▀`/`█` comes out as a comb of ~60%-ink seams,
// one per cell boundary. Measured at deviceScaleFactor 2, font size 14: cell
// advance 16.786 device px, 19 seam pixels across 24 cells, darkest seam 153 of
// 255. Neither font fixes that on its own; the fractional advance does it.
//
// Dux Mono's advance is exactly 0.6em and MonoLisa's is exactly 0.64em, so a
// size is seam-free for both only when 1.2*size and 1.28*size are both whole:
// 12.5 is the only such size in a readable range (25 and 37.5 are the others).
// At 12.5 the measured advance is 16 device px under MonoLisa and 15 under the
// Dux fallback, every block row measures full 255 ink with zero seam pixels,
// and the ※ drift check stays at zero under both.
//
// If you change this size, re-measure: `(advance in CSS px) * 2` must be a
// whole number for BOTH stacks, or the comb comes back.
const FONT_SIZE = 12.5

const ansi = fs.readFileSync(ansiPath, "utf8").replace(/\n$/, "")
const fontDir = path.dirname(fontPath)
const fonts = Object.fromEntries(
  ["regular", "bold", "symbols", "fill"].map((face) => [
    face,
    fs.readFileSync(path.join(fontDir, `dux-mono-${face}.woff2`)).toString("base64"),
  ]),
)
const xtermJs = require.resolve("@xterm/xterm")
const packageRoot = path.dirname(path.dirname(xtermJs))
const xtermCss = fs.readFileSync(path.join(packageRoot, "css", "xterm.css"), "utf8")

// The crop is computed from the plain-text grid the capture writes beside the
// ANSI, so it is expressed in CELLS and multiplied by the cell metrics measured
// off the live terminal. That is what keeps a crop edge on an exact cell
// boundary instead of slicing a border column down its middle, which is what
// the hand-tuned pixel constants this replaced used to do.
//
// "sidebar" frames the left pane: its border column on the left, the matching
// border column on the right, its top border row, and every row down to the
// last one with content in it plus one row of air. The pane's own bottom border
// is deliberately outside the crop; these shots are about the rows, and the
// pane runs to the bottom of a 45-row screen with nothing in it.
const MAX_ASPECT = 1.5

function readGrid(textPath) {
  if (!fs.existsSync(textPath)) {
    throw new Error(`crop needs the capture's text grid, which is missing: ${textPath}`)
  }
  return fs
    .readFileSync(textPath, "utf8")
    .replace(/\n$/, "")
    .split("\n")
    .map((line) => Array.from(line))
}

function sidebarCropCells(grid, cell) {
  const topRow = grid.findIndex((line) => line[0] === "╭")
  if (topRow === -1) {
    throw new Error("crop: no pane top border (╭ in column 0) anywhere in the text grid")
  }
  const rightColumn = grid[topRow].indexOf("╮")
  if (rightColumn === -1) {
    throw new Error(`crop: the pane border starting on row ${topRow} never closes with ╮`)
  }

  const bottomRow = grid.findIndex((line, index) => index > topRow && line[0] === "╰")
  if (bottomRow === -1) {
    throw new Error(`crop: the pane opening on row ${topRow} never closes with ╰`)
  }

  let lastContentRow = topRow
  for (let row = topRow + 1; row < bottomRow; row += 1) {
    const interior = grid[row].slice(1, rightColumn)
    if (interior.some((cell) => cell !== " " && cell !== undefined)) lastContentRow = row
  }

  const endRow = lastContentRow + 1
  const height = (endRow - topRow + 1) * cell.height
  // Tall narrow pictures read badly in the docs, so a crop never exceeds
  // MAX_ASPECT. Cells are much taller than they are wide, so this has to be
  // measured in PIXELS: a rect that looks square in cell counts is over 2:1 on
  // screen. There is room to spare on every shot today; if a future one runs
  // out, widen into the neighbouring pane rather than cutting real rows.
  const gridWidth = Math.max(...grid.map((line) => line.length))
  let endColumn = rightColumn
  while (height > MAX_ASPECT * (endColumn + 1) * cell.width && endColumn + 1 < gridWidth) {
    endColumn += 1
  }
  if (height > MAX_ASPECT * (endColumn + 1) * cell.width) {
    throw new Error(
      `crop: ${endRow - topRow + 1} rows cannot fit ${MAX_ASPECT}:1 inside a ${gridWidth}-column grid`,
    )
  }

  return { column: 0, row: topRow, columns: endColumn + 1, rows: endRow - topRow + 1 }
}

;(async () => {
  const browser = await puppeteer.launch({
    executablePath: chrome,
    headless: "new",
    args: ["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage", "--force-color-profile=srgb"],
  })
  const page = await browser.newPage()
  await page.setViewport({
    width: Math.max(900, Math.ceil(cols * FONT_SIZE)),
    height: Math.max(600, Math.ceil(rows * FONT_SIZE * 2)),
    deviceScaleFactor: DEVICE_SCALE,
  })
  await page.setContent('<main id="capture"><div id="terminal"></div></main>')
  await page.addStyleTag({ content: `${xtermCss}
    @font-face { font-family: "Dux Mono Symbols"; src: url(data:font/woff2;base64,${fonts.symbols}) format("woff2"); font-weight: 400; unicode-range: U+2190-21FF, U+2300-23FF, U+2500-25FF, U+2600-27BF, U+2800-28FF, U+E0A0-E0D7; }
    @font-face { font-family: "Dux Mono"; src: url(data:font/woff2;base64,${fonts.regular}) format("woff2"); font-weight: 400; }
    @font-face { font-family: "Dux Mono"; src: url(data:font/woff2;base64,${fonts.bold}) format("woff2"); font-weight: 700; }
    @font-face { font-family: "Dux Mono Fill"; src: url(data:font/woff2;base64,${fonts.fill}) format("woff2"); font-weight: 400; unicode-range: U+2000-2BFF, U+2E00-2E7F, U+1F000-1FBFF; }
    * { box-sizing: border-box; }
    html, body { margin: 0; background: #0d1117; }
    #capture { display: inline-block; padding: ${FRAME}px; background: #0d1117; }
    #terminal { display: inline-block; }
    .xterm { padding: 0; }
  ` })
  await page.addScriptTag({ path: xtermJs })
  // Fetch every BUNDLED face BEFORE the Terminal is constructed.
  // `document.fonts.ready` alone is not enough: it settles once no load is
  // pending, and a `unicode-range`-restricted face nothing has rendered yet has
  // no load pending, so it stays unfetched. xterm's DOM renderer then measures
  // glyph advances against the fallback font, caches them (its WidthCache busts
  // only on a font change), and emits negative letter-spacing that drags whole
  // rows left once the real face arrives: measured drift of a full cell for ※,
  // 0.4 for ✓/✷/↳, 0.22 for braille.
  //
  // Each load names ONE family with a sample inside that family's own
  // unicode-range, so no face depends on where it sits in the stack. The range
  // literals above are the same ones the web app declares in
  // crates/dux-web/web/src/index.css and exports from
  // crates/dux-web/web/src/lib/terminalFont.ts; when a range moves there, move
  // it here and re-pick these samples. Deliberately not shared code: a build
  // step for a screenshot tool is not worth it.
  //
  // A load whose family or sample matches no declared face resolves to an
  // EMPTY array rather than rejecting, so a typo here would silently bring the
  // drifted capture back. Every result is checked and an empty one fails the
  // capture out loud. Only the bundled faces are checked this way: a face the
  // OPERATING SYSTEM provides is never in `document.fonts` at all, so the
  // preferred family is probed by measurement instead, further down.
  await page.evaluate(async () => {
    const preloads = [
      { shorthand: '14px "Dux Mono"', sample: "Ag" },
      { shorthand: 'bold 14px "Dux Mono"', sample: "Ag" },
      { shorthand: '14px "Dux Mono Symbols"', sample: "✓⣿─" },
      { shorthand: '14px "Dux Mono Fill"', sample: "※✷" },
    ]
    const loaded = await Promise.all(
      preloads.map((preload) =>
        document.fonts.load(preload.shorthand, preload.sample),
      ),
    )
    loaded.forEach((faces, index) => {
      if (faces.length === 0) {
        throw new Error(`a terminal face did not load: ${preloads[index].shorthand}`)
      }
    })
  })
  await page.evaluate(() => document.fonts.ready)

  // The preferred face is installed on the host, not shipped here, so its
  // absence is a warning rather than a failure: the capture still works on the
  // bundled stack, at a different (also seam-free) cell size. It cannot be
  // detected through `document.fonts`, which only ever holds CSS-declared
  // faces, so measure a run of glyphs against a family that certainly does not
  // exist and see whether naming the face changes the answer.
  const preferredPresent = await page.evaluate((family) => {
    const context = document.createElement("canvas").getContext("2d")
    const sample = "M".repeat(20)
    const widthWith = (stack) => {
      context.font = `100px ${stack}`
      return context.measureText(sample).width
    }
    const absent = widthWith('"Dux No Such Family", monospace')
    return widthWith(`"${family}", "Dux No Such Family", monospace`) !== absent
  }, PREFERRED_FAMILY)
  if (!preferredPresent) {
    console.error(
      `!! ${PREFERRED_FAMILY} is not installed on this host; falling back to the bundled Dux Mono stack.`,
    )
    console.error(
      "!! The capture is still correct, but its cell size differs from the committed screenshots,",
    )
    console.error(
      "!! so every shot you take will differ from the set in website/public/screens/.",
    )
  }

  const metrics = await page.evaluate(
    ({ ansi, cols, rows, fontStack, fontSize }) => new Promise((resolve) => {
      const terminal = new window.Terminal({
        cols,
        rows,
        allowTransparency: false,
        convertEol: true,
        cursorBlink: false,
        cursorInactiveStyle: "none",
        disableStdin: true,
        fontFamily: fontStack,
        fontSize,
        lineHeight: 1,
        scrollback: 0,
        theme: { background: "#0d1117" },
      })
      terminal.open(document.getElementById("terminal"))
      terminal.write(`\x1b[?25l\x1b[2J\x1b[H${ansi}`, () => {
        const screen = document.querySelector(".xterm-screen").getBoundingClientRect()
        resolve({
          left: screen.left,
          top: screen.top,
          cellWidth: screen.width / cols,
          cellHeight: screen.height / rows,
        })
      })
    }),
    { ansi, cols, rows, fontStack: FONT_STACK, fontSize: FONT_SIZE },
  )
  const deviceCell = metrics.cellWidth * DEVICE_SCALE
  if (Math.abs(deviceCell - Math.round(deviceCell)) > 1e-6) {
    throw new Error(
      `cell advance is ${deviceCell} device pixels, not a whole number: every block row will render as a comb. See the FONT_SIZE comment.`,
    )
  }
  await new Promise((resolve) => setTimeout(resolve, 100))

  if (cropArg === "sidebar") {
    const cells = sidebarCropCells(readGrid(ansiPath.replace(/\.ansi$/, ".txt")), {
      width: metrics.cellWidth,
      height: metrics.cellHeight,
    })
    const clip = {
      x: metrics.left + cells.column * metrics.cellWidth,
      y: metrics.top + cells.row * metrics.cellHeight,
      width: cells.columns * metrics.cellWidth,
      height: cells.rows * metrics.cellHeight,
    }
    for (const [name, value] of Object.entries(clip)) {
      if (Math.abs(value * DEVICE_SCALE - Math.round(value * DEVICE_SCALE)) > 1e-6) {
        throw new Error(`crop ${name} is ${value} CSS px, which is not a whole device pixel`)
      }
    }
    await page.screenshot({ path: pngPath, clip, captureBeyondViewport: true })
    console.log(
      `wrote ${pngPath} (crop: ${cells.columns}x${cells.rows} cells at ${cells.column},${cells.row})`,
    )
  } else {
    const capture = await page.$("#capture")
    await capture.screenshot({ path: pngPath, omitBackground: false })
    console.log(`wrote ${pngPath} (${cols}x${rows} cells)`)
  }
  await browser.close()
})().catch((error) => {
  console.error(error.stack || String(error))
  process.exit(1)
})
