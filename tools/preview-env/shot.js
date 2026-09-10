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
// sidebar+center layout. Both render at 2x, so a phone PNG is 780px wide and a
// desktop one 2880px, which is the size every committed desktop screenshot is.
const width = +(w || (mobile ? 390 : 1440))
const height = +(h || (mobile ? 844 : 900))

// The scale is a browser flag, never puppeteer's `deviceScaleFactor` viewport
// field: a webgl canvas captured under an emulated device scale factor above 1
// comes back black in headless Chromium, so the metrics override is sent with a
// factor of 0, meaning "no override" rather than "1x", and
// `--force-device-scale-factor` produces the same 2x pixels the ordinary way.
// SwiftShader is asked for by name because it is what makes WebGL exist with no
// GPU; `--disable-gpu` silently leaves the terminal on the DOM renderer.
const launchArgs = [
  "--no-sandbox",
  "--disable-dev-shm-usage",
  "--force-color-profile=srgb",
  "--hide-scrollbars",
  "--force-device-scale-factor=2",
  `--window-size=${width},${height}`,
  "--use-gl=angle",
  "--use-angle=swiftshader",
  "--enable-unsafe-swiftshader",
]

;(async () => {
  const browser = await puppeteer.launch({
    executablePath: process.env.CHROME,
    headless: "new",
    // No default viewport: a puppeteer-applied one would re-introduce the
    // metrics override this is avoiding.
    defaultViewport: null,
    args: launchArgs,
  })
  const page = await browser.newPage()
  const cdp = await page.createCDPSession()
  const metrics = { width, height, deviceScaleFactor: 0, mobile }
  await cdp.send("Emulation.setDeviceMetricsOverride", metrics)
  if (mobile) {
    await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 })
    await cdp.send("Emulation.setEmitTouchEventsForMouse", {
      enabled: true,
      configuration: "mobile",
    })
  }
  await page.goto(url, { waitUntil: "networkidle0", timeout: 30000 })
  // Let the boot read and the pushed workspace document land, plus one
  // animation frame to settle.
  await new Promise((r) => setTimeout(r, 800))
  // A window opened at a size the platform will not give (a phone preset is
  // narrower than Chromium's minimum window width) can write the window's pixels
  // rather than the emulated viewport's, so the metrics are re-asserted
  // (idempotent) and the clip pins the output to the preset either way.
  await cdp.send("Emulation.setDeviceMetricsOverride", metrics)
  await page.screenshot({ path: out, clip: { x: 0, y: 0, width, height } })
  await browser.close()
  console.log("wrote", out, `(${width}x${height}${mobile ? " mobile" : ""})`)
})().catch((e) => {
  console.error(String(e))
  process.exit(1)
})
