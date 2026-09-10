// The tab strip above an agent's terminal: three tabs of the same provider,
// numbered, with the one on screen highlighted.
module.exports = async ({ createAgent, palette, sendKeys, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  for (let i = 0; i < 2; i++) {
    await palette("new-agent-tab")
    // The command asks which provider the new tab runs; fake is the default and
    // the only one that comes up without a login.
    await sleep(800)
    sendKeys("Enter")
    await sleep(2500)
  }
  await waitFor("3 tabs", 15000)
  // Back to the first tab, which is the one the strip shows as active.
  sendKeys("C-Left", "C-Left")
  await sleep(1200)
}

module.exports.file = "tui-tabs-strip-ordinals.png"
module.exports.cols = 120
module.exports.rows = 36
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
