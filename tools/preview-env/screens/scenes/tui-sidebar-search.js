// The same sidebar filtered: only the matching rows survive, with the matched
// letters picked out in each row.
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
  await sleep(800)
  sendText("retry")
  // The field echoes what was typed, which is how this knows the filter opened
  // and took the text.
  await waitFor("/ retry", 10000)
  await sleep(800)
  // The filter leaves the selection where it was; the shot is of the second
  // surviving row.
  sendKeys("Down")
  await sleep(1000)
}

// The filter field holding the query, and the two rows that survived it. A
// capture where the filter never took shows no field.
module.exports.expectText = ["/ retry", "retry-budget", "retry-tests"]

module.exports.file = "tui-sidebar-search.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.crop = "sidebar"
module.exports.fixture = "steady"
