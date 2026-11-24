// On a phone a long press on a file in the editor's tree opens the menu as a
// card over the bottom of the screen, the way every other menu opens there. The
// finger that opened it is still down over the card, and its lift must choose
// nothing. A tap outside and the browser's Back close it without moving the
// page, a deliberate tap on an item chooses it, and while another device drives
// the agent the card says why its writes are greyed.

const AGENT = "add-rate-limits"
const OTHER = "Firefox on Windows"

// The long press opens the menu at half a second, and how long the finger stays
// down after that is itself part of what is checked, so these waits are fixed.
const HOLD_MS = 800
const AFTER_LIFT_MS = 600

// A phone short enough that the card covers most of the tree, so a row's own
// place on screen is under the card once it opens.
const PHONE_HEIGHT = 340

// Every visible row of the tree, by name and the middle of its box.
function treeRows(page) {
  return page.evaluate(() =>
    [...document.querySelectorAll('[data-testid="file-tree-drop-surface"] li > button')]
      .map((b) => {
        const r = b.getBoundingClientRect()
        return { name: window.__checkNameOf(b), x: r.x + r.width / 2, y: r.y + r.height / 2, height: r.height }
      })
      .filter((row) => row.height > 0 && row.y > 0 && row.y < innerHeight),
  )
}

// The open card, measured: its box against the screen, its rows and reasons.
function card(page) {
  return page.evaluate(() => {
    const menu = [...document.querySelectorAll('[role="menu"]')].find((m) => m.getBoundingClientRect().height > 0)
    if (!menu) return null
    const r = menu.getBoundingClientRect()
    return {
      top: r.top,
      bottom: r.bottom,
      left: r.left,
      right: r.right,
      screen: { width: innerWidth, height: innerHeight },
      backdrop: document.querySelector('[data-slot="context-menu-backdrop"]') !== null,
      items: [...menu.querySelectorAll('[role="menuitem"]')].map((i) => {
        const b = i.getBoundingClientRect()
        return { name: window.__checkNameOf(i), top: b.top, bottom: b.bottom, disabled: i.getAttribute("aria-disabled") === "true" }
      }),
      reasons: [...menu.querySelectorAll('[data-slot="menu-reason"]')].map((e) => e.textContent.replace(/\s+/g, " ").trim()),
    }
  })
}

const hashOf = (page) => page.evaluate(() => location.hash)

async function waitForCard(H, dev, what) {
  return H.until(() => card(dev.page), what, { timeout: 4000 }).then(() => card(dev.page))
}

// Puts a finger down on a point and keeps it there until the card is open.
// Returns the finger, still down.
async function holdOn(H, dev, at) {
  const finger = await dev.page.touchscreen.touchStart(at.x, at.y)
  await H.sleep(HOLD_MS)
  return finger
}

// What a phone's browser may send after the lift, a mouse release and a click
// on whatever lies under the finger. Headless Chromium's touch lift sends
// neither (measured: only pointerup and touchend), so a check that merely lifted
// the finger would pass whatever the card does with them; these are sent by
// hand, as the page would receive them from a browser that does.
function sendLiftClick(page, at) {
  return page.evaluate(({ x, y }) => {
    const target = document.elementFromPoint(x, y)
    for (const type of ["mouseup", "click"]) {
      target?.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window, clientX: x, clientY: y }))
    }
  }, at)
}

async function closeCard(H, dev) {
  await dev.page.keyboard.press("Escape")
  await H.waitFor(dev.page, () => document.querySelector('[role="menu"]') === null, "the card to close on Escape")
}

// A tap on a point the card does not cover, found rather than assumed.
async function tapOutside(H, dev) {
  const shown = await card(dev.page)
  H.expect(shown !== null && shown.top > 40, `there should be room above the card to tap outside it; it starts at ${shown && Math.round(shown.top)}`)
  await dev.page.touchscreen.tap(shown.screen.width / 2, shown.top / 2)
}

module.exports = {
  protects:
    "On a phone a long press on a file in the editor's tree opens its menu as a card over the bottom of the screen that its own opening touch chooses nothing from, a tap outside it and the browser's Back close it without moving the page, a tap on an item chooses it, and while another device drives the agent the card says why its writes are greyed.",
  async run(ctx, H) {
    const dev = await H.device(ctx, "phone", { height: PHONE_HEIGHT })
    await H.openEditor(dev, await H.agentId(AGENT))
    await H.waitFor(dev.page, () => document.querySelector('[data-testid="file-tree-drop-surface"] li > button') !== null, "the file tree's rows")
    const hash = await hashOf(dev.page)

    // A first long press, on the top row, to learn where the card lands.
    const rows = await treeRows(dev.page)
    H.expect(rows.length >= 2, `the tree should list at least two rows to press; it lists ${JSON.stringify(rows.map((r) => r.name))}`)
    let finger = await holdOn(H, dev, rows[0])
    const probe = await waitForCard(H, dev, "a long press on a row to open its menu")
    await finger.end()
    await H.sleep(AFTER_LIFT_MS)

    // The card: the width of the screen, on its bottom edge, over a backdrop.
    H.expect(
      probe.left <= 1 && probe.right >= probe.screen.width - 1 && probe.bottom >= probe.screen.height - 1,
      `a long press's menu should be a card on the bottom edge as wide as the screen; it spans ${Math.round(probe.left)}-${Math.round(probe.right)} across and ends at ${Math.round(probe.bottom)} of ${probe.screen.height}`,
    )
    H.expect(probe.backdrop, "the card should have a backdrop behind it for a tap outside to land on")
    await closeCard(H, dev)

    // The opening touch chooses nothing: the same row is pressed again, at a
    // point of its own box that lies inside one of the card's items, so the
    // finger is over that item when the card opens. The same row, because
    // every kind of row has a menu of its own length and the card measured
    // above is this row's.
    const first = rows[0]
    let under = null
    for (let y = first.y - first.height / 2 + 2; y <= first.y + first.height / 2 - 2 && !under; y++) {
      if (probe.items.some((item) => y > item.top + 4 && y < item.bottom - 4)) under = { name: first.name, x: first.x, y }
    }
    H.expect(
      under,
      `some point of ${first.name}'s row (${Math.round(first.y - first.height / 2)}-${Math.round(first.y + first.height / 2)}) should lie under an item of its card (items at ${JSON.stringify(probe.items.map((i) => [Math.round(i.top), Math.round(i.bottom)]))})`,
    )
    finger = await holdOn(H, dev, under)
    await waitForCard(H, dev, `a long press on ${under.name} to open its menu`)
    const over = await dev.page.evaluate(
      ({ x, y }) => document.elementFromPoint(x, y)?.closest('[role="menuitem"]')?.textContent ?? null,
      under,
    )
    H.expect(over !== null, `the finger on ${under.name} should be over one of the card's items when the card opens; it is over something else`)
    await finger.end()
    await sendLiftClick(dev.page, under)
    // Waiting for something NOT to happen: a click that chose the item under
    // the finger would open a dialog or move the page within this time.
    await H.sleep(AFTER_LIFT_MS)
    H.expect(
      (await card(dev.page)) !== null,
      `lifting the finger that opened the card, over "${over}", should leave the card open; it closed`,
    )
    H.expect(
      (await H.dialogs(dev.page)).length === 0,
      `lifting the finger that opened the card, over "${over}", should choose nothing; a dialog opened`,
    )
    H.expect((await hashOf(dev.page)) === hash, `lifting the finger that opened the card should not move the page; the address is now ${await hashOf(dev.page)}`)

    // Back closes it and the page stays where it was.
    await H.back(dev)
    await H.waitFor(dev.page, () => document.querySelector('[role="menu"]') === null, "the card to close on the browser's Back")
    H.expect((await hashOf(dev.page)) === hash, `Back that closes the card should leave the page where it was; the address is now ${await hashOf(dev.page)}`)

    // A tap outside closes it, and the page stays.
    finger = await holdOn(H, dev, rows[0])
    await waitForCard(H, dev, "the card to open again")
    await finger.end()
    await H.sleep(AFTER_LIFT_MS)
    await tapOutside(H, dev)
    await H.waitFor(dev.page, () => document.querySelector('[role="menu"]') === null, "the card to close on a tap outside it")
    H.expect((await hashOf(dev.page)) === hash, `a tap outside the card should leave the page where it was; the address is now ${await hashOf(dev.page)}`)

    // A later, deliberate tap on an item chooses it.
    finger = await holdOn(H, dev, rows[0])
    await waitForCard(H, dev, "the card to open a third time")
    await finger.end()
    await H.sleep(AFTER_LIFT_MS)
    await H.press(dev, { name: "^New File…$", selector: "[role=menuitem]" })
    await H.waitForDialog(dev, "^New file$")
    await H.press(dev, { name: "^Cancel$", selector: "button", within: H.DIALOG })
    await H.waitForNoDialog(dev, "the New file dialog on Cancel")

    // Another device drives the agent: the card says why its writes are off.
    // The phone's editor holds no terminal of the agent, so the row is free and
    // the other device claims it by arriving; there is no card to take over.
    const other = await H.device(ctx, "computer", { userAgent: H.OTHER_DEVICE_UA })
    await H.goto(other, H.agentHash(await H.agentId(AGENT)))
    await H.drive(other)
    await H.waitFor(dev.page, () => document.body.innerText.includes("is active on"), `the editor to say the agent is active on ${OTHER}`, { timeout: 8000 })

    // The read-only line the editor now shows moves the tree down, so the row is
    // measured again rather than pressed where it was.
    await H.sleep(300)
    const moved = await treeRows(dev.page)
    finger = await holdOn(H, dev, moved[0])
    const blocked = await waitForCard(H, dev, "the card to open while another device drives")
    await finger.end()
    H.expect(
      blocked.reasons.length === 1 && blocked.reasons[0].includes(OTHER),
      `with ${OTHER} driving, the card should say why its writes are greyed, once, naming ${OTHER}; its reasons are ${JSON.stringify(blocked.reasons)}`,
    )
    const writes = blocked.items.filter((i) => /^(New File|New Folder|Rename|Move|Delete)/.test(i.name))
    H.expect(
      writes.length > 0 && writes.every((i) => i.disabled),
      `with ${OTHER} driving, the card's writes should all be disabled; they are ${JSON.stringify(writes.map((i) => [i.name, i.disabled]))}`,
    )
    H.expect(
      blocked.items.some((i) => /info/i.test(i.name) && !i.disabled),
      `the card's info item only reads, so it should stay enabled; the items are ${JSON.stringify(blocked.items.map((i) => [i.name, i.disabled]))}`,
    )
    await closeCard(H, dev)
  },
}
