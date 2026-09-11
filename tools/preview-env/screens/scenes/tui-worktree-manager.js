// The worktree manager: one worktree no agent holds, above the two that are
// held and therefore cannot be removed here.
module.exports = async ({ createAgent, palette, seedLooseWorktree, sendKeys, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await createAgent(0, "cache-warmup")
  // A worktree with no agent behind it, which is the only kind this dialog can
  // remove. Made with git directly, because dux only ever leaves one behind by
  // deleting an agent and keeping its worktree.
  seedLooseWorktree("demo-api", "docs-pass")
  await palette("manage-worktrees")
  // The command asks which project first; demo-api is the row it opens on.
  await waitFor("Manage worktrees in project", 15000)
  sendKeys("Enter")
  await waitFor("Manage Worktrees", 15000)
  await sleep(1200)
}

// The manager, the worktree no agent holds, and the two that are held.
module.exports.expectText = ["Manage Worktrees", "docs-pass", "retry-budget", "cache-warmup"]

module.exports.file = "tui-worktree-manager.png"
module.exports.cols = 140
module.exports.rows = 40
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
