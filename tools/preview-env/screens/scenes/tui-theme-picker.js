// The theme picker, listing the bundled themes with the current one first.
module.exports = async ({ createAgent, palette, sleep }) => {
  await createAgent(0, "retry-budget")
  await palette("change-theme")
  await sleep(1000)
}

// The picker and the theme it opens on, which is the one in use.
module.exports.expectText = ["Change Theme", "dux_dark"]

module.exports.file = "tui-theme-picker.png"
module.exports.cols = 140
module.exports.rows = 40
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
