// The tab strip above an agent's terminal: three tabs of the same provider,
// numbered, with the one on screen highlighted. Adding a tab puts it on
// screen, so the third, the last one added, is the highlighted pill.
module.exports = async ({ addTab, createAgent, sleep }) => {
  await createAgent(0, "retry-budget")
  await addTab(2)
  await addTab(3)
  await sleep(1200)
}

// The agent the strip belongs to and the provider every pill carries.
module.exports.expectText = ["retry-budget", "fake"]

module.exports.file = "tui-tabs-strip-ordinals.png"
module.exports.cols = 120
module.exports.rows = 36
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
