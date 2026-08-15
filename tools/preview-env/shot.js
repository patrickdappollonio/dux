// Screenshot the running dux preview with the cached Playwright Chromium (driven
// by puppeteer-core). Runs on the HOST, pointed at the container's forwarded
// loopback port. Proven pipeline: the PNG it writes is directly readable.
//
//   CHROME=<chromium> node shot.js <url> [out.png] [width] [height] [--mobile]
//
// shot.sh sets CHROME and sensible defaults; call that instead of this directly.
const puppeteer = require("puppeteer-core")

const argv = process.argv.slice(2)
const mobile = argv.includes("--mobile")
const positional = argv.filter((a) => !a.startsWith("--"))
const [url, out = "shot.png", w, h] = positional

// Phone preset mirrors the mobile-shell breakpoint (<md); desktop is a roomy
// sidebar+center layout. deviceScaleFactor 2 for crisp text either way.
const width = +(w || (mobile ? 390 : 1280))
const height = +(h || (mobile ? 844 : 900))

;(async () => {
  const browser = await puppeteer.launch({
    executablePath: process.env.CHROME,
    headless: "new",
    args: [
      "--no-sandbox",
      "--disable-gpu",
      "--disable-dev-shm-usage",
      "--force-color-profile=srgb",
      "--hide-scrollbars",
    ],
  })
  const page = await browser.newPage()
  await page.setViewport({
    width,
    height,
    deviceScaleFactor: 2,
    isMobile: mobile,
    hasTouch: mobile,
  })
  await page.goto(url, { waitUntil: "networkidle0", timeout: 30000 })
  // Let the boot read and the pushed workspace document land, plus one
  // animation frame to settle.
  await new Promise((r) => setTimeout(r, 800))
  await page.screenshot({ path: out })
  await browser.close()
  console.log("wrote", out, `(${width}x${height}${mobile ? " mobile" : ""})`)
})().catch((e) => {
  console.error(String(e))
  process.exit(1)
})
