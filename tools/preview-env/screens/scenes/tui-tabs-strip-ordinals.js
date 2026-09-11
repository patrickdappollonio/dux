// The tab strip above an agent's terminal: three tabs of the same provider,
// numbered, with the one on screen highlighted.
module.exports = async ({ addTab, createAgent, sendKeys, sleep }) => {
  await createAgent(0, "retry-budget")
  await addTab(2)
  await addTab(3)
  // Back to the first tab, which is the one the strip shows as active.
  sendKeys("C-Left", "C-Left")
  await sleep(1200)
}

// The agent the strip belongs to and the provider every pill carries.
module.exports.expectText = ["retry-budget", "fake"]

module.exports.file = "tui-tabs-strip-ordinals.png"
module.exports.cols = 120
module.exports.rows = 36
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
