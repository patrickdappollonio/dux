// The startup command log viewer: one run beside what it printed while
// preparing the worktree.
//
// The run's own timestamp is the clock's, so the log's file name and the date
// beside it are the two lines a reshoot always changes.
const fs = require("fs")
const path = require("path")

const STARTUP_COMMAND = [
  'echo "Installing dependencies for $DUX_AGENT_BRANCH"',
  "ls -1 src",
  'echo "Worktree ready at $DUX_WORKTREE_PATH"',
].join("\\n")

module.exports = async ({ createAgent, duxHome, palette, sleep, waitFor }) => {
  // Written into the project's own config entry rather than typed into the
  // dialog: this shot is of the log, and config is where the command lives.
  // The project entries exist by now because the driver added them through the
  // UI before the journey started.
  const configPath = path.join(duxHome, "config.toml")
  const config = fs.readFileSync(configPath, "utf8")
  const marker = 'path = "/capture/repos/demo-api"'
  if (!config.includes(marker)) throw new Error("demo-api is not in the config yet")
  fs.writeFileSync(
    configPath,
    config.replace(marker, `${marker}\nstartup_command = "${STARTUP_COMMAND}"`),
  )
  await palette("reload-config")
  await sleep(1000)

  await createAgent(0, "retry-budget")
  await palette("read-startup-command-logs")
  await waitFor("Startup Command Logs", 15000)
  await sleep(1000)
}

module.exports.file = "tui-startup-command-log.png"
module.exports.cols = 160
module.exports.rows = 30
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
