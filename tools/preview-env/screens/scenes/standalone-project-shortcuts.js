// The New standalone agent dialog with the project shortcuts over the folder
// explorer: one project picked, so the explorer stands in its folder, and one
// greyed out because a standalone agent already works in its folder.
const {
  agents,
  api,
  boxOf,
  clearToasts,
  containerSh,
  expectDialogOpen,
  expectVisible,
  expectVisibleText,
  get,
  goto,
  openLauncherEntry,
  sleep,
  takeOver,
} = require("../lib.js")

// The seed's workspace has one project. The picture needs a list, so the scene
// adds two more for itself and takes them away again: demo-web, whose
// repository every preview already has, and one whose folder a standalone agent
// works in.
const PICKED = { path: "/repos/demo-web", name: "demo-web" }
const TAKEN = { path: "/repos/billing-service", name: "billing-service" }
const TAKEN_AGENT = "billing-notes"

async function addProject({ path, name }) {
  const list = await get("/api/v1/projects")
  const have = list.find((p) => p.path === path)
  if (have) return have
  const added = await api("POST", "/api/v1/projects", { path, name })
  await api("PATCH", `/api/v1/projects/${added.id}`, { provider: "fake" })
  return added
}

async function stageProjects() {
  containerSh(
    `mkdir -p ${TAKEN.path} && cd ${TAKEN.path}
     [ -d .git ] || { git init -q && echo '# billing-service' > README.md && git add -A && git commit -qm seed; }`,
  )
  await addProject(PICKED)
  await addProject(TAKEN)
  if (!(await agents())[TAKEN_AGENT]) {
    await api("POST", "/api/v1/sessions", {
      kind: "standalone",
      folder: TAKEN.path,
      name: TAKEN_AGENT,
      provider: "fake",
    })
  }
  // The greyed-out row is the server's answer, so it is waited for rather than
  // slept over: the agent is recorded a moment after the request returns.
  const deadline = Date.now() + 30000
  while (Date.now() < deadline) {
    const { rows } = await get("/api/v1/standalone-agent/project-shortcuts")
    if (rows.some((row) => row.name === TAKEN.name && row.reason)) return
    await sleep(500)
  }
  throw new Error(`${TAKEN.name} never became a greyed-out shortcut`)
}

// Back to the seed's workspace: the agent first, because its folder is the
// project's, then both projects. dux removes its own records only.
async function unstageProjects() {
  const agent = (await agents())[TAKEN_AGENT]
  if (agent) {
    await api("DELETE", `/api/v1/sessions/${agent.id}`)
    await sleep(1500)
  }
  for (const { path } of [PICKED, TAKEN]) {
    const project = (await get("/api/v1/projects")).find((p) => p.path === path)
    if (project) await api("DELETE", `/api/v1/projects/${project.id}`)
  }
  containerSh(`rm -rf ${TAKEN.path}`)
  await sleep(800)
}

module.exports = {
  file: "standalone-project-shortcuts.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    // Registered before the staging, so a scene refused half way still hands
    // the next one the seed's workspace. It runs after the shutter, never
    // before: taking the agent away raises a toast and empties the list.
    after(unstageProjects)
    await stageProjects()
    const by = await agents()
    await goto(page, `#/agent/${by["design-notes"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await openLauncherEntry(page, "new-standalone-agent")
    await sleep(1500)
    const row = await page.evaluate((name) => {
      const button = [...document.querySelectorAll("[data-project-shortcut]")].find((b) =>
        b.textContent.includes(name),
      )
      if (!button) return null
      const r = button.getBoundingClientRect()
      return { x: r.x + r.width / 2, y: r.y + r.height / 2 }
    }, PICKED.name)
    if (!row) throw new Error(`no ${PICKED.name} shortcut in the dialog`)
    await page.mouse.click(row.x, row.y)
    await sleep(1500)
    // The picture is of the list and the explorer, not of a focus ring.
    await page.evaluate(() => document.activeElement?.blur())
    await page.mouse.move(4, 4)
    await sleep(400)
    await clearToasts(page)
    // The dialog, the list with its sentence, the row that cannot be picked
    // and why, and the explorer standing in the picked project's folder,
    // which is what the caption says the picture shows.
    await expectDialogOpen(page, "New standalone agent")
    await expectVisibleText(page, "Your projects", { selector: '[role="dialog"] div' })
    await expectVisible(
      page,
      `[role="dialog"] [data-project-shortcut][data-picked]`,
      "the picked project's row",
    )
    await expectVisibleText(page, "already has a standalone agent", {
      selector: '[role="dialog"] [data-shortcut-reason]',
      what: "the greyed-out row's reason",
    })
    await expectVisibleText(page, "not its environment variables or startup command", {
      selector: '[role="dialog"] p',
    })
    await expectVisibleText(page, PICKED.path, {
      selector: '[role="dialog"] code',
      what: "the picked project's folder as the chosen one",
    })
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
