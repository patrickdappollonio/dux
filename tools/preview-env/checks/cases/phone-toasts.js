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

// The bottom edge of the top chrome a person sees: the header, and the action
// flap hanging off it on a pane (found by its theater and macro controls, which
// nothing else at the top carries, named the way a screen reader
// says them).
async function chromeBottom(dev) {
  return dev.page.evaluate(() => {
    const bottoms = [...document.querySelectorAll("header")]
      .map((h) => h.getBoundingClientRect())
      .filter((r) => r.height > 0 && r.top < innerHeight / 3)
      .map((r) => r.bottom)
    for (const b of document.querySelectorAll("button")) {
      if (!/^(Theater mode|Run a macro)$/.test(window.__checkNameOf(b))) continue
      const r = b.getBoundingClientRect()
      if (r.height > 0 && r.top < innerHeight / 3) bottoms.push(r.bottom)
    }
    return bottoms.length ? Math.max(...bottoms) : null
  })
}

module.exports = {
  // Its toasts are replayed to pages opened in the next 30 seconds, so the
  // checks after it close them on arrival.
  raisesServerStatuses: true,
  protects:
    "On a phone a toast starts below the lowest top chrome on the hub, on an agent pane (header and action flap) and under a full-screen dialog's title row, and at the top of the screen in theater mode.",
  async run(ctx, H) {
    const dev = await H.device(ctx, "phone")

    const below = async (where, bottom) => {
      await raiseToast(H)
      const top = await toastTop(H, dev)
      H.expect(bottom !== null, `${where}: there is no top chrome to measure`)
      H.expect(
        top >= bottom - 0.5,
        `${where}: the toast should start below the top chrome, which ends at ${Math.round(bottom)}px; it starts at ${Math.round(top)}px, over it`,
      )
    }

    await H.goto(dev, "")
    await below("On the hub", await chromeBottom(dev))

    await H.goto(dev, H.agentHash(await H.agentId(AGENT)))
    await H.drive(dev)
    await below("On an agent pane", await chromeBottom(dev))

    await H.paneMenu(dev, "^Rename agent…$")
    const dialog = await H.waitForDialog(dev, "^Rename agent$")
    H.expect(dialog.header, `"${dialog.name}" has no title row to measure`)
    await below(`Under the full-screen "${dialog.name}"`, dialog.header.bottom)
    await H.press(dev, { name: "^Close$", selector: "button", within: H.DIALOG })
    await H.waitForNoDialog(dev, `"${dialog.name}"`)

    // The toasts raised so far lie over the flap, so they are closed first, as a
    // person reaching for the theater button would.
    await H.dismissToasts(dev)
    await H.press(dev, { name: "^Theater mode$", selector: "button" })
    await H.waitFor(dev.page, () => document.querySelector("header") === null, "theater to take the header away")
    await raiseToast(H)
    const top = await toastTop(H, dev)
    H.expect(top <= 40, `In theater mode the toast should be at the top of the screen; it starts at ${Math.round(top)}px`)
  },
}
