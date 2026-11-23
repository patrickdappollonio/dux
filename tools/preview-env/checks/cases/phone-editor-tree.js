// On a phone the editor's file tree is the screen until a file is opened, then
// gives the room to the file, and comes back on the header's toggle. A menu
// opened from the tree and closed again leaves it where it was.

const AGENT = "add-rate-limits"
const FILE = "README.md"

// The tree is shown when its entries can be read: the file's own row in it is
// on screen at a readable width. A hidden tree leaves them clipped to a sliver.
async function treeShown(page, file) {
  return page.evaluate((name) => {
    const row = [...document.querySelectorAll("button")].find((b) => window.__checkNameOf(b) === name)
    if (!row) return { shown: false, seen: "no row for it at all" }
    const r = row.getBoundingClientRect()
    const shown = r.width >= 100 && r.x >= 0 && r.right <= innerWidth && r.bottom > 0 && r.y < innerHeight
    return { shown, seen: `its row measures ${Math.round(r.width)}x${Math.round(r.height)} at ${Math.round(r.x)},${Math.round(r.y)}` }
  }, file)
}

async function expectTree(H, page, want, when) {
  await H.until(async () => (await treeShown(page, FILE)).shown === want, `the tree to be ${want ? "shown" : "hidden"}`, { timeout: 3000 }).catch(async () => {
    const now = await treeShown(page, FILE)
    H.fail(`${when}, the file tree should be ${want ? "shown" : "hidden"}; ${now.seen}`)
  })
}

async function openMenu(H, dev) {
  await H.press(dev, { name: "^Explorer actions$", selector: "button" })
  await H.waitFor(dev.page, () => document.querySelector('[role="menu"]') !== null, "the explorer's menu to open")
}

// A tap on a point the open menu (a sheet on a phone) does not cover, found
// rather than assumed, so the tap is outside it.
async function tapOutside(H, dev) {
  const point = await H.waitFor(
    dev.page,
    () => {
      const menu = document.querySelector('[role="menu"]')
      if (!menu) return { ok: false, seen: "no menu" }
      const r = menu.getBoundingClientRect()
      if (r.top > 60) return { x: innerWidth / 2, y: r.top / 2 }
      if (r.bottom < innerHeight - 60) return { x: innerWidth / 2, y: (r.bottom + innerHeight) / 2 }
      return { ok: false, seen: `the menu covers the screen (${Math.round(r.top)} to ${Math.round(r.bottom)})` }
    },
    "room outside the explorer's menu to tap",
  )
  await dev.page.touchscreen.tap(point.x, point.y)
}

module.exports = {
  protects:
    "On a phone the editor shows the file tree with no file open, hides it when a file is opened, shows it again from the header's toggle, and keeps it shown after the explorer's ⋯ menu opens and closes, on a tap outside it and on Escape.",
  async run(ctx, H) {
    const dev = await H.device(ctx, "phone")
    await H.openEditor(dev, await H.agentId(AGENT))
    await expectTree(H, dev.page, true, "With no file open")

    await H.press(dev, { name: `^${FILE}$`, selector: "button" })
    await expectTree(H, dev.page, false, `Once ${FILE} is opened`)

    await H.press(dev, { name: "^Show the file explorer$", selector: "button" })
    await expectTree(H, dev.page, true, "After the header's Show the file explorer")

    // Closed both ways a phone closes it: a tap outside the menu, and Escape.
    await openMenu(H, dev)
    await tapOutside(H, dev)
    await H.waitFor(dev.page, () => document.querySelector('[role="menu"]') === null, "the explorer's menu to close on a tap outside it")
    await expectTree(H, dev.page, true, "After the explorer's ⋯ menu opened and closed on a tap outside it")

    await openMenu(H, dev)
    await dev.page.keyboard.press("Escape")
    await H.waitFor(dev.page, () => document.querySelector('[role="menu"]') === null, "the explorer's menu to close on Escape")
    await expectTree(H, dev.page, true, "After the explorer's ⋯ menu opened and closed on Escape")
  },
}
