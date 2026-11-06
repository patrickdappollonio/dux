// The Pull requests picker, opened from the palette: the agent's pull requests
// under their headings, and the keys for each action in its footer.
const { execFileSync } = require("child_process")

// The agent fix-login-redirect with three pull requests, as the browser scenes
// have it: the stand-in gh finds the open one on its branch and a merged one on
// a branch its worktree was on before, and a draft from another repository is
// attached through the Pull requests picker.
async function seedPullRequests({ createAgent, duxHome, palette, repos, sendKeys, sendText, waitFor }) {
  await createAgent(0, "fix-login-redirect")
  const git = (cwd, ...args) => execFileSync("git", args, { cwd, stdio: "pipe" })
  // Only now: dux pulls a project before creating an agent, and nothing
  // answers this URL.
  git(`${repos}/demo-api`, "remote", "add", "origin", "https://github.com/acme/demo-api.git")
  const worktree = `${duxHome}/worktrees/demo-api/fix-login-redirect`
  git(worktree, "checkout", "-q", "-b", "login-session-cookie")
  git(worktree, "checkout", "-q", "fix-login-redirect")
  await palette("pull-requests")
  await waitFor(" Pull Requests ")
  sendKeys("a")
  await waitFor("Attach Pull Request")
  sendText("acme/infra#77")
  sendKeys("Enter")
  // Attaching closes the picker. The sync finds the merged one on its next
  // poll, which the config below brings down to the shortest there is, and
  // the band then counts all three.
  await waitFor("1/3", 120000)
}

module.exports = async (driver) => {
  await seedPullRequests(driver)
  await driver.selectAgent("fix-login-redirect")
  await driver.palette("pull-requests")
  await driver.waitFor("#118", 20000)
  await driver.sleep(800)
}

// The three pull requests, the attached one by its repository's name.
module.exports.expectText = ["#123", "acme/infra#77", "#118"]

module.exports.github = true
module.exports.config = (text) =>
  text.replace(/^pr_poll_interval_seconds = .*$/m, "pr_poll_interval_seconds = 30")
module.exports.file = "tui-pull-requests-modal.png"
module.exports.cols = 120
module.exports.rows = 36
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
