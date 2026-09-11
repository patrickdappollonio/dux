// The sidebar's two-line rows: three active agents, one of them running three
// tabs, above the collapsed Inactive tail.
//
// Self-contained on purpose: the capture container mounts this file alone as the
// journey, so a scene cannot require a shared helper. tui-sidebar-search.js
// builds the same workspace for the same reason.
module.exports = async ({ addTab, createAgent, selectAgent, setFixture, sleep }) => {
  // Born on the fixture that exits at once, so these two have no live process
  // and land in the Inactive tail. Stopping them afterwards would mean driving
  // the kill dialog, which is a picker rather than a one-shot command.
  await setFixture("failure")
  for (const name of ["docs-pass", "release-notes"]) {
    await createAgent(0, name)
    await sleep(1500)
  }
  await setFixture("steady")

  // The active rows read in reverse creation order, so the one on top is made
  // last and the one the shot has selected is made first.
  await createAgent(0, "retry-budget")
  await addTab(2)
  await addTab(3)
  await createAgent(0, "retry-tests")
  await createAgent(0, "cache-warmup")

  // Picked by name rather than by counting rows: which row an agent lands on is
  // the sidebar's sort talking.
  await selectAgent("retry-budget")
  await sleep(1200)
}

// The three active rows, the tab count on one of them, and the collapsed tail:
// the whole of what the caption counts.
module.exports.expectText = [
  "retry-budget",
  "retry-tests",
  "cache-warmup",
  "3 tabs",
  "Inactive (2)",
]

module.exports.file = "tui-agent-list-two-line.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.crop = "sidebar"
module.exports.fixture = "steady"
