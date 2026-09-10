// The project chooser a new agent starts at, with the way out to a standalone
// agent named on its footer.
module.exports = async ({ createAgent, palette, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await palette("new-agent")
  await waitFor("New agent in project", 15000)
  await sleep(800)
}

module.exports.file = "tui-new-agent-chooser.png"
module.exports.cols = 160
module.exports.rows = 30
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
