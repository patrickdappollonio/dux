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
  fs.writeFileSync(configPath, config)
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

function sendKeys(...keys) {
  tmux("send-keys", "-t", `${session}:0.0`, ...keys)
}

function sendText(text) {
  tmux("send-keys", "-t", `${session}:0.0`, "-l", text)
}

async function addProject(absolutePath, label) {
  sendKeys("a")
  await waitFor("Add Project: /")
  sendKeys("g")
  await waitFor("go: ")
  sendKeys("Home", ...Array(128).fill("DC"))
  sendText(absolutePath)
  sendKeys("Enter")
  await waitFor(`Added project "${label}" to workspace`, 20000)
}

async function createAgent(projectIndex, name) {
  sendKeys("n")
  await waitFor("New agent in project")
  if (projectIndex > 0) sendKeys(...Array(projectIndex).fill("Down"))
  sendKeys("Enter")
  await waitFor("Name New Agent", 20000)
  sendText(name)
  sendKeys("Enter")
  await waitFor(name, 30000)
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
  await journey({ captureText, createAgent, sendKeys, sendText, sleep, waitFor })
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
