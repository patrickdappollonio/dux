// The confirmation behind the worktree manager: what will be removed, what goes
// with it, and the ticked box that takes the branch too.
module.exports = async ({ createAgent, palette, seedLooseWorktree, sendKeys, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await createAgent(0, "cache-warmup")
  seedLooseWorktree("demo-api", "docs-pass")
  await palette("manage-worktrees")
  // The command asks which project first; demo-api is the row it opens on.
  await waitFor("Manage worktrees in project", 15000)
  sendKeys("Enter")
  await waitFor("Manage Worktrees", 15000)
  await sleep(800)
  // The removable worktree is the first row and starts selected, so Enter is the
  // whole gesture.
  sendKeys("Enter")
  await waitFor("Delete Worktree", 15000)
  await sleep(1000)
}

module.exports.file = "tui-worktree-delete-confirm.png"
module.exports.cols = 140
module.exports.rows = 40
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
