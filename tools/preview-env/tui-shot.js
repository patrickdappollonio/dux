const fs = require("fs")
const path = require("path")
const puppeteer = require("puppeteer-core")
const { captureProblem } = require("./screens/ink.js")

// The exit code that means the capture refused itself: the cells do not say what
// the scene said they must, or the picture came back empty. reshoot.sh reads it
// to tell a wrong picture from a run that never got there.
const REFUSED = 65

function refuse(sentence) {
  const error = new Error(sentence)
  error.refusal = true
  throw error
}

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
// The capture's own frame, in CSS pixels, around the terminal grid. A crop gets
// none: everything outside its cell rect is other cells of the same screen, so a
// margin would drag in half a row of the header and half a column of the next pane.
const FRAME = 18

// The preferred face. Commercial and not vendored: named here and resolved by
// the host, so a machine without it falls back to the bundled Dux Mono stack.
// The exact family name is what `fc-list` reports.
const PREFERRED_FAMILY = "MonoLisa Nerd Font Mono"
const DUX_STACK = '"Dux Mono Symbols", "Dux Mono", "Dux Mono Fill", monospace'
const FONT_STACK = `"${PREFERRED_FAMILY}", ${DUX_STACK}`

// xterm's DOM renderer lays every cell out at the font's own unrounded advance,
// and any size whose advance misses a whole device pixel makes Chromium
// antialias every glyph box edge, turning a row of block glyphs into a comb of
// seams. Dux Mono's advance is exactly 0.6em and MonoLisa's exactly 0.64em, so a
// size is seam-free for both only when 1.2*size and 1.28*size are both whole.
// Changing this size means re-measuring: `(advance in CSS px) * 2` must be a
// whole number for both stacks, or the comb comes back.
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

// A crop is expressed in CELLS, computed from the plain-text grid the capture
// writes beside the ANSI and multiplied by cell metrics measured off the live
// terminal, so a crop edge lands on an exact cell boundary rather than slicing a
// border column down its middle. "sidebar" frames the left pane down to its last
// row with content plus one row of air, deliberately excluding the pane's own
// bottom border: the pane runs to the bottom of a 45-row screen with nothing in it.
const MAX_ASPECT = 1.5

function readGrid(textPath) {
  if (!fs.existsSync(textPath)) {
    throw new Error(`the capture's text grid is missing: ${textPath}`)
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
  // Measured in PIXELS, not cell counts: cells are much taller than they are
  // wide, so a rect that looks square in cells is over 2:1 on screen. A crop
  // that runs out of room should widen into the neighbouring pane, not cut rows.
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

// --- What the picture has to say -------------------------------------------
//
// A journey that ends on the wrong screen captures a perfectly valid grid of it,
// so a scene names the words its picture is about. The check lives here rather
// than in the driver because only this side knows the CROP: a sidebar shot is a
// picture of thirty columns, and text in the pane beside it is not in it.
const journeyPath = process.env.DUX_TUI_JOURNEY

function expectedText() {
  if (!journeyPath || !fs.existsSync(journeyPath)) return null
  const journey = require(path.resolve(journeyPath))
  const wanted = journey.expectText
  return Array.isArray(wanted) && wanted.length ? wanted : null
}

// The cells the picture will actually contain, as one string.
function croppedText(grid, cells) {
  if (!cells) return grid.map((line) => line.join("")).join("\n")
  return grid
    .slice(cells.row, cells.row + cells.rows)
    .map((line) => line.slice(cells.column, cells.column + cells.columns).join(""))
    .join("\n")
}

function checkCells(grid, cells) {
  const wanted = expectedText()
  if (!wanted) {
    console.error("no expectText on this journey, so nothing checked what it captured")
    return
  }
  const text = croppedText(grid, cells)
  const missing = wanted.filter((needle) => !text.includes(needle))
  if (missing.length) {
    refuse(
      `${path.basename(journeyPath)}: the captured picture does not show ` +
        `${missing.map((needle) => JSON.stringify(needle)).join(", ")}\n\n${text}`,
    )
  }
}

// The artifact, once it exists. Every check above reads the cells; this reads
// the picture, which is a separate thing that can come back empty on its own.
function checkPicture(png) {
  const problem = captureProblem(png)
  if (problem) refuse(`${path.basename(pngPath)}: ${problem}`)
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
  // Every bundled face must be fetched before the Terminal is constructed.
  // `document.fonts.ready` settles once no load is pending, and a
  // `unicode-range`-restricted face nothing has rendered has no load pending, so
  // xterm's DOM renderer measures glyph advances against the fallback, caches
  // them, and drags whole rows left once the real face arrives.
  //
  // Each load names one family with a sample inside that family's own
  // unicode-range, so no face depends on where it sits in the stack; the range
  // literals above mirror crates/dux-web/web/src/index.css, so a range that
  // moves there moves here too. A load matching no declared face resolves to an
  // empty array rather than rejecting, so every result is checked and an empty
  // one fails the capture out loud. Only bundled faces can be checked this way:
  // an OS-provided face is never in `document.fonts`, so the preferred family is
  // probed by measurement further down.
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

  // The preferred face is installed on the host, so its absence is a warning and
  // not a failure. It cannot be detected through `document.fonts`, which only
  // holds CSS-declared faces, so a run of glyphs is measured against a family
  // that certainly does not exist to see whether naming the face changes it.
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

  const grid = readGrid(ansiPath.replace(/\.ansi$/, ".txt"))
  if (cropArg === "sidebar") {
    const cells = sidebarCropCells(grid, {
      width: metrics.cellWidth,
      height: metrics.cellHeight,
    })
    checkCells(grid, cells)
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
    // Into memory and read before it is kept, like every browser scene's
    // capture: a picture nobody looked at is how an empty frame ships.
    const png = Buffer.from(
      await page.screenshot({ clip, captureBeyondViewport: true }),
    )
    checkPicture(png)
    fs.writeFileSync(pngPath, png)
    console.log(
      `wrote ${pngPath} (crop: ${cells.columns}x${cells.rows} cells at ${cells.column},${cells.row})`,
    )
  } else {
    checkCells(grid, null)
    const capture = await page.$("#capture")
    const png = Buffer.from(await capture.screenshot({ omitBackground: false }))
    checkPicture(png)
    fs.writeFileSync(pngPath, png)
    console.log(`wrote ${pngPath} (${cols}x${rows} cells)`)
  }
  await browser.close()
})().catch((error) => {
  // A refusal is a sentence about the picture and carries its own exit code; a
  // stack is for everything else, which is the tool failing rather than a
  // verdict on what was captured.
  if (error && error.refusal) {
    console.error(error.message)
    process.exit(REFUSED)
  }
  console.error(error.stack || String(error))
  process.exit(1)
})
