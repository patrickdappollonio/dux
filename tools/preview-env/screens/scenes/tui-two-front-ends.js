// The terminal UI half of the introduction's pair, showing the same workspace
// the browser shot shows: six agents, three of them working, one of them living
// in a plain folder.
module.exports = async ({
  createAgent,
  createStandaloneAgent,
  selectAgent,
  setFixture,
  sleep,
}) => {
  // Idle first, working second: the sidebar floats the working agents to the
  // top, which is what makes the three that are working the three at the top,
  // as the browser shot reads too.
  await createAgent(0, "review-billing")
  await createAgent(0, "polish-onboarding")
  await createStandaloneAgent("/root/design-notes", "design-notes")

  await setFixture("working")
  await createAgent(0, "refactor-cache")
  await createAgent(0, "add-rate-limits")
  await createAgent(0, "fix-login-redirect")

  // The shot is of the agent whose pane is streaming, picked by name: which row
  // it lands on is the sidebar's sort talking.
  await selectAgent("add-rate-limits")
  // The pane is a live stream, so this is how many test batches end up on it.
  await sleep(6000)
}

module.exports.file = "tui-two-front-ends.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
