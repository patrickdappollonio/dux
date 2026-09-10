// The macro bar over an agent pane, listing the macros that pane can take.
module.exports = async ({ createAgent, sendKeys, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await waitFor("Waiting for the next instruction", 20000)
  // The macro bar has a chord of its own rather than a palette command.
  sendKeys("C-\\")
  await waitFor("Macros", 10000)
  await sleep(800)
}

// The macros the bar lists. Seeded into the config rather than typed through the
// editor, because this shot is of the bar, not of how macros are written.
module.exports.config = (text) =>
  `${text}
[macros.Review]
text = "review this code for bugs"
surface = "agent"

[macros."Write tests"]
text = "write unit tests for what you just changed"
surface = "agent"

[macros."Explain failure"]
text = "explain the last test failure and propose a fix"
surface = "agent"

[macros.Lint]
text = "cargo clippy --all-targets"
surface = "agent"
`

module.exports.file = "tui-macro-bar.png"
module.exports.cols = 160
module.exports.rows = 26
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
