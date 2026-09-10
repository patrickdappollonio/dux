// The sidebar's two-line rows: three active agents, one of them running three
// tabs, above the collapsed Inactive tail.
//
// Self-contained on purpose: the capture container mounts this file alone as the
// journey, so a scene cannot require a shared helper. tui-sidebar-search.js
// builds the same workspace for the same reason.
module.exports = async ({ createAgent, palette, sendKeys, sleep, waitFor }) => {
  // Stopped straight after creation, while each is still the selected agent:
  // an agent with no live process is what the Inactive tail collects.
  for (const name of ["docs-pass", "release-notes"]) {
    await createAgent(0, name)
    await palette("kill-running")
    await sleep(1500)
  }

  // The active rows read in reverse creation order, so the one on top is made
  // last and the one the shot has selected is made first.
  await createAgent(0, "retry-budget")
  for (let i = 0; i < 2; i++) {
    await palette("new-agent-tab")
    await sleep(800)
    sendKeys("Enter")
    await sleep(2500)
  }
  await waitFor("3 tabs", 15000)
  await createAgent(0, "retry-tests")
  await createAgent(0, "cache-warmup")

  // Creation leaves the newest agent selected, which is the top row; the shot is
  // of the third one down.
  sendKeys("Down", "Down")
  await sleep(1200)
}

module.exports.file = "tui-agent-list-two-line.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.crop = "sidebar"
module.exports.fixture = "steady"
