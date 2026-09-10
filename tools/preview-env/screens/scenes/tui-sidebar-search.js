// The same sidebar filtered: the pane title counts what survived and the
// matched letters are picked out in each row.
//
// The workspace is built inline rather than shared with
// tui-agent-list-two-line.js: the capture container mounts one journey file and
// nothing beside it, so a scene cannot require a helper.
module.exports = async ({ createAgent, palette, sendKeys, sendText, sleep, waitFor }) => {
  for (const name of ["docs-pass", "release-notes"]) {
    await createAgent(0, name)
    await palette("kill-running")
    await sleep(1500)
  }

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

  sendKeys("/")
  await sleep(500)
  sendText("retry")
  await sleep(800)
  // The filter leaves the selection where it was; the shot is of the second
  // surviving row.
  sendKeys("Down")
  await sleep(1000)
}

module.exports.file = "tui-sidebar-search.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.crop = "sidebar"
module.exports.fixture = "steady"
