// One agent waiting on you among two that are idle. The flag clears the moment
// you look at an agent's pane, so the one that has it is created last and never
// focused, and the selection is walked back to a quiet row.
module.exports = async ({ createAgent, sendKeys, setFixture, sleep }) => {
  await createAgent(0, "retry-budget")
  await createAgent(0, "cache-warmup")
  // The fake provider reads its fixture from the global environment, so the next
  // agent comes up on the one that rings the bell.
  await setFixture("attention")
  await createAgent(1, "review-retry-policy")
  await sleep(2000)
  // Needs-you sorts to the top, so the agent the shot has selected is two rows
  // down from the one just created.
  sendKeys("Down", "Down")
  await sleep(1200)
}

module.exports.file = "tui-attention-sidebar.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.crop = "sidebar"
module.exports.fixture = "steady"
