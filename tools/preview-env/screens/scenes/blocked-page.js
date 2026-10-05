// The page a blocked address gets. The scene earns the block the way a guesser
// does: wrong passwords on the sign-in page, each one waiting out the slow-down
// the last one bought, until dux writes the address the preview's browser
// arrives from into blocked_addresses and the page turns into this one. The page
// never names the address, so the picture carries none.
const { clearToasts, expectVisibleText, goto, sleep } = require("../lib.js")
const { gateClip, requirePassword, resetAuth } = require("../auth.js")

// The default `max_failed_logins` is five. An attempt that lands a moment
// before the slow-down is over is answered "wait" and counts for nothing, so
// there is room for a few of those.
const MAX_ATTEMPTS = 12

module.exports = {
  file: "blocked-page.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    after(resetAuth)
    await requirePassword()
    await goto(page, "")
    const field = '[data-testid="login-form"] input[name="password"]'
    const submit = '[data-testid="login-form"] button[type="submit"]'
    const blocked = () =>
      page.evaluate(() => /This address is blocked/.test(document.body.innerText))
    for (let i = 0; i < MAX_ATTEMPTS && !(await blocked()); i++) {
      // The slow-down doubles from one second, so the longest wait here is
      // eight; the button is enabled again once it is over.
      await page.waitForFunction(
        (f, b) => {
          if (/This address is blocked/.test(document.body.innerText)) return true
          const input = document.querySelector(f)
          const button = document.querySelector(b)
          return input && button && !input.disabled && !button.disabled
        },
        { timeout: 30000 },
        field,
        submit,
      )
      // The page's countdown rounds to whole seconds, so it can re-enable the
      // button a little before dux will take the next attempt.
      await sleep(1200)
      if (await blocked()) break
      await page.type(field, `not-the-password-${i}`)
      await page.keyboard.press("Enter")
      await sleep(1500)
    }
    await clearToasts(page)
    await expectVisibleText(page, "This address is blocked")
    await expectVisibleText(page, "Try again", { selector: "button" })
    return gateClip(page)
  },
}
