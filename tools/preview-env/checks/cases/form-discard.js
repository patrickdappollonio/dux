// A form asks before Escape throws away what was typed, and Keep editing puts
// the person back where they were. With nothing typed it closes at once, and
// its Save says why it cannot be pressed.
const { AGENT } = require("../dialogs.js")

const TITLE = "^Environment: "
const TYPED = "CHECKS_DRAFT=1"

async function tooltip(page) {
  return page.evaluate(() =>
    [...document.querySelectorAll('[data-slot="tooltip-content"]')]
      .filter((el) => el.getBoundingClientRect().width > 0)
      .map((el) => el.textContent.trim())
      .join(" | "),
  )
}

module.exports = {
  protects:
    "A form with nothing changed has its Save disabled with a reason and closes on Escape at once; with an edit, Escape asks \"Discard unsaved changes?\" and Keep editing keeps the text and puts focus back in the field.",
  async run(ctx, H) {
    const dev = await H.device(ctx, "computer")
    await H.goto(dev, H.agentHash(await H.agentId(AGENT)))
    await H.drive(dev)

    await H.paneMenu(dev, "^Configure environment variables…$")
    let form = await H.waitForDialog(dev, TITLE)
    const save = form.buttons.find((b) => b.name === "Save")
    H.expect(save && save.disabled, `with nothing changed, "${form.name}" should have its Save disabled; it is ${save ? "enabled" : "missing"}`)
    await H.hover(dev, { name: "^Save$", selector: "button", within: H.DIALOG })
    await H.waitFor(
      dev.page,
      () => [...document.querySelectorAll('[data-slot="tooltip-content"]')].some((el) => el.getBoundingClientRect().width > 0 && el.textContent.trim().length > 0),
      "a reason to appear over the disabled Save",
      { timeout: 3000 },
    ).catch(async () => H.fail(`the disabled Save in "${form.name}" shows no reason when pointed at (tooltips on screen: "${await tooltip(dev.page)}")`))

    await dev.page.keyboard.press("Escape")
    await H.waitForNoDialog(dev, `"${form.name}" with nothing changed, on Escape,`)

    await H.paneMenu(dev, "^Configure environment variables…$")
    form = await H.waitForDialog(dev, TITLE)
    await dev.page.keyboard.type(TYPED)
    await dev.page.keyboard.press("Escape")
    const question = await H.waitForDialog(dev, ".").catch(() => null)
    H.expect(
      question && question.name === "Discard unsaved changes?",
      `Escape over an edited "${form.name}" should ask "Discard unsaved changes?"; ${question ? `"${question.name}" is on top instead` : "nothing asked"}`,
    )

    await H.press(dev, { name: "^Keep editing$", selector: "button" })
    await H.until(async () => (await H.dialogs(dev.page)).length === 1, "the question to close on Keep editing")
    const kept = await H.topDialog(dev.page)
    H.expect(kept && /^Environment: /.test(kept.name), `Keep editing should leave "${form.name}" open; the dialog is ${kept ? `"${kept.name}"` : "gone"}`)
    H.expect(
      kept.fields.some((f) => f.value.includes(TYPED)),
      `Keep editing should keep the typed "${TYPED}"; the fields hold ${JSON.stringify(kept.fields.map((f) => f.value))}`,
    )
    const focus = await H.until(async () => {
      const now = await H.focused(dev.page)
      return now.typed ? now : null
    }, "focus to come back to the field", { timeout: 2000 }).catch(async () => H.fail(`Keep editing should put focus back in the field; focus is on ${(await H.focused(dev.page)).what}`))
    H.expect(
      focus.value && focus.value.includes(TYPED),
      `Keep editing should put focus back in the edited field; focus is on ${focus.what}`,
    )

    await dev.page.keyboard.press("Escape")
    await H.waitForDialog(dev, "^Discard unsaved changes\\?$")
    await H.press(dev, { name: "^Discard changes$", selector: "button" })
    await H.waitForNoDialog(dev, "the form after Discard changes")
  },
}
