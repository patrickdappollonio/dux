// On a computer every kind of dialog floats as a card, takes focus where its
// kind says, closes on Escape and hands focus back to what opened it.
const { DIALOGS, arrive } = require("../dialogs.js")

module.exports = {
  protects:
    "On a computer a confirm, a form, a picker and a report each float as a card, focus the safe button, the first field, the search field or Close on open, and close on Escape with focus back on what opened them.",
  async run(ctx, H) {
    const dev = await H.device(ctx, "computer")
    const { width: vw, height: vh } = dev
    for (const [kind, spec] of Object.entries(DIALOGS)) {
      await arrive(dev, spec.from)
      await spec.open(dev)
      const d = await H.waitForDialog(dev, spec.title)
      const b = d.box
      H.expect(
        b.x > 8 && b.y > 8 && b.right < vw - 8 && b.bottom < vh - 8,
        `the ${kind} "${d.name}" should float as a card clear of the ${vw}x${vh} window's edges; it sits at ${H.rect(b)}`,
      )

      const focus = await H.focused(dev.page)
      if (kind === "confirm" || kind === "report") {
        H.expect(
          focus.name === spec.safe && !focus.typed,
          `the ${kind} "${d.name}" should open with focus on its ${spec.safe} button; focus is on ${focus.what}`,
        )
      } else if (kind === "picker") {
        H.expect(
          focus.typed && /^Search/.test(focus.name),
          `the picker "${d.name}" should open with focus in its search field; focus is on ${focus.what}`,
        )
      } else {
        const first = d.fields[0]
        H.expect(first, `the ${kind} "${d.name}" has no text field to focus`)
        H.expect(
          focus.typed && focus.name === first.name,
          `the ${kind} "${d.name}" should open with focus in its first field "${first.name}"; focus is on ${focus.what}`,
        )
      }

      await dev.page.keyboard.press("Escape")
      await H.waitForNoDialog(dev, `the ${kind} "${d.name}" on Escape`)
      const back = await H.focused(dev.page)
      H.expect(
        back.name === spec.opener,
        `closing the ${kind} "${d.name}" should put focus back on "${spec.opener}", which opened it; focus is on ${back.what}`,
      )
    }
  },
}
