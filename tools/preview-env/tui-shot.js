const fs = require("fs")
const path = require("path")
const puppeteer = require("puppeteer-core")

const [ansiPath, pngPath, colsArg, rowsArg, fontPath] = process.argv.slice(2)
const cols = Number(colsArg)
const rows = Number(rowsArg)

if (!ansiPath || !pngPath || !Number.isInteger(cols) || !Number.isInteger(rows) || !fontPath) {
  console.error("usage: node tui-shot.js <input.ansi> <output.png> <cols> <rows> <font.woff2>")
  process.exit(64)
}

const chrome = process.env.CHROME
if (!chrome) {
  console.error("CHROME must point to a Chromium executable")
  process.exit(64)
}

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

;(async () => {
  const browser = await puppeteer.launch({
    executablePath: chrome,
    headless: "new",
    args: ["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage", "--force-color-profile=srgb"],
  })
  const page = await browser.newPage()
  await page.setViewport({
    width: Math.max(900, cols * 10),
    height: Math.max(600, rows * 22),
    deviceScaleFactor: 2,
  })
  await page.setContent('<main id="capture"><div id="terminal"></div></main>')
  await page.addStyleTag({ content: `${xtermCss}
    @font-face { font-family: "Dux Mono Symbols"; src: url(data:font/woff2;base64,${fonts.symbols}) format("woff2"); font-weight: 400; unicode-range: U+2190-21FF, U+2300-23FF, U+2500-25FF, U+2600-27BF, U+2800-28FF, U+E0A0-E0D7; }
    @font-face { font-family: "Dux Mono"; src: url(data:font/woff2;base64,${fonts.regular}) format("woff2"); font-weight: 400; }
    @font-face { font-family: "Dux Mono"; src: url(data:font/woff2;base64,${fonts.bold}) format("woff2"); font-weight: 700; }
    @font-face { font-family: "Dux Mono Fill"; src: url(data:font/woff2;base64,${fonts.fill}) format("woff2"); font-weight: 400; unicode-range: U+2000-2BFF, U+2E00-2E7F, U+1F000-1FBFF; }
    * { box-sizing: border-box; }
    html, body { margin: 0; background: #0d1117; }
    #capture { display: inline-block; padding: 18px; background: #0d1117; }
    #terminal { display: inline-block; }
    .xterm { padding: 0; }
  ` })
  await page.addScriptTag({ path: xtermJs })
  // Fetch every face BEFORE the Terminal is constructed. `document.fonts.ready`
  // alone is not enough: it settles once no load is pending, and a
  // `unicode-range`-restricted face nothing has rendered yet has no load
  // pending, so it stays unfetched. xterm's DOM renderer then measures glyph
  // advances against the fallback font, caches them (its WidthCache busts only
  // on a font change), and emits negative letter-spacing that drags whole rows
  // left once the real face arrives: measured drift of a full cell for ※, 0.4
  // for ✓/✷/↳, 0.22 for braille.
  //
  // Each load names ONE family with a sample inside that family's own
  // unicode-range, so no face depends on where it sits in the stack. The range
  // literals above are the same ones the web app declares in
  // crates/dux-web/web/src/index.css and exports from
  // crates/dux-web/web/src/lib/terminalFont.ts; when a range moves there, move
  // it here and re-pick these samples. Deliberately not shared code: a build
  // step for a screenshot tool is not worth it.
  await page.evaluate(async () => {
    await Promise.all([
      document.fonts.load('14px "Dux Mono"', "Ag"),
      document.fonts.load('bold 14px "Dux Mono"', "Ag"),
      document.fonts.load('14px "Dux Mono Symbols"', "✓⣿─"),
      document.fonts.load('14px "Dux Mono Fill"', "※✷"),
    ])
  })
  await page.evaluate(() => document.fonts.ready)
  await page.evaluate(
    ({ ansi, cols, rows }) => new Promise((resolve) => {
      const terminal = new window.Terminal({
        cols,
        rows,
        allowTransparency: false,
        convertEol: true,
        cursorBlink: false,
        cursorInactiveStyle: "none",
        disableStdin: true,
        fontFamily: '"Dux Mono Symbols", "Dux Mono", "Dux Mono Fill", monospace',
        fontSize: 14,
        lineHeight: 1,
        scrollback: 0,
        theme: { background: "#0d1117" },
      })
      terminal.open(document.getElementById("terminal"))
      terminal.write(`\x1b[?25l\x1b[2J\x1b[H${ansi}`, resolve)
    }),
    { ansi, cols, rows },
  )
  await new Promise((resolve) => setTimeout(resolve, 100))
  const capture = await page.$("#capture")
  await capture.screenshot({ path: pngPath, omitBackground: false })
  await browser.close()
  console.log(`wrote ${pngPath} (${cols}x${rows} cells)`)
})().catch((error) => {
  console.error(error.stack || String(error))
  process.exit(1)
})
