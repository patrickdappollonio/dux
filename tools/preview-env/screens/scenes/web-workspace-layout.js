// The three-pane desktop workspace: the sidebar, a working agent's terminal,
// and its changed files.
const {
  SIDEBAR_ORDER,
  agents,
  clearToasts,
  expectNoCover,
  expectPanePainted,
  expectRows,
  expectStateWord,
  freshen,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "web-workspace-layout.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    // A fixture that has been streaming for minutes fills the pane with test
    // batches; five seconds is a session that just came up.
    await freshen(by["fix-login-redirect"].id, 5)
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await setTerminalsExpanded(page, true)
    await sleep(2500)
    await clearToasts(page)
    // All three panes the caption names: the six rows on the left, a terminal
    // with a session on it in the middle, and the state word that says the
    // agent is working. This picture went out once with the pane still reading
    // "Attaching…".
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectRows(page, SIDEBAR_ORDER)
    await expectStateWord(page, "fix-login-redirect", "Working")
    return { x: 0, y: 0, width: 1440, height: 900 }
  },
}

// The Terminals divider defaults open; a run that collapsed it must put it back.
async function setTerminalsExpanded(page, wantOpen) {
  await page.evaluate((open) => {
    const btn = [...document.querySelectorAll("button")].find((b) =>
      /^\s*Terminals/.test(b.textContent || ""),
    )
    if (!btn) return
    if ((btn.getAttribute("aria-expanded") === "true") !== open) btn.click()
  }, wantOpen)
  await sleep(400)
}
