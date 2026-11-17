// The standalone agent picker: the project chooser on top, with the keys, over
// the folder explorer. One project is greyed out because a standalone agent
// already works in its folder.
module.exports = async ({ createStandaloneAgent, focusSidebar, palette, repos, sleep, waitFor }) => {
  await createStandaloneAgent(`${repos}/demo-web`, "demo-web")
  // dux focuses the new agent's pane, where a chord would be typed at the agent.
  await focusSidebar()
  await palette("new-standalone-agent")
  await waitFor("Start In A Project's Folder", 15000)
  // The rows are read off the disk in the background and land a moment after
  // the picker opens.
  await waitFor("already has a standalone agent", 15000)
  await sleep(800)
}

// Both halves, the sentence about what a shortcut does not bring along, the
// project that can be picked and the reason on the one that cannot.
module.exports.expectText = [
  "Start In A Project's Folder",
  "not its environment variables or startup command",
  "demo-api",
  "already has a standalone agent",
  "Standalone Agent In",
]

module.exports.file = "tui-standalone-project-shortcuts.png"
module.exports.cols = 160
module.exports.rows = 36
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
