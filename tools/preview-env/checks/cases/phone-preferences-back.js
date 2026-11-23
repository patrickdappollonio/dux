// On a phone the browser's Back steps out of a Preferences topic, and a second
// Back sets the dialog aside, edits and all, with a way to restore it, rather
// than carrying the page out of dux. Both spellings of the home address count:
// the history entry the dialog pushes must not depend on how the page was
// reached.

const TOPIC = "^Typing and touch"
const SETTING = "^Copy on select$"

async function state(page) {
  return page.evaluate(() => {
    const open = [...document.querySelectorAll('[role="dialog"]')].filter((d) => !d.hasAttribute("data-closed"))
    const top = open.at(-1)
    const buttons = top ? [...top.querySelectorAll("button")].map((b) => window.__checkNameOf(b)) : []
    return {
      href: location.href,
      dialog: top ? window.__checkNameOf(top).slice(0, 60) : null,
      topics: buttons.some((name) => /^Typing and touch/.test(name)),
      inTopic: [...document.querySelectorAll('[role="switch"]')].some((s) => /^Copy on select$/.test(window.__checkNameOf(s))),
      restore: [...document.querySelectorAll("[data-sonner-toast] button")].some((b) => b.textContent.trim() === "Restore"),
    }
  })
}

async function oneAddress(H, dev, hash) {
  const where = hash ? `opened from the address ending "/${hash}"` : 'opened from the address ending "/"'
  await H.goto(dev, hash)
  await H.press(dev, { name: "^Menu$", selector: "button" })
  await H.press(dev, { name: "^Preferences", selector: "[role=menuitem]" })
  await H.waitForDialog(dev, "^Preferences")
  await H.press(dev, { name: TOPIC, selector: "button", within: H.DIALOG })
  const checked = () =>
    dev.page.evaluate(() => {
      const s = [...document.querySelectorAll('[role="switch"]')].find((el) => /^Copy on select$/.test(window.__checkNameOf(el)))
      return s ? s.getAttribute("aria-checked") : null
    })
  const was = await checked()
  await H.press(dev, { name: SETTING, selector: '[role="switch"]' })
  await H.until(async () => (await checked()) !== was, "the switch to flip", { timeout: 2000 })
  // The edit is only the dialog's once it has rendered as one.
  await H.frames(dev.page)
  const origin = new URL(H.BASE).origin

  await H.back(dev)
  const first = await H.until(async () => {
    const now = await state(dev.page).catch(() => null)
    return now && now.topics && !now.inTopic ? now : null
  }, "the topic list after Back", { timeout: 4000 }).catch(async () => {
    const now = await state(dev.page).catch((e) => ({ href: `gone (${e.message})` }))
    H.fail(`${where}, the browser's Back from a Preferences topic should return to the topic list; the page shows ${now.dialog ? `"${now.dialog}"` : "no dialog"} at ${now.href}`)
  })
  H.expect(first.href.startsWith(origin), `${where}, the first Back left dux for ${first.href}`)

  await H.back(dev)
  await H.until(async () => {
    const now = await state(dev.page).catch(() => null)
    return now && !now.dialog && now.restore ? now : null
  }, "the dialog to be set aside", { timeout: 4000 }).catch(async () => {
    const now = await state(dev.page).catch((e) => ({ href: `gone (${e.message})` }))
    H.fail(
      `${where}, a second Back should set Preferences aside with a "Restore" toast and stay in dux; the page is at ${now.href} showing ${now.dialog ? `"${now.dialog}" still open` : "no dialog"}${now.restore ? "" : " and no Restore toast"}`,
    )
  })
}

module.exports = {
  protects:
    "On a phone, from both \"/\" and \"#/\", the browser's Back from an edited Preferences topic returns to the topic list, and a second Back sets the dialog aside with a \"Restore\" toast without leaving the app.",
  async run(ctx, H) {
    for (const hash of ["", "#/"]) {
      const dev = await H.device(ctx, "phone")
      await oneAddress(H, dev, hash)
    }
  },
}
