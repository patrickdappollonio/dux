// Focus stays in an open dialog while the window is resized across the phone
// breakpoint and back, so the next key typed still lands in its field.
const { AGENT, DIALOGS } = require("../dialogs.js")

// Resizes the window, then waits for the form to have settled at the new size:
// crossing the phone breakpoint moves it between a card and a cover.
async function resize(H, dev, width, title) {
  await dev.cdp.send("Emulation.setDeviceMetricsOverride", { ...dev.metrics, width })
  await H.waitFor(dev.page, (w) => innerWidth === w, `the window to be ${width}px wide`, { arg: width })
  await H.waitForDialog(dev, title).catch(async () => {
    const top = await H.topDialog(dev.page)
    H.fail(`after resizing to ${width}px the form should still be open; ${top ? `"${top.name}" is on top` : "no dialog is open"}`)
  })
}

module.exports = {
  protects:
    "With a form open, resizing the window 1440 → 700 → 1440 keeps focus inside the dialog, and a typed letter lands in its field.",
  async run(ctx, H) {
    const dev = await H.device(ctx, "computer")
    await H.goto(dev, H.agentHash(await H.agentId(AGENT)))
    await H.drive(dev)
    await DIALOGS.form.open(dev)
    const d = await H.waitForDialog(dev, DIALOGS.form.title)
    const before = d.fields[0].value

    for (const width of [700, 1440]) {
      await resize(H, dev, width, DIALOGS.form.title)
      const top = await H.topDialog(dev.page)
      const focus = await H.focused(dev.page)
      H.expect(top.focusInside, `after resizing to ${width}px focus should still be inside "${d.name}"; it is on ${focus.what}`)
    }

    await dev.page.keyboard.type("q")
    const after = await H.topDialog(dev.page)
    H.expect(
      after.fields[0].value === `${before}q`,
      `a letter typed after the resizes should land in "${d.fields[0].name}"; it holds "${after.fields[0].value}" (it held "${before}")`,
    )

    // Left without saving the letter, whether or not the form asks first.
    await dev.page.keyboard.press("Escape")
    const asked = await H.until(async () => {
      const open = await H.dialogs(dev.page)
      if (open.length === 0) return "closed"
      return /^Discard/.test(open.at(-1).name) ? "asked" : null
    }, `"${d.name}" to close or ask about its edit`)
    if (asked === "asked") await H.press(dev, { name: "^Discard changes$", selector: "button" })
    await H.waitForNoDialog(dev, `"${d.name}" without its edit`)
  },
}
