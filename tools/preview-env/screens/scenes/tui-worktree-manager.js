// The worktree manager: one worktree no agent holds, above the two that are
// held and therefore cannot be removed here.
module.exports = async ({ createAgent, palette, seedLooseWorktree, sendKeys, sendText, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await createAgent(0, "cache-warmup")
  // A worktree with no agent behind it, which is the only kind this dialog can
  // remove. Made with git directly, because dux only ever leaves one behind by
  // deleting an agent and keeping its worktree.
  seedLooseWorktree("demo-api", "docs-pass")
  await palette("manage-worktrees")
  // The command asks which project first. demo-api is picked by name through
  // the chooser's search rather than trusted to be the row it opens on, which
  // is the selected project and therefore the app's call.
  await waitFor("Manage worktrees in project", 15000)
  sendKeys("/")
  await sleep(300)
  sendText("demo-api")
  await sleep(600)
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
