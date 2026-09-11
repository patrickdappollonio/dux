// The same sidebar filtered: the pane title counts what survived and the
// matched letters are picked out in each row.
//
// The workspace is built inline rather than shared with
// tui-agent-list-two-line.js: the capture container mounts one journey file and
// nothing beside it, so a scene cannot require a helper.
module.exports = async ({
  addTab,
  createAgent,
  focusSidebar,
  sendKeys,
  sendText,
  setFixture,
  sleep,
  waitFor,
}) => {
  // Born on the fixture that exits at once, so these two land in the Inactive
  // tail and the filter has something to hide as well as something to keep.
  await setFixture("failure")
  for (const name of ["docs-pass", "release-notes"]) {
    await createAgent(0, name)
    await sleep(1500)
  }
  await setFixture("steady")

  await createAgent(0, "retry-budget")
  await addTab(2)
  await addTab(3)
  await createAgent(0, "retry-tests")
  await createAgent(0, "cache-warmup")

  // The filter is a sidebar key, and creation left the center pane focused.
  await focusSidebar()
  sendKeys("/")
  // The pane title counts the filter's matches the moment it opens, which is how
  // this knows the field is there to type into.
  await waitFor("/5)", 10000)
  sendText("retry")
  await sleep(800)
  // The filter leaves the selection where it was; the shot is of the second
  // surviving row.
  sendKeys("Down")
  await sleep(1000)
}

// The count the filter leaves in the pane title, and the two rows that survived
// it. A capture where the filter never took shows five rows and no count.
module.exports.expectText = ["2/5", "retry-budget", "retry-tests"]

module.exports.file = "tui-sidebar-search.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.crop = "sidebar"
module.exports.fixture = "steady"
