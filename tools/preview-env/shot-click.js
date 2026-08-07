// Click an element matching a text string, then screenshot. For capturing
// dialogs/menus that are opened by interaction (not routable). Host-side.
//   CHROME=<chromium> node shot-click.js <url> <out.png> "<button text>" [w] [h]
const puppeteer = require("puppeteer-core")
const [, , url, out = "shot.png", clickText, w = "1280", h = "900"] = process.argv
;(async () => {
  const browser = await puppeteer.launch({
    executablePath: process.env.CHROME,
    headless: "new",
    args: ["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage", "--force-color-profile=srgb", "--hide-scrollbars"],
  })
  const page = await browser.newPage()
  await page.setViewport({ width: +w, height: +h, deviceScaleFactor: 2 })
  await page.goto(url, { waitUntil: "networkidle0", timeout: 30000 })
  await new Promise((r) => setTimeout(r, 600))
  if (clickText) {
    const clicked = await page.evaluate((text) => {
      const els = [...document.querySelectorAll("button, [role='button'], a")]
      const el = els.find((e) => e.textContent.trim().includes(text))
      if (el) { el.click(); return true }
      return false
    }, clickText)
    if (!clicked) console.error(`WARN: no clickable element matching "${clickText}"`)
    await new Promise((r) => setTimeout(r, 700))
  }
  await page.screenshot({ path: out })
  await browser.close()
  console.log("wrote", out)
})().catch((e) => { console.error(String(e)); process.exit(1) })
