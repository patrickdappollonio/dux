// The New agent form sends one create however often Enter is pressed while it
// waits, and a name that is already a branch asks before attaching to it; every
// way out of that question goes back to the form with the name still typed.

const PROJECT = "demo-api"
const REPO = `/repos/${PROJECT}`
// Made for this check and deleted after it. The runner treats any branch named
// "checks-…" as a leftover and deletes it at the start of a run.
const EXISTING_BRANCH = "checks-existing-branch"
const HELD_NAME = "checks-held-create"

const isCreate = (request) => request.method() === "POST" && new URL(request.url()).pathname === "/api/v1/sessions"

async function openForm(H, dev) {
  await H.goto(dev, "")
  await H.press(dev, { name: "^New agent…$", selector: "button" })
  await H.waitForDialog(dev, "^New agent$")
  await H.press(dev, { name: `^${PROJECT}$`, selector: "button", within: H.DIALOG })
  const form = await H.waitForDialog(dev, `^New agent in ${PROJECT}$`)
  const field = form.fields[0]
  H.expect(field, `"${form.name}" has no name field`)
  return form
}

async function nameValue(H, dev) {
  const top = await H.topDialog(dev.page)
  return { top, value: top && top.fields[0] ? top.fields[0].value : null }
}

// No agent may be left behind under either name, nor its working copy: the
// workspace the scenes are shot against holds exactly the seeded ones.
async function removeStray(H) {
  for (const name of [HELD_NAME, EXISTING_BRANCH]) {
    const stray = (await H.lib.agents())[name]
    if (stray) await H.lib.api("DELETE", `/api/v1/sessions/${stray.id}?delete_worktree=true&force_connected=true`)
  }
}

module.exports = {
  protects:
    "New agent: with the create held, three Enters send exactly one POST /api/v1/sessions; an existing branch's name raises \"Attach to existing branch?\", and Cancel, Escape and the browser's Back each return to the form with the name kept.",
  async run(ctx, H) {
    ctx.after(() =>
      H.lib.containerSh(
        `if git -C ${REPO} rev-parse --verify --quiet refs/heads/${EXISTING_BRANCH} >/dev/null; then git -C ${REPO} branch -q -D -- ${EXISTING_BRANCH}; fi`,
      ),
    )
    // Run first (clean-ups run newest first): an agent on the branch would keep it
    // checked out.
    ctx.after(() => removeStray(H))
    H.lib.containerSh(`git -C ${REPO} branch -- ${EXISTING_BRANCH}`)

    // Held: the create is caught and never answered, so the form stays busy.
    const held = await H.device(ctx, "computer")
    const creates = []
    await held.page.setRequestInterception(true)
    held.page.on("request", (request) => {
      if (isCreate(request)) {
        creates.push(request)
        return
      }
      request.continue()
    })
    ctx.after(() => Promise.all(creates.map((r) => r.abort().catch(() => {}))))
    await openForm(H, held)
    await held.page.keyboard.type(HELD_NAME)
    // Three Enters at a person's pace, each a separate press.
    for (let i = 0; i < 3; i++) {
      await held.page.keyboard.press("Enter")
      await H.sleep(150)
    }
    await H.until(() => creates.length > 0, "the form to send its create", { timeout: 3000 }).catch(() =>
      H.fail("three Enters in the New agent form sent no create at all"),
    )
    // A wait for nothing more to happen: long enough for a second create to have
    // left, had the form sent one.
    await H.sleep(700)
    H.expect(creates.length === 1, `three Enters while the create was held should send exactly one POST /api/v1/sessions; ${creates.length} were sent`)

    // An existing branch: the server refuses and the form asks. Nothing may
    // attach, so a create that says yes is refused here before it leaves.
    const dev = await H.device(ctx, "computer")
    const attached = []
    await dev.page.setRequestInterception(true)
    dev.page.on("request", (request) => {
      if (isCreate(request) && /"use_existing_branch":\s*true/.test(request.postData() || "")) {
        attached.push(request)
        return request.abort()
      }
      request.continue()
    })
    const form = await openForm(H, dev)
    await dev.page.keyboard.type(EXISTING_BRANCH)

    const ways = {
      Cancel: () => H.press(dev, { name: "^Cancel$", selector: "button", within: H.DIALOG }),
      Escape: () => dev.page.keyboard.press("Escape"),
      "the browser's Back": () => H.back(dev),
    }
    for (const [way, take] of Object.entries(ways)) {
      await dev.page.keyboard.press("Enter")
      const question = await H.waitForDialog(dev, "^Attach to existing branch").catch(async () => {
        const top = await H.topDialog(dev.page)
        H.fail(`creating "${EXISTING_BRANCH}", an existing branch, should ask "Attach to existing branch ${EXISTING_BRANCH}?"; ${top ? `"${top.name}" is on top` : "no dialog is open"}`)
      })
      await take()
      await H.waitForDialogGone(dev, "^Attach to existing branch", `the attach question on ${way}`)
      const { top, value } = await nameValue(H, dev)
      H.expect(
        top && top.name === form.name,
        `${way} on "${question.name}" should go back to "${form.name}"; ${top ? `"${top.name}" is on top` : "no dialog is open"}`,
      )
      H.expect(value === EXISTING_BRANCH, `${way} should keep the typed name "${EXISTING_BRANCH}"; the field holds "${value}"`)
      H.expect(top.focusInside, `${way} should leave focus inside "${form.name}"; it is on ${(await H.focused(dev.page)).what}`)
    }
    H.expect(attached.length === 0, "no way out of the attach question may attach, and one sent a create that attaches")
    H.expect(!(await H.lib.agents())[EXISTING_BRANCH], `no agent named ${EXISTING_BRANCH} may exist after leaving the question; one does`)
  },
}
