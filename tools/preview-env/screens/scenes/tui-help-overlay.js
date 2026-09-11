// The in-app help overlay, which is the authoritative keybinding reference.
module.exports = async ({ createAgent, sendKeys, sleep }) => {
  await createAgent(0, "retry-budget")
  sendKeys("?")
  await sleep(800)
}

// The overlay's own frame title, which is the only thing that tells this screen
// apart from the workspace it is drawn over.
module.exports.expectText = ["Help"]

module.exports.file = "tui-help-overlay.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
