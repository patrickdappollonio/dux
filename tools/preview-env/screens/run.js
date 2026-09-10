// Shoots the browser scenes. reshoot.sh calls this; it is not meant to be the
// entry point a person types.
//
//   node run.js --list             one line per scene, for the shell to read
//   node run.js <scene> [scene...] shoot these browser scenes
//
// Scenes are grouped by the staging they ask for, so a run shooting the whole
// set relights the workspace once per staging rather than once per picture.
const fs = require("fs")
const path = require("path")
const { stage } = require("./lib.js")
const lib = require("./lib.js")

const sceneDir = path.join(__dirname, "scenes")
const outDir = process.env.SCREENS_DIR || path.resolve(__dirname, "../../../website/public/screens")

const DEFAULT_STAGING = "all-working"

function load(name) {
  const file = path.join(sceneDir, `${name}.js`)
  if (!fs.existsSync(file)) throw new Error(`no scene named ${name}`)
  const mod = require(file)
  // A terminal UI scene is the journey function itself, with its capture
  // settings hung off it; a browser scene is an object with a shoot method.
  const kind = typeof mod === "function" ? "tui" : "web"
  if (kind === "web" && typeof mod.shoot !== "function") {
    throw new Error(`scene ${name} exports neither a journey function nor a shoot method`)
  }
  if (mod.file !== `${name}.png`) {
    throw new Error(`scene ${name} names its file ${mod.file}; it must be ${name}.png`)
  }
  return { name, kind, mod }
}

function allScenes() {
  return fs
    .readdirSync(sceneDir)
    .filter((f) => f.endsWith(".js"))
    .map((f) => f.slice(0, -3))
    .sort()
    .map(load)
}

function list() {
  for (const scene of allScenes()) {
    const m = scene.mod
    const fields =
      scene.kind === "tui"
        ? [m.cols || 160, m.rows || 45, m.theme || "dux_dark", m.crop || ""]
        : [m.viewport, m.staging || DEFAULT_STAGING]
    console.log([scene.name, scene.kind, m.file, ...fields].join("\t"))
  }
}

async function shootOne(scene) {
  const mobile = scene.mod.viewport === "phone"
  const { browser, page, reassert } = await lib.open({ mobile })
  try {
    const clip = await scene.mod.shoot(page, { open: lib.open, mobile })
    await reassert()
    const out = path.join(outDir, scene.mod.file)
    await page.screenshot({ path: out, clip })
    return out
  } finally {
    await browser.close()
  }
}

async function main() {
  const names = process.argv.slice(2)
  if (names[0] === "--list") return list()

  const scenes = (names.length ? names.map(load) : allScenes()).filter((s) => s.kind === "web")
  if (!scenes.length) return

  // Least churn: every staging's scenes in one block, the default one first so a
  // freshly seeded workspace is already in the right state.
  scenes.sort((a, b) => {
    const sa = a.mod.staging || DEFAULT_STAGING
    const sb = b.mod.staging || DEFAULT_STAGING
    if (sa === sb) return a.name.localeCompare(b.name)
    if (sa === DEFAULT_STAGING) return -1
    if (sb === DEFAULT_STAGING) return 1
    return sa.localeCompare(sb)
  })

  fs.mkdirSync(outDir, { recursive: true })
  let staged = DEFAULT_STAGING
  const failures = []
  for (const scene of scenes) {
    const want = scene.mod.staging || DEFAULT_STAGING
    if (want !== staged) {
      console.log(`staging ${want}`)
      await stage(want)
      staged = want
    }
    try {
      const out = await shootOne(scene)
      console.log("wrote", path.basename(out))
    } catch (error) {
      console.error(`FAILED ${scene.name}: ${String(error)}`)
      failures.push(scene.name)
    }
  }
  // Leave the workspace in the state the seed leaves it in, so a second run and
  // the terminal UI journeys after this one both start from the same place.
  if (staged !== DEFAULT_STAGING) {
    console.log(`restaging ${DEFAULT_STAGING}`)
    await stage(DEFAULT_STAGING)
  }
  if (failures.length) {
    console.error(`\n${failures.length} scene(s) failed: ${failures.join(", ")}`)
    process.exitCode = 1
  }
}

main().catch((error) => {
  console.error(String(error))
  process.exit(1)
})
