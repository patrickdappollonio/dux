// The welcome screen, reopened from the palette rather than waited for: the
// capture config disables the automatic one so no other journey has to dismiss
// it.
module.exports = async ({ createAgent, palette, sleep }) => {
  await createAgent(0, "retry-budget")
  await palette("show-welcome-screen")
  await sleep(1000)
}

// The screen's own frame title and the button the caption names.
module.exports.expectText = ["Welcome to dux", "Add a project"]

module.exports.file = "tui-welcome-screen.png"
module.exports.cols = 140
module.exports.rows = 40
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
