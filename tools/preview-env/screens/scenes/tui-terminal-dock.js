// An agent with its terminals docked under it: the agent on top, the dock
// below with a numbered pill per terminal, and the layout control at the right
// of the dock's strip. A second terminal is what brings both strips up.
module.exports = async ({ createAgent, palette, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await palette("new-terminal-for-agent")
  await waitFor("agent | split | term", 15000)
  await palette("new-terminal-for-agent")
  await waitFor("│ 2 │", 15000)
  await sleep(1500)
}

// The one list's title, the control, and the agent the dock belongs to.
module.exports.expectText = [
  "Agents & Terminals",
  "agent | split | term",
  "retry-budget",
]

module.exports.file = "tui-terminal-dock.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
