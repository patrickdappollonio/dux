// New standalone agent asks for a name, pre-filled exactly while random names
// are on by default, says the folder dux creates,
// leads on to the existing-folder picker at the picker's own height and back
// with the name kept, and creates the agent in a folder dux records as its own.

// Made for this check and deleted after it, with its folder.
const NAME = "checks-quick-agent"

async function removeStray(H) {
  const stray = (await H.lib.agents())[NAME]
  if (stray) {
    await H.lib.api("DELETE", `/api/v1/sessions/${stray.id}?delete_folder=true&force_connected=true`)
  }
}

module.exports = {
  protects:
    "New standalone agent: the name box opens pre-filled while random names are on by default and empty while they are off, and says the folder dux creates; Use an existing folder or project… opens the picker at its own height and Back returns with the name kept; Create agent makes an agent whose folder dux created.",
  async run(ctx, H) {
    ctx.after(() => removeStray(H))
    const dev = await H.device(ctx, "computer")
    await H.goto(dev, "")
    await H.press(dev, { name: "^More ways to start$", selector: "button" })
    await H.press(dev, { name: "^New standalone agent", selector: "[role=menuitem]" })
    const named = await H.waitForDialog(dev, "^New standalone agent$")
    const randomized = (await H.lib.get("/api/v1/bootstrap")).randomize_agent_names_by_default === true
    if (randomized) {
      await H.until(async () => {
        const top = await H.topDialog(dev.page)
        return top && top.fields[0] && top.fields[0].value !== ""
      }, "the name box to be pre-filled with a generated name")
    } else {
      H.expect(
        named.fields[0] && named.fields[0].value === "",
        `with random names off by default the name box should open empty; it holds "${named.fields[0] && named.fields[0].value}"`,
      )
    }
    const help = await dev.page.evaluate(() => document.querySelector('[role="dialog"]')?.textContent ?? "")
    H.expect(/dux creates the folder\./.test(help), `"${named.name}" should say "dux creates the folder."; it says: ${help.slice(0, 300)}`)

    await dev.page.keyboard.down("Control")
    await dev.page.keyboard.press("a")
    await dev.page.keyboard.up("Control")
    await dev.page.keyboard.type(NAME)

    await H.press(dev, { name: "^Use an existing folder or project…$", selector: "button", within: H.DIALOG })
    const picker = await H.waitForDialog(dev, "^Use an existing folder or project$")
    // The picker takes its own height rather than the shorter name step's.
    H.expect(
      picker.box.height > named.box.height + 150,
      `the picker should be taller than the name step it came from (${Math.round(named.box.height)}px); it is ${Math.round(picker.box.height)}px`,
    )
    await H.press(dev, { name: "New standalone agent", selector: "button", within: H.DIALOG })
    const back = await H.waitForDialog(dev, "^New standalone agent$")
    H.expect(back.fields[0] && back.fields[0].value === NAME, `Back should keep the typed name "${NAME}"; the field holds "${back.fields[0] && back.fields[0].value}"`)

    await H.press(dev, { name: "^Create agent$", selector: "button", within: H.DIALOG })
    await H.waitForDialogGone(dev, "^New standalone agent$", "the name step after Create agent")
    const made = await H.until(async () => (await H.lib.agents())[NAME], `agent ${NAME} to appear`, { timeout: 15000 })
    H.expect(made.workspace && made.workspace.kind === "folder", `${NAME} should be a standalone agent; its workspace is ${JSON.stringify(made.workspace)}`)
    H.expect(made.workspace.created_by_dux === true, `${NAME}'s folder should be one dux created; created_by_dux is ${made.workspace.created_by_dux}`)
    H.expect(
      new RegExp(`/agents/${NAME}$`).test(made.workspace.folder_path),
      `${NAME} should run in <dux folder>/agents/${NAME}; it runs in ${made.workspace.folder_path}`,
    )
  },
}
