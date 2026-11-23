// On a phone a toast starts below whatever chrome is at the top of the screen,
// so its buttons and the chrome's stay reachable, and in theater, where there
// is no top chrome, it is at the top.
const { AGENT } = require("../dialogs.js")

// A toast every open page is told about.
async function raiseToast(H) {
  await H.lib.api("POST", "/api/v1/config/reload")
}

// Where the topmost toast starts, once it has stopped sliding in.
async function toastTop(H, dev) {
  let last = null
  return H.until(async () => {
    const top = await dev.page.evaluate(() => {
      const tops = [...document.querySelectorAll("[data-sonner-toast]")]
        .filter((t) => t.getAttribute("data-removed") !== "true")
        .map((t) => t.getBoundingClientRect())
        .filter((r) => r.height > 0)
        .map((r) => r.top)
      return tops.length ? Math.min(...tops) : null
    })
    const still = top !== null && last !== null && Math.abs(top - last) < 0.5
    last = top
    return still ? top : null
  }, "a toast to settle on screen", { timeout: 6000 })
}

// The bottom edge of the top chrome a person sees: the header, and on a pane
// the action flap hanging off it, found by its theater and macro controls,
// named the way a screen reader says them. Every landmark asked for must be
// there: a renamed control would otherwise quietly leave the flap unmeasured.
async function chromeBottom(H, dev, { flap }) {
  const found = await dev.page.evaluate((names) => {
    const shown = (r) => r.height > 0 && r.top < innerHeight / 3
    const header = [...document.querySelectorAll("header")].map((h) => h.getBoundingClientRect()).filter(shown)
    const controls = {}
    for (const b of document.querySelectorAll("button")) {
      const name = window.__checkNameOf(b)
      const r = b.getBoundingClientRect()
      if (names.includes(name) && shown(r)) controls[name] = r.bottom
    }
    return { header: header.map((r) => r.bottom), controls }
  }, FLAP_CONTROLS)
  H.expect(found.header.length, "there is no header at the top of the screen to measure")
  const missing = flap ? FLAP_CONTROLS.filter((name) => !(name in found.controls)) : []
  H.expect(!missing.length, `the action flap should carry ${missing.map((n) => `"${n}"`).join(" and ")} at the top of the screen; it does not`)
  return Math.max(...found.header, ...Object.values(found.controls))
}

const FLAP_CONTROLS = ["Theater mode", "Run a macro"]
// Phone toasts re-measure the chrome above them every 250ms while one is up.
const RETURN_MS = 1000

module.exports = {
  protects:
    "On a phone a toast starts below the lowest top chrome on the hub, on an agent pane (header and action flap) and under a full-screen dialog's title row, goes back below the flap once that dialog closes, and sits at the top of the screen in theater mode.",
  async run(ctx, H) {
    const dev = await H.device(ctx, "phone")

    const below = async (where, bottom) => {
      await raiseToast(H)
      const top = await toastTop(H, dev)
      H.expect(
        top >= bottom - 0.5,
        `${where}: the toast should start below the top chrome, which ends at ${Math.round(bottom)}px; it starts at ${Math.round(top)}px, over it`,
      )
    }

    await H.goto(dev, "")
    await below("On the hub", await chromeBottom(H, dev, { flap: false }))

    await H.goto(dev, H.agentHash(await H.agentId(AGENT)))
    await H.drive(dev)
    const paneChrome = await chromeBottom(H, dev, { flap: true })
    await below("On an agent pane", paneChrome)

    await H.paneMenu(dev, "^Rename agent…$")
    const dialog = await H.waitForDialog(dev, "^Rename agent$")
    H.expect(dialog.header, `"${dialog.name}" has no title row to measure`)
    await below(`Under the full-screen "${dialog.name}"`, dialog.header.bottom)
    await H.press(dev, { name: "^Close$", selector: "button", within: H.DIALOG })
    await H.waitForNoDialog(dev, `"${dialog.name}"`)

    const closed = Date.now()
    let top = null
    await H.until(
      async () => {
        top = await toastTop(H, dev)
        return top >= paneChrome - 0.5
      },
      "",
      { timeout: RETURN_MS },
    ).catch(() =>
      H.fail(
        `Once "${dialog.name}" closed, the toasts should go back below the action flap, which ends at ${Math.round(paneChrome)}px, within ${RETURN_MS}ms; ${Date.now() - closed}ms later the topmost starts at ${Math.round(top)}px`,
      ),
    )

    await H.press(dev, { name: "^Theater mode$", selector: "button" })
    await H.waitFor(dev.page, () => document.querySelector("header") === null, "theater to take the header away")
    await raiseToast(H)
    const theaterTop = await toastTop(H, dev)
    H.expect(theaterTop <= 40, `In theater mode the toast should be at the top of the screen; it starts at ${Math.round(theaterTop)}px`)
  },
}
