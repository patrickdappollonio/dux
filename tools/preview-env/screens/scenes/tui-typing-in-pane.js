// Typing a prompt straight into an agent's pane, which is what the Typing state
// on its row means.
module.exports = async ({ createAgent, sendKeys, sendText, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await waitFor("Waiting for the next instruction", 20000)
  sendKeys("Enter")
  await sleep(600)
  sendText("add a bounded retry to the request path")
  await sleep(800)
}

// The prompt being typed, which is both the subject of the picture and the
// reason the row says Typing.
module.exports.expectText = ["add a bounded retry to the request path", "Typing"]

module.exports.file = "tui-typing-in-pane.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
