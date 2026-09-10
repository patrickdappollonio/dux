// The header's serving crumb: the standing signal that this process is running
// a listener, port and all.
module.exports = async ({ createAgent, palette, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await palette("start-background-server")
  await waitFor("serving", 20000)
  // The pane is a live stream, so the number of test batches on it is however
  // long this waits. Long enough to read as a session, short enough to fit.
  await sleep(4000)
}

// The port the crumb names. Config, because the crumb reports what dux bound
// rather than anything a journey can type at it.
module.exports.config = (text) => text.replace(/^port = \d+$/m, "port = 3890")

module.exports.file = "tui-serving-crumb.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.fixture = "working"
