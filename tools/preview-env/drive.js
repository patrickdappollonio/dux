// Flexible UI driver for exploring dux web states. Runs a sequence of actions
// then screenshots. Host-side, points at the container's forwarded port.
//
//   CHROME=<chromium> node drive.js <url> <out.png> '<actions-json>' [w] [h] [--mobile]
//
// Actions (array): each is one of
//   {"click":"text"}         click first button/link/[role] whose text includes "text"
//   {"clickSel":"css"}       click first element matching a CSS selector
//   {"type":"text"}          type into the focused element
//   {"typeInto":"css","text":"x"}  focus a selector, then type
//   {"key":"Enter"}          press a key
//   {"wait":600}             sleep ms
//   {"hover":"text"}         hover first element whose text includes "text"
const puppeteer = require("puppeteer-core")
const argv = process.argv.slice(2)
const mobile = argv.includes("--mobile")
const pos = argv.filter((a) => !a.startsWith("--"))
const [url, out = "shot.png", actionsJson = "[]", w, h] = pos
const width = +(w || (mobile ? 390 : 1280))
const height = +(h || (mobile ? 844 : 900))
const actions = JSON.parse(actionsJson)

const clickByText = (text) => {
  const els = [...document.querySelectorAll("button, [role='button'], [role='menuitem'], a, [role='option']")]
  const el = els.reverse().find((e) => e.textContent.trim().includes(text))
  if (el) { el.click(); return true }
  return false
}
const hoverByText = (text) => {
  const els = [...document.querySelectorAll("*")]
  const el = els.find((e) => e.children.length === 0 && e.textContent.trim() === text)
  if (el) { el.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })); return true }
  return false
}

;(async () => {
  const browser = await puppeteer.launch({
    executablePath: process.env.CHROME,
    headless: "new",
    args: ["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage", "--force-color-profile=srgb", "--hide-scrollbars"],
  })
  const page = await browser.newPage()
  await page.setViewport({ width, height, deviceScaleFactor: 2, isMobile: mobile, hasTouch: mobile })
  await page.goto(url, { waitUntil: "networkidle0", timeout: 30000 })
  await new Promise((r) => setTimeout(r, 600))
  for (const a of actions) {
    if (a.click !== undefined) {
      const ok = await page.evaluate(clickByText, a.click)
      if (!ok) console.error(`WARN: no click match for "${a.click}"`)
    } else if (a.clickSel) {
      await page.click(a.clickSel).catch(() => console.error(`WARN: no selector ${a.clickSel}`))
    } else if (a.clickEval) {
      // DOM .click() via evaluate: fires even on hover-hidden (opacity/max-width 0)
      // triggers that a real pointer click would miss.
      const ok = await page.evaluate((s) => {
        const e = document.querySelector(s)
        if (e) { e.click(); return true }
        return false
      }, a.clickEval)
      if (!ok) console.error(`WARN: no element for clickEval ${a.clickEval}`)
    } else if (a.hover !== undefined) {
      const ok = await page.evaluate(hoverByText, a.hover)
      if (!ok) console.error(`WARN: no hover match for "${a.hover}"`)
    } else if (a.typeInto) {
      await page.focus(a.typeInto).catch(() => {})
      await page.keyboard.type(a.text)
    } else if (a.type !== undefined) {
      await page.keyboard.type(a.type)
    } else if (a.key) {
      await page.keyboard.press(a.key)
    } else if (a.wait) {
      await new Promise((r) => setTimeout(r, a.wait))
    }
    await new Promise((r) => setTimeout(r, 350))
  }
  await new Promise((r) => setTimeout(r, 500))
  await page.screenshot({ path: out })
  await browser.close()
  console.log("wrote", out)
})().catch((e) => { console.error(String(e)); process.exit(1) })
