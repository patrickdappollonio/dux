// The Pull requests dialog on a phone: a sheet, reached from the header's
// "#123 +2" chip and the list it opens.
const {
  agents,
  clearToasts,
  expectDialogOpen,
  expectVisibleText,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "phone-pull-requests-sheet.png",
  viewport: "phone",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await sleep(2000)
    await clearToasts(page)
    // A DOM click rather than a pointer one: under touch emulation the
    // emulated tap never reports done once a sheet covers the button.
    const opened = await page.evaluate(() => {
      const chip = [...document.querySelectorAll("button[aria-label]")].find((b) =>
        /and 2 more$/.test(b.getAttribute("aria-label")),
      )
      if (!chip) return false
      chip.click()
      return true
    })
    if (!opened) throw new Error("no header chip counting two more pull requests")
    await sleep(900)
    const managed = await page.evaluate(() => {
      const item = [...document.querySelectorAll("[role=menuitem]")].find(
        (el) => el.textContent.trim() === "Pull requests…",
      )
      if (!item) return false
      item.click()
      return true
    })
    if (!managed) throw new Error("the chip's list offers no Pull requests…")
    await sleep(1200)
    await page.evaluate(() => document.activeElement?.blur())
    await clearToasts(page)
    await expectDialogOpen(page, "Pull requests")
    await expectVisibleText(page, "Allow the preview host", {
      what: "the attached pull request's title",
    })
    await expectVisibleText(page, "login-session-cookie", {
      what: "the merged pull request's branch",
    })
    return { x: 0, y: 0, width: 390, height: 844 }
  },
}
