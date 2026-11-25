// Runs the browser checks. run.sh calls this once the preview runs a binary
// built from this checkout; it is not meant to be the entry point a person types.
//
//   node run.js --list              one line per check: its name and what it protects
//   node run.js [--reseed] [check...]  run every check, or the ones named
//
// One PASS or FAIL line per check, a screenshot of every page the check had open
// for each failure, and a non-zero exit when anything failed. Ctrl-C (or a
// SIGTERM) closes the pages, runs the clean-up of the check under way and closes
// the browsers before leaving; a second one leaves at once.
const fs = require("fs")
const path = require("path")
const H = require("./harness.js")
const { STANDALONE_FOLDER, seed } = require("../screens/seed.js")

const { lib } = H
const caseDir = path.join(__dirname, "cases")
const failureDir = path.join(__dirname, "failures")
const readme = path.join(__dirname, "README.md")

// Past the slowest check with room to spare, so a hang names its check rather
// than stalling the run.
const CHECK_TIMEOUT_MS = 90000
// How long the server may take to notice a closed page and let go of what it
// drove.
const RELEASE_TIMEOUT_MS = 15000

// What the checks can change in the workspace, so the start of a run can tell
// a leftover from an interrupted run and put it back.
const PROJECT_REPO = "/repos/demo-api"
const WORKTREES = "/data/dux/worktrees/demo-api"
// The seed writes changed files into these two; the other agents' worktrees are
// clean.
const WRITTEN_WORKTREES = new Set(["fix-login-redirect", "add-rate-limits"])
// A check that makes a branch names it with this prefix, and a check that
// changes the config file copies it here first; either is put back at the start
// of the next run if the check could not put it back itself.
const CHECK_BRANCH_PREFIX = "checks-"
const CONFIG_FILE = "/data/dux/config.toml"
const CONFIG_BACKUP = "/data/checks-config.toml.orig"
// Tabs each seeded agent has; an agent not named here has its one first tab.
const TAB_COUNTS = { "refactor-cache": 3 }
const tabsSeeded = (title) => TAB_COUNTS[title] ?? 1

function load(name) {
  const file = path.join(caseDir, `${name}.js`)
  if (!fs.existsSync(file)) throw new Error(`no check named ${name} (try --list)`)
  const mod = require(file)
  if (typeof mod.run !== "function" || typeof mod.protects !== "string") {
    throw new Error(`check ${name} must export { protects, run }`)
  }
  return { name, ...mod }
}

function allChecks() {
  return fs
    .readdirSync(caseDir)
    .filter((f) => f.endsWith(".js"))
    .map((f) => f.slice(0, -3))
    .sort()
    .map(load)
}

// The README's table of checks says what each one protects in the check's own
// words. Run before anything else, so the two cannot drift apart.
function tableProblems(checks) {
  const rows = new Map()
  for (const line of fs.readFileSync(readme, "utf8").split("\n")) {
    const m = /^\| `([a-z0-9-]+)` \| [^|]+ \| (.+) \|$/.exec(line)
    if (m) rows.set(m[1], m[2].trim())
  }
  const problems = []
  for (const check of checks) {
    if (!rows.has(check.name)) problems.push(`${check.name} has no row in the README's table`)
    else if (rows.get(check.name) !== check.protects) {
      problems.push(`${check.name}: the README says "${rows.get(check.name)}" and the check says "${check.protects}"`)
    }
    rows.delete(check.name)
  }
  for (const name of rows.keys()) problems.push(`the README's table lists ${name}, which is not a check`)
  return problems
}

// --- The workspace ----------------------------------------------------------

// Whether the workspace is the seeded one the checks are written against, and
// carries nothing a check left behind. Null when it is; otherwise why not.
async function workspaceProblem() {
  const sessions = await lib.get("/api/v1/sessions")
  const titles = sessions.map((s) => s.title).sort()
  const want = [...lib.SIDEBAR_ORDER].sort()
  if (JSON.stringify(titles) !== JSON.stringify(want)) return `the agents are ${titles.join(", ") || "none"}`
  const dormant = sessions.flatMap((s) =>
    s.tabs
      .filter((t) => !t.has_live_process)
      .map((t) => `${s.title}'s ${t.provider} tab${t.last_run_verdict ? ` (${JSON.stringify(t.last_run_verdict)})` : ""}`),
  )
  if (dormant.length) return `${dormant.join(", ")} ha${dormant.length > 1 ? "ve" : "s"} no process`
  for (const s of sessions) {
    if (s.tabs.length !== tabsSeeded(s.title)) {
      return `${s.title} has ${s.tabs.length} tabs rather than ${tabsSeeded(s.title)}`
    }
  }
  const terminals = (await lib.get("/api/v1/workspace")).terminals || []
  const docked = terminals.find((t) => t.owner.kind === "session")
  if (docked) return `an agent has a terminal of its own (${docked.id}), which no seed makes`
  for (const kind of ["project", "standalone"]) {
    if (!terminals.some((t) => t.owner.kind === kind)) return `there is no ${kind} terminal`
  }
  // Asked of the container rather than of dux: the folder lives outside the
  // preview's volumes, so a recreated container loses it, and dux's own word on
  // it is only as fresh as its last look.
  const fact = lib
    .containerSh(
      `test -d ${STANDALONE_FOLDER} || echo "the folder agent's folder is gone"
       test ! -e ${CONFIG_BACKUP} || echo "a check's change to the config file was never put back"
       for ref in $(git -C ${PROJECT_REPO} for-each-ref --format='%(refname:short)' 'refs/heads/${CHECK_BRANCH_PREFIX}*'); do
         echo "a check's branch $ref is still in the project"
       done
       for title in ${lib.SIDEBAR_ORDER.filter((t) => t !== "design-notes").join(" ")}; do
         own=$(git -C ${WORKTREES}/$title rev-list --count HEAD --not --exclude=$title --branches)
         [ "$own" = 0 ] || echo "$title has $own commit(s) on no other branch"
       done`,
    )
    .trim()
  return fact ? fact.split("\n")[0] : null
}

// Everything a check can leave behind, put back, then the seed: the way back to
// the workspace the checks are written against from wherever a check (or an
// interrupted run) left it.
async function backToSeed() {
  const by = await lib.agents()
  for (const [title, session] of Object.entries(by)) {
    if (lib.SIDEBAR_ORDER.includes(title)) continue
    // A standalone agent has no worktree to remove; its folder goes when dux
    // created it and stays when it did not.
    const worktree = session.workspace?.kind === "managed"
    await lib.api("DELETE", `/api/v1/sessions/${session.id}?delete_worktree=${worktree}&force_connected=true`)
    console.log(`removed the agent ${title}, which no seed makes`)
  }
  for (const s of await lib.get("/api/v1/sessions")) {
    for (const tab of s.tabs.slice(tabsSeeded(s.title))) {
      await lib.api("DELETE", `/api/v1/sessions/${s.id}/tabs/${tab.id}?force_connected=true`)
      console.log(`closed ${s.title}'s extra ${tab.provider} tab, which no seed makes`)
    }
  }
  for (const t of (await lib.get("/api/v1/workspace")).terminals || []) {
    if (t.owner.kind !== "session") continue
    const id = t.owner.session_id ?? t.owner.sessionId
    await lib.api("DELETE", `/api/v1/sessions/${id}/terminals/${t.id}?force_connected=true`)
    console.log(`closed the terminal ${t.id} of an agent, which no seed makes`)
  }
  const undone = lib
    .containerSh(
      `if [ -e ${CONFIG_BACKUP} ]; then
         cp ${CONFIG_BACKUP} ${CONFIG_FILE} && rm ${CONFIG_BACKUP} && echo "put the config file back"
       fi
       for ref in $(git -C ${PROJECT_REPO} for-each-ref --format='%(refname:short)' 'refs/heads/${CHECK_BRANCH_PREFIX}*'); do
         git -C ${PROJECT_REPO} branch -q -D -- "$ref" && echo "deleted the branch $ref"
       done
       for title in ${lib.SIDEBAR_ORDER.filter((t) => t !== "design-notes").join(" ")}; do
         cd ${WORKTREES}/$title
         first=$(git rev-list --reverse HEAD --not --exclude=$title --branches | head -1)
         if [ -n "$first" ]; then
           git reset -q --hard "$first~1" && echo "took $title back to before its own commits"
         fi
         case " ${[...WRITTEN_WORKTREES].join(" ")} " in
           *" $title "*) ;;
           *) git reset -q --hard && git clean -qfd ;;
         esac
       done`,
    )
    .trim()
  if (undone) console.log(undone)
  if (/config file/.test(undone)) await lib.api("POST", "/api/v1/config/reload")
  await seed({ pullRequests: false })
}

async function ensureWorkspace(force) {
  const problem = force ? "--reseed" : await workspaceProblem()
  if (!problem) return
  console.log(`putting the workspace back to the seed (${problem})`)
  await backToSeed()
  const still = await workspaceProblem()
  if (still) throw new Error(`the workspace is still not the seeded one after seeding: ${still}`)
}

// Every tab and terminal some device drives, by name.
async function driven() {
  const out = []
  for (const s of await lib.get("/api/v1/sessions")) {
    if (s.tabs.some((t) => t.input_owner)) out.push(s.title)
  }
  for (const t of (await lib.get("/api/v1/workspace")).terminals || []) {
    if (t.input_owner) out.push(`terminal ${t.id}`)
  }
  return out
}

// --- One check ----------------------------------------------------------------

function withTimeout(promise, name) {
  let timer
  const bell = new Promise((_, reject) => {
    timer = setTimeout(
      () => reject(new H.CheckFailure(`${name} did not finish within ${CHECK_TIMEOUT_MS / 1000}s`)),
      CHECK_TIMEOUT_MS,
    )
  })
  return Promise.race([promise, bell]).finally(() => clearTimeout(timer))
}

async function saveFailurePictures(name, ctx) {
  const saved = []
  for (const [at, dev] of ctx.devices.entries()) {
    const file = path.join(failureDir, `${name}${ctx.devices.length > 1 ? `-${at + 1}-${dev.kind}` : ""}.png`)
    try {
      await dev.page.screenshot({ path: file })
      saved.push(path.relative(process.cwd(), file))
    } catch {
      // A page that crashed has nothing to show; the reason says enough.
    }
  }
  return saved
}

// Closes the check's pages, then runs its clean-ups, newest first. Cleanups run
// after the pages are gone, because a page still open can undo what a cleanup
// puts back (a pane looked at clears an attention flag). Safe to call twice:
// an interrupt during a check's own finish waits for the same finish.
function finish(ctx) {
  if (!ctx.finishing) {
    ctx.finishing = (async () => {
      for (const context of ctx.contexts) await context.close().catch(() => {})
      // With its pages gone, an interrupted check has nothing left to act on, so
      // its clean-up may wait for what it puts back.
      H.halt(false)
      while (ctx.cleanups.length) {
        const fn = ctx.cleanups.pop()
        try {
          await fn()
        } catch (cleanupError) {
          ctx.notes.push(`cleanup failed: ${cleanupError.message}`)
        }
      }
    })()
  }
  return ctx.finishing
}

let current = null

async function runOne(check) {
  const ctx = { contexts: [], devices: [], cleanups: [], notes: [], log: (line) => ctx.notes.push(line) }
  ctx.after = (fn) => ctx.cleanups.push(fn)
  current = ctx
  const started = Date.now()
  let error = null
  try {
    await withTimeout(check.run(ctx, H), check.name)
  } catch (thrown) {
    error = thrown
  }
  const pictures = error ? await saveFailurePictures(check.name, ctx) : []
  await finish(ctx)
  current = null
  // The next check's device must arrive at panes nobody drives, so it is never
  // met by this check's take-over card.
  const left = await H.until(async () => ((await driven()).length ? false : true), "", {
    timeout: RELEASE_TIMEOUT_MS,
  }).catch(() => null)
  if (!left && !error) {
    error = new H.CheckFailure(
      `${(await driven()).join(", ")} still had a device driving it ${RELEASE_TIMEOUT_MS / 1000}s after this check's pages closed`,
    )
  }
  return { error, pictures, notes: ctx.notes, seconds: (Date.now() - started) / 1000 }
}

// --- The run -----------------------------------------------------------------

let leaving = false
// What the run waits on once it is leaving: `leave` ends the process when its
// clean-up is done, and nothing else may report or start in the meantime.
const parked = () => new Promise(() => {})

async function leave(why, code) {
  if (leaving) {
    console.log(`${why} again: leaving without finishing the clean-up`)
    process.exit(code)
  }
  leaving = true
  H.halt()
  console.log(`\n${why}: closing the pages and putting back what the check under way changed (send it again to leave at once)`)
  try {
    if (current) {
      await finish(current)
      for (const note of current.notes) console.log(`     note: ${note}`)
    }
  } finally {
    await H.closeBrowsers()
  }
  process.exit(code)
}

async function main() {
  const args = process.argv.slice(2)
  if (args[0] === "--list") {
    for (const check of allChecks()) console.log(`${check.name}\t${check.protects}`)
    const problems = tableProblems(allChecks())
    if (problems.length) throw new Error(`the README's table of checks is out of step:\n  ${problems.join("\n  ")}`)
    return
  }
  const reseed = args.includes("--reseed")
  const names = args.filter((a) => a !== "--reseed")
  const unknown = names.filter((a) => a.startsWith("--"))
  if (unknown.length) throw new Error(`unknown option ${unknown.join(", ")}`)
  const problems = tableProblems(allChecks())
  if (problems.length) throw new Error(`the README's table of checks is out of step:\n  ${problems.join("\n  ")}`)
  const checks = names.length ? names.map(load) : allChecks()
  fs.mkdirSync(failureDir, { recursive: true })
  for (const old of fs.readdirSync(failureDir)) {
    if (old.endsWith(".png")) fs.rmSync(path.join(failureDir, old))
  }

  const startedAll = Date.now()
  await lib.api("POST", "/api/v1/first-load/dismiss")
  await ensureWorkspace(reseed)

  const failed = []
  try {
    for (const check of checks) {
      if (leaving) await parked()
      const result = await runOne(check)
      if (leaving) await parked()
      const time = `${result.seconds.toFixed(1)}s`
      if (!result.error) {
        console.log(`PASS ${check.name} (${time})`)
      } else {
        failed.push(check.name)
        const why =
          result.error instanceof H.CheckFailure
            ? result.error.message
            : `the check itself broke: ${result.error.stack || result.error}`
        console.log(`FAIL ${check.name} (${time}): ${why}`)
        for (const picture of result.pictures) console.log(`     screenshot: ${picture}`)
      }
      for (const note of result.notes) console.log(`     note: ${note}`)
    }
  } finally {
    await H.closeBrowsers()
  }
  const total = ((Date.now() - startedAll) / 1000).toFixed(1)
  console.log(
    failed.length
      ? `\n${failed.length} of ${checks.length} checks failed in ${total}s: ${failed.join(", ")}`
      : `\nall ${checks.length} checks passed in ${total}s`,
  )
  if (failed.length) process.exitCode = 1
}

process.on("SIGINT", () => leave("interrupted", 130))
process.on("SIGTERM", () => leave("terminated", 143))
process.on("SIGHUP", () => leave("hung up", 129))
process.on("uncaughtException", (error) => {
  console.error(`the check run crashed: ${error.stack || error}`)
  leave("crashed", 1)
})
process.on("unhandledRejection", (error) => {
  console.error(`the check run crashed: ${(error && error.stack) || error}`)
  leave("crashed", 1)
})

main().catch(async (error) => {
  if (leaving) await parked()
  console.error(`the check run could not finish: ${error.message || error}`)
  if (current) await finish(current)
  await H.closeBrowsers()
  process.exit(1)
})
