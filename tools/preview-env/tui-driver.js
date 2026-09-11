const fs = require("fs")
const path = require("path")
const { spawnSync } = require("child_process")

const cols = readInteger("DUX_TUI_COLS", 160, 80)
const rows = readInteger("DUX_TUI_ROWS", 45, 24)
const theme = process.env.DUX_TUI_THEME || "catppuccin-mocha"
const outputStem = process.env.DUX_TUI_OUTPUT_STEM || "tui"
const journeyPath = "/journey.js"
const socket = `dux-shot-${process.pid}`
const session = "capture"
const duxHome = process.env.DUX_HOME || "/capture/dux"
const repos = "/capture/repos"
const output = "/output"

if (!/^[a-zA-Z0-9._-]+$/.test(outputStem)) fail(`invalid output stem: ${outputStem}`, 64)
if (!fs.existsSync(journeyPath)) fail("the journey script is not mounted at /journey.js", 64)

const journey = require(journeyPath)
if (typeof journey !== "function") fail("journey.js must export an async function", 64)
const fixture = journey.fixture || "steady"
// A journey's `expectText` is read by the rasterizer rather than here: what the
// picture must say is a question about the cells the CROP keeps, and the crop is
// computed on the other side of this container.
// An optional last word on the seeded config, so a journey that needs a setting
// (a serving port, a set of macros, a theme) does not have to type it into a
// dialog first. Takes the rendered config text and returns the text to write.
const patchConfig = journey.config || ((text) => text)

function readInteger(name, fallback, minimum) {
  const value = Number(process.env[name] || fallback)
  if (!Number.isInteger(value) || value < minimum) fail(`${name} must be an integer of at least ${minimum}`, 64)
  return value
}

function fail(message, code = 1) {
  console.error(message)
  process.exit(code)
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", ...options })
  if (result.status !== 0) {
    const detail = result.stderr || result.stdout || `${command} exited ${result.status}`
    throw new Error(detail.trim())
  }
  return result.stdout
}

function git(cwd, ...args) {
  return run("git", args, {
    cwd,
    env: {
      ...process.env,
      GIT_AUTHOR_DATE: "2026-01-15T12:00:00Z",
      GIT_COMMITTER_DATE: "2026-01-15T12:00:00Z",
    },
  })
}

function seedRepo(repo) {
  fs.mkdirSync(path.join(repo, "src"), { recursive: true })
  fs.mkdirSync(path.join(repo, "tests"), { recursive: true })
  git(repo, "init", "-q", "-b", "main")
  git(repo, "config", "user.name", "Dux Preview")
  git(repo, "config", "user.email", "preview@dux.local")
  fs.writeFileSync(path.join(repo, "README.md"), `# ${path.basename(repo)}\n\nA deterministic Dux preview project.\n`)
  fs.writeFileSync(path.join(repo, "src/client.rs"), "pub fn request() -> Result<(), String> { Ok(()) }\n")
  fs.writeFileSync(path.join(repo, "tests/client.rs"), "#[test]\nfn request_succeeds() { assert!(true); }\n")
  git(repo, "add", "-A")
  git(repo, "commit", "-qm", "Seed preview project")
}

function seedState() {
  fs.mkdirSync(duxHome, { recursive: true })
  fs.mkdirSync(repos, { recursive: true })
  fs.mkdirSync(output, { recursive: true })
  seedRepo(path.join(repos, "demo-api"))
  seedRepo(path.join(repos, "demo-web"))
  fs.writeFileSync(path.join(repos, "demo-api/src/retry.rs"), "pub fn retry_limit() -> usize {\n    3\n}\n")
  fs.appendFileSync(path.join(repos, "demo-api/README.md"), "\nDocument the retry behavior.\n")
  fs.writeFileSync(path.join(repos, "demo-web/src/status.ts"), 'export const status = "ready"\n')

  run("dux", ["config", "regenerate", "--yes"], { env: { ...process.env, DUX_HOME: duxHome } })
  const configPath = path.join(duxHome, "config.toml")
  let config = fs.readFileSync(configPath, "utf8")
  config = config
    .replace(/^provider = .*$/m, 'provider = "fake"')
    .replace(/^disable_automated_welcome_screen = false$/m, "disable_automated_welcome_screen = true")
    .replace(/^disable_release_notes = false$/m, "disable_release_notes = true")
    .replace(/^github_integration = true$/m, "github_integration = false")
    .replace(/^theme = .*$/m, `theme = "${theme}"`)
  config += `
[providers.fake]
command = "/usr/local/bin/fake-agent"
args = []
`
  fs.writeFileSync(configPath, patchConfig(config))
}

/// The fixture the fake provider reads is an environment variable, and the
/// global `[env]` table is what dux hands a provider it spawns. Rewriting that
/// table and reloading the config therefore changes what the NEXT agent comes up
/// as, which is how one journey stages agents in different states.
async function setFixture(name) {
  const configPath = path.join(duxHome, "config.toml")
  const text = fs.readFileSync(configPath, "utf8").replace(/\n\[env\][\s\S]*?(?=\n\[|$)/, "")
  fs.writeFileSync(configPath, `${text}\n[env]\nDUX_FAKE_FIXTURE = "${name}"\n`)
  await palette("reload-config")
  await waitFor("Configuration reloaded", 15000)
}

/// Seed a worktree no agent holds, with something uncommitted in it, so the
/// worktree manager has a removable row to show.
function seedLooseWorktree(project, branch) {
  const repo = path.join(repos, project)
  const at = path.join(duxHome, "worktrees", project, branch)
  fs.mkdirSync(path.dirname(at), { recursive: true })
  git(repo, "worktree", "add", "-q", "-b", branch, at, "HEAD")
  fs.appendFileSync(path.join(at, "README.md"), "\nA note left in this worktree.\n")
}

function tmux(...args) {
  return run("tmux", ["-L", socket, ...args])
}

function captureText() {
  return tmux("capture-pane", "-p", "-N", "-t", `${session}:0.0`)
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}

async function waitFor(needle, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (captureText().includes(needle)) return
    await sleep(100)
  }
  throw new Error(`timed out waiting for ${JSON.stringify(needle)}\n\n${captureText()}`)
}

/// Wait until any one of several strings is on screen, and say which.
async function waitForAny(needles, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const text = captureText()
    const hit = needles.find((needle) => text.includes(needle))
    if (hit) return hit
    await sleep(100)
  }
  throw new Error(`timed out waiting for any of ${JSON.stringify(needles)}\n\n${captureText()}`)
}

function sendKeys(...keys) {
  tmux("send-keys", "-t", `${session}:0.0`, ...keys)
}

function sendText(text) {
  tmux("send-keys", "-t", `${session}:0.0`, "-l", text)
}

async function addProject(absolutePath, label) {
  await focusSidebar()
  sendKeys("a")
  await waitFor("Add Project: /")
  await browseTo(absolutePath)
  await waitFor(`Added project "${label}" to workspace`, 20000)
}

/// Open the command palette, type one command's exact name, and run it. Every
/// journey that reaches a screen with no key of its own goes through here, which
/// is also what the docs tell a user to do.
async function palette(command) {
  sendKeys("C-p")
  await waitFor("Command Palette")
  sendText(command)
  // The typed text lands in the input a frame before the list is filtered, so
  // waiting for the command's own name can match the input line and run
  // whatever was still highlighted underneath. Settle first, then confirm the
  // match is on the list, then run it.
  await sleep(800)
  await waitFor(command)
  sendKeys("Enter")
  await sleep(800)
}

/// Put the focus on the agent pane without engaging it, which is where the
/// pane's own chords (the macro bar) are answered and Enter still means "start
/// typing". Only the center pane's footer offers Reconnect.
async function focusPane() {
  for (let i = 0; i < 4; i++) {
    if (captureText().includes("Reconnect")) return
    sendKeys("Tab")
    await sleep(500)
  }
  throw new Error(`the agent pane never took focus\n\n${captureText()}`)
}

/// Add one provider tab to the selected agent and wait for it to be there.
///
/// Two confirmations, each waited for by name rather than slept over: the
/// palette command opens the provider chooser, and the chooser's own Enter is
/// what creates the tab. The pane is then focused AND interactive, so the focus
/// is aimed back at dux before the caller's next chord.
async function addTab(expectedCount) {
  await palette("new-agent-tab")
  await waitFor("New Tab Provider", 15000)
  sendKeys("Enter")
  await waitFor(`${expectedCount} tabs`, 30000)
  await focusSidebar()
}

/// Put the focus back on the sidebar. Some actions (adding a tab) leave the
/// agent pane focused AND interactive, where every key is forwarded to the child
/// and a journey's next chord would be typed at the agent instead of acted on.
/// Tab cycles the panes, and only the sidebar's footer offers Add project (the
/// changes pane's footer opens with the same Move that a looser test would have
/// matched).
async function focusSidebar() {
  for (let i = 0; i < 4; i++) {
    if (captureText().includes("Add project")) return
    sendKeys("Tab")
    await sleep(500)
  }
  throw new Error(`the sidebar never took focus\n\n${captureText()}`)
}

/// Type an absolute path into whichever folder browser is open. The browser's
/// "go" field starts on the last directory it was in, so the existing text is
/// cleared before the path is typed.
async function browseTo(absolutePath) {
  sendKeys("g")
  await waitFor("go: ")
  sendKeys("Home", ...Array(128).fill("DC"))
  sendText(absolutePath)
  sendKeys("Enter")
}

/// Create a standalone agent in a plain folder. An empty name means the folder's
/// own name, which is what the screenshots show.
async function createStandaloneAgent(absolutePath, label) {
  fs.mkdirSync(absolutePath, { recursive: true })
  await focusSidebar()
  sendKeys("s")
  await waitFor("Standalone Agent In")
  await browseTo(absolutePath)
  await waitFor("Name standalone agent", 20000)
  sendKeys("Enter")
  await waitFor(`agent "${label}" running`, 60000)
  await sleep(600)
}

async function createAgent(projectIndex, name) {
  // Creating an agent leaves the center pane focused, so a second creation's
  // key would land on a pane that has no binding for it.
  await focusSidebar()
  sendKeys("n")
  await waitFor("New agent in project")
  if (projectIndex > 0) sendKeys(...Array(projectIndex).fill("Down"))
  sendKeys("Enter")
  await waitFor("Name New Agent", 20000)
  sendText(name)
  sendKeys("Enter")
  // Wait for the creation to REPORT, not merely for the row to appear. The row
  // shows up while the worktree is still being made, and dux focuses the new
  // agent's pane when it finishes: a journey that carried on at the row would
  // have its next keystrokes stolen by that focus change.
  //
  // Either report ends the wait. An agent born on a fixture that exits at once
  // (which is how a journey stages the Inactive tail) never shows the created
  // line at all: the error pre-empts it and drops the infos queued behind it.
  await waitForAny([`"${name}" in project`, 'Press "r" to relaunch'], 60000)
  await sleep(600)
  // dux focuses the new agent's pane, and it is interactive there: leave every
  // journey on the sidebar so the next key is a dux key, whatever it is.
  await focusSidebar()
}

/// Select an agent by name rather than by counting rows. Which row an agent
/// lands on is the sidebar's sort talking (working agents float, and the order
/// within a group is not something a journey should be predicting), so the
/// header's own crumb is what this walks the list against.
async function selectAgent(name, maxRows = 24) {
  await focusSidebar()
  sendKeys(...Array(maxRows).fill("Up"))
  await sleep(600)
  for (let i = 0; i < maxRows; i++) {
    if (captureText().includes(`agent: ${name}`)) return
    sendKeys("Down")
    await sleep(300)
  }
  throw new Error(`never reached the agent ${name}\n\n${captureText()}`)
}

async function main() {
  seedState()
  tmux(
    "new-session", "-d", "-c", "/", "-x", String(cols), "-y", String(rows), "-s", session,
    `env DUX_HOME='${duxHome}' DUX_FAKE_FIXTURE='${fixture}' TERM=xterm-256color COLORTERM=truecolor dux`,
  )
  await waitFor("Press a to add a project")
  await addProject("/capture/repos/demo-api", "demo-api")
  await addProject("/capture/repos/demo-web", "demo-web")
  await journey({
    addTab,
    captureText,
    createAgent,
    createStandaloneAgent,
    duxHome,
    focusPane,
    focusSidebar,
    palette,
    repos,
    seedLooseWorktree,
    selectAgent,
    sendKeys,
    sendText,
    setFixture,
    sleep,
    waitFor,
  })
  await sleep(Number(process.env.DUX_TUI_SETTLE_MS || 400))

  const ansi = tmux("capture-pane", "-p", "-e", "-N", "-t", `${session}:0.0`)
  const text = captureText()
  const capturedRows = text.endsWith("\n") ? text.slice(0, -1).split("\n").length : text.split("\n").length
  if (capturedRows !== rows) throw new Error(`captured ${capturedRows} rows; expected ${rows}`)


  fs.writeFileSync(path.join(output, `${outputStem}.ansi`), ansi)
  fs.writeFileSync(path.join(output, `${outputStem}.txt`), text)
  fs.writeFileSync(path.join(output, `${outputStem}.json`), `${JSON.stringify({
    journey: process.env.DUX_TUI_JOURNEY_NAME || path.basename(journeyPath),
    columns: cols,
    rows,
    theme,
    revision: process.env.DUX_PREVIEW_REVISION || "unknown",
    fixture,
  }, null, 2)}\n`)
}

main()
  .catch((error) => {
    console.error(error.stack || String(error))
    try { console.error(`\nFinal terminal grid:\n${captureText()}`) } catch {}
    process.exitCode = 1
  })
  .finally(() => {
    spawnSync("tmux", ["-L", socket, "kill-server"], { stdio: "ignore" })
  })
