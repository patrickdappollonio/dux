// Runs the browser checks. run.sh calls this; it is not meant to be the entry
// point a person types.
//
//   node run.js --list          one line per check: its name and what it protects
//   node run.js [check...]      run every check, or the ones named
//
// One PASS or FAIL line per check, a screenshot of every page the check had open
// for each failure, and a non-zero exit when anything failed.
const fs = require("fs")
const path = require("path")
const H = require("./harness.js")
const { STANDALONE_FOLDER, seed } = require("../screens/seed.js")

const { lib } = H
const caseDir = path.join(__dirname, "cases")
const failureDir = path.join(__dirname, "failures")

// Past the slowest check with room to spare, so a hang names its check rather
// than stalling the run.
const CHECK_TIMEOUT_MS = 90000

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

// The checks are written against the screenshot scenes' workspace. A workspace
// that already is that (every seeded agent running, the folder agent's folder
// there, both terminals up) is used as it is; anything else is seeded first,
// which relights every agent and takes about half a minute.
async function workspaceProblem() {
  const sessions = await lib.get("/api/v1/sessions")
  const titles = sessions.map((s) => s.title).sort()
  const want = [...lib.SIDEBAR_ORDER].sort()
  if (JSON.stringify(titles) !== JSON.stringify(want)) return `the agents are ${titles.join(", ") || "none"}`
  const dormant = sessions.filter((s) => s.tabs.some((t) => !t.has_live_process)).map((s) => s.title)
  if (dormant.length) return `${dormant.join(", ")} ha${dormant.length > 1 ? "ve" : "s"} a tab with no process`
  // Asked of the container rather than of dux: the folder lives outside the
  // preview's volumes, so a recreated container loses it, and dux's own word on
  // it is only as fresh as its last look.
  try {
    lib.containerSh(`test -d ${STANDALONE_FOLDER}`)
  } catch {
    return "the folder agent's folder is gone"
  }
  const terminals = (await lib.get("/api/v1/workspace")).terminals || []
  for (const kind of ["project", "standalone"]) {
    if (!terminals.some((t) => t.owner.kind === kind)) return `there is no ${kind} terminal`
  }
  return null
}

async function ensureWorkspace() {
  const problem = await workspaceProblem()
  if (!problem) return
  console.log(`seeding the workspace (${problem})`)
  await seed({ pullRequests: false })
  const still = await workspaceProblem()
  if (still) throw new Error(`the workspace is still not the seeded one after seeding: ${still}`)
}

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

async function runOne(check) {
  const ctx = { contexts: [], devices: [], cleanups: [], notes: [], log: (line) => ctx.notes.push(line) }
  ctx.after = (fn) => ctx.cleanups.push(fn)
  const started = Date.now()
  let error = null
  try {
    await withTimeout(check.run(ctx, H), check.name)
  } catch (thrown) {
    error = thrown
  }
  if (check.raisesServerStatuses) H.serverStatusRaised()
  const pictures = error ? await saveFailurePictures(check.name, ctx) : []
  for (const context of ctx.contexts) await context.close().catch(() => {})
  // Cleanups run after the pages are gone, because a page still open can undo
  // what a cleanup puts back (a pane looked at clears an attention flag).
  for (const fn of ctx.cleanups.reverse()) {
    try {
      await fn()
    } catch (cleanupError) {
      ctx.notes.push(`cleanup failed: ${cleanupError.message}`)
    }
  }
  return { error, pictures, notes: ctx.notes, seconds: (Date.now() - started) / 1000 }
}

async function main() {
  const names = process.argv.slice(2)
  if (names[0] === "--list") {
    for (const check of allChecks()) console.log(`${check.name}\t${check.protects}`)
    return
  }
  const checks = names.length ? names.map(load) : allChecks()
  fs.mkdirSync(failureDir, { recursive: true })
  for (const old of fs.readdirSync(failureDir)) {
    if (old.endsWith(".png")) fs.rmSync(path.join(failureDir, old))
  }

  const startedAll = Date.now()
  await lib.api("POST", "/api/v1/first-load/dismiss")
  await ensureWorkspace()

  const failed = []
  try {
    for (const check of checks) {
      const result = await runOne(check)
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

main().catch(async (error) => {
  console.error(`the check run could not start: ${error.message || error}`)
  await H.closeBrowsers()
  process.exit(1)
})
