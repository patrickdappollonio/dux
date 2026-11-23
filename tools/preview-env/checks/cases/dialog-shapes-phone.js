// On a phone a confirm stays a small card with its two buttons side by side,
// and every other kind covers the screen: a title row carrying the close, a
// footer on the bottom edge, and no text field focused so the keyboard stays
// down.
const { DIALOGS, arrive } = require("../dialogs.js")

module.exports = {
  protects:
    "On a phone a confirm is a small card with its two buttons side by side; a form, a picker and a report cover the screen with the close in the title row, a footer on the bottom edge and no text field focused; the close control hands focus back to what opened them.",
  async run(ctx, H) {
    const dev = await H.device(ctx, "phone")
    const { width: vw, height: vh } = dev
    for (const [kind, spec] of Object.entries(DIALOGS)) {
      await arrive(dev, spec.from)
      await spec.open(dev)
      const d = await H.waitForDialog(dev, spec.title)
      const b = d.box
      const focus = await H.focused(dev.page)
      H.expect(
        !focus.typed,
        `the ${kind} "${d.name}" should open with no text field focused on a phone, so the keyboard stays down; focus is on ${focus.what}`,
      )

      let closer
      if (kind === "confirm") {
        H.expect(
          b.width < vw && b.height < vh / 2,
          `the confirm "${d.name}" should stay a small card on a phone; it measures ${H.rect(b)} on a ${vw}x${vh} screen`,
        )
        const footer = d.buttons.filter((x) => H.near(x.box.y, d.buttons.at(-1).box.y, 1))
        H.expect(
          footer.length === 2 && footer[0].box.right <= footer[1].box.x,
          `the confirm "${d.name}" should carry its two buttons side by side; its buttons are ${d.buttons.map((x) => `"${x.name}" at ${H.rect(x.box)}`).join(", ")}`,
        )
        closer = spec.safe
      } else {
        H.expect(
          H.near(b.x, 0) && H.near(b.y, 0) && H.near(b.width, vw) && H.near(b.height, vh),
          `the ${kind} "${d.name}" should cover the whole ${vw}x${vh} screen on a phone; it measures ${H.rect(b)}`,
        )
        const close = d.buttons.find((x) => x.name === "Close")
        H.expect(
          close && d.header && close.box.y >= d.header.y && close.box.bottom <= d.header.bottom,
          `the ${kind} "${d.name}" should carry a Close in its title row${d.header ? ` (${H.rect(d.header)})` : ""}; ${close ? `its Close sits at ${H.rect(close.box)}` : "it has no Close"}`,
        )
        H.expect(
          kind !== "form" || d.footer,
          `the form "${d.name}" has no footer for its Cancel and Save`,
        )
        H.expect(
          !d.footer || H.near(d.footer.bottom, vh),
          `the ${kind} "${d.name}" should pin its footer to the bottom of the screen (${vh}); the footer ends at ${Math.round(d.footer && d.footer.bottom)}`,
        )
        closer = "Close"
      }

      await H.press(dev, { name: `^${closer}$`, selector: "button", within: H.DIALOG })
      await H.waitForNoDialog(dev, `the ${kind} "${d.name}" on its ${closer}`)
      const back = await H.focusIsMarked(dev.page, "opener")
      H.expect(
        back.same,
        `closing the ${kind} "${d.name}" should put focus back on the "${spec.opener}" that opened it (${back.marked}); focus is on ${(await H.focused(dev.page)).what}`,
      )
    }
  },
}
