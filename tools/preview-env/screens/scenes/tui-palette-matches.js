// The command palette answering a partial phrase with the one command that
// matches it.
module.exports = async ({ createAgent, sendKeys, sendText, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  sendKeys("C-p")
  await waitFor("Command Palette")
  sendText("new tab")
  await sleep(800)
}

module.exports.file = "tui-palette-matches.png"
module.exports.cols = 120
module.exports.rows = 36
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
