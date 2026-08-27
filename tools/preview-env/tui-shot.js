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
const font = fs.readFileSync(fontPath).toString("base64")
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
    @font-face { font-family: "Dux Mono"; src: url(data:font/woff2;base64,${font}) format("woff2"); font-weight: 400; }
    * { box-sizing: border-box; }
    html, body { margin: 0; background: #0d1117; }
    #capture { display: inline-block; padding: 18px; background: #0d1117; }
    #terminal { display: inline-block; }
    .xterm { padding: 0; }
  ` })
  await page.addScriptTag({ path: xtermJs })
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
        fontFamily: '"Dux Mono", monospace',
        fontSize: 14,
        lineHeight: 1.08,
        scrollback: 0,
        theme: { background: "#0d1117" },
      })
      terminal.open(document.getElementById("terminal"))
      terminal.write(`\x1b[?25l\x1b[2J\x1b[H${ansi}`, resolve)
    }),
    { ansi, cols, rows },
  )
  await page.evaluate(() => document.fonts.ready)
  await new Promise((resolve) => setTimeout(resolve, 100))
  const capture = await page.$("#capture")
  await capture.screenshot({ path: pngPath, omitBackground: false })
  await browser.close()
  console.log(`wrote ${pngPath} (${cols}x${rows} cells)`)
})().catch((error) => {
  console.error(error.stack || String(error))
  process.exit(1)
})
