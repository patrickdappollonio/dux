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
const lib = require("./lib.js")

const sceneDir = path.join(__dirname, "scenes")
const outDir = process.env.SCREENS_DIR || path.resolve(__dirname, "../../../website/public/screens")

const DEFAULT_STAGING = "all-working"

// A scene that hangs (a selector that never appears, a socket that never opens)
// would otherwise stall the whole run with no clue which one it was. Comfortably
// past the slowest scene, which restarts an agent and waits out a fixture.
const SCENE_TIMEOUT_MS = 120000

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

// Reject if the scene has not produced a clip in time, naming the scene: a
// bare hang says nothing about which of forty-one is stuck.
function withTimeout(promise, name) {
  let timer
  const bell = new Promise((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`${name} did not finish within ${SCENE_TIMEOUT_MS / 1000}s`)),
      SCENE_TIMEOUT_MS,
    )
  })
  return Promise.race([promise, bell]).finally(() => clearTimeout(timer))
}

// Park the working cue on one chosen frame before the shutter opens.
//
// A still cannot show an animation, so it has to stand for one, and left alone
// every capture lands on the same unlucky frame: the animations start when the
// page mounts and the tool shoots at a fixed offset after that, so the docs all
// came back on the zero-dot step, showing a bare state word with an empty slot
// beside it that reads as a spacing bug rather than as a cue mid-cycle.
//
// The frame chosen is three dots with the glyph and word at full brightness.
// That pairing is deliberately NOT a real frame of the live cue, where the dots
// and the dip share one clock and three dots arrive while the word is halfway
// down: a dimmed word in a still reads as a rendering fault, and what the
// picture is for is showing the reader what the row says. Applied to every web
// scene here rather than scene by scene, so a new one cannot forget.
async function freezeWorkingCue(page) {
  await page.addStyleTag({
    content: `
      .working-dots::after {
        animation: none !important;
        clip-path: inset(0 0 0 0) !important;
      }
      [class*="animate-working-pulse"] {
        animation: none !important;
        opacity: 1 !important;
      }
    `,
  })
}

async function shootOne(scene) {
  const mobile = scene.mod.viewport === "phone"
  const { browser, page, reassert } = await lib.open({ mobile })
  // A scene that opens a second browser must not close it before the capture: a
  // second device leaving is a thing the page reacts to, and the take-over card
  // re-titles the moment its driver disconnects. So teardown is registered here
  // and run after the shot rather than in the scene's own `finally`. It runs
  // after this browser closes too, because a hook that puts back state a scene
  // disturbed (an attention flag the pane cleared by being looked at) would be
  // undone again by a page still watching that pane.
  const afterShot = []
  try {
    const clip = await withTimeout(
      scene.mod.shoot(page, { open: lib.open, mobile, after: (fn) => afterShot.push(fn) }),
      scene.name,
    )
    await reassert()
    await freezeWorkingCue(page)
    const out = path.join(outDir, scene.mod.file)
    await page.screenshot({ path: out, clip })
    return out
  } finally {
    await browser.close()
    for (const fn of afterShot) await fn()
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
  try {
    for (const scene of scenes) {
      const want = scene.mod.staging || DEFAULT_STAGING
      try {
        if (want !== staged) {
          console.log(`staging ${want}`)
          // Recorded BEFORE the attempt: a staging that fails half way through
          // has already relit some agents, so the workspace is no longer the one
          // it was, and the restage below has to run.
          staged = want
          await lib.stage(want)
        }
        const out = await shootOne(scene)
        console.log("wrote", path.basename(out))
      } catch (error) {
        console.error(`FAILED ${scene.name}: ${String(error)}`)
        failures.push(scene.name)
      }
    }
  } finally {
    // Leave the workspace in the state the seed leaves it in, so a second run
    // and the terminal UI journeys after this one both start from the same
    // place. In a finally, because an abandoned run owes the next one the same.
    if (staged !== DEFAULT_STAGING) {
      console.log(`restaging ${DEFAULT_STAGING}`)
      await lib.stage(DEFAULT_STAGING)
    }
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
