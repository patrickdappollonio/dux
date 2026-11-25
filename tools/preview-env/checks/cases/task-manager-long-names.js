// The Task Manager keeps every column on screen when an agent has a very long
// name: the name is the one column that gives way, shortened with an ellipsis,
// and nothing in the dialog scrolls sideways. The whole name stays reachable
// (a hover on a computer, a tap on a phone).

const NAMES = [
  "prevent-automation-to-change-agents-or-claude-rules",
  "extremely-long-agent-name-".repeat(4) + "x",
]

// The narrowest window that still gets the table, the same table on a wide
// window, and a phone, which stacks the rows as cards.
const DEVICES = [
  { kind: "computer", width: 1280, height: 800 },
  { kind: "computer", width: 800, height: 600 },
  { kind: "phone", width: 390, height: 844 },
]

const MENU = /^(Settings|Menu)$/

async function createAgents(ctx, H) {
  const project = await H.lib.project("/repos/demo-api")
  // Registered before the create, so an interrupt between the two still runs it.
  ctx.after(async () => {
    const by = await H.lib.agents()
    for (const name of NAMES) {
      if (by[name]) await H.lib.api("DELETE", `/api/v1/sessions/${by[name].id}?delete_worktree=true&force_connected=true`)
    }
  })
  for (const name of NAMES) {
    await H.lib.api("POST", "/api/v1/sessions", {
      kind: "new",
      project_id: project.id,
      name,
      copy_uncommitted_changes: false,
    })
  }
}

// The open dialog, measured: its box and the boxes of everything in its list
// that could be cut off, the sideways scroll of each container, and how each
// long name is drawn.
function measure(page, names) {
  return page.evaluate((names) => {
    const dialog = [...document.querySelectorAll('[role="dialog"]')].filter((d) => !d.hasAttribute("data-closed")).at(-1)
    if (!dialog) return null
    const list = dialog.querySelector('[data-testid="task-manager-scroll"]')
    if (!list) return null
    const box = (e) => {
      const r = e.getBoundingClientRect()
      return { left: r.left, right: r.right }
    }
    const visible = { left: Math.max(box(dialog).left, 0), right: Math.min(box(dialog).right, innerWidth) }
    const listBox = box(list)
    const outside = []
    for (const e of list.querySelectorAll("*")) {
      if (e.children.length > 0 && e.tagName !== "TH" && e.tagName !== "TD") continue
      const r = e.getBoundingClientRect()
      if (r.width < 1 || r.height < 1) continue
      if (r.left < Math.max(visible.left, listBox.left) - 1 || r.right > Math.min(visible.right, listBox.right) + 1) {
        outside.push(`${e.tagName.toLowerCase()} "${e.textContent.trim().slice(0, 30)}" at ${Math.round(r.left)}-${Math.round(r.right)}`)
      }
    }
    const table = list.querySelector("table")
    const headers = table ? [...table.querySelectorAll("th")].map((th) => th.textContent.trim()).filter(Boolean) : []
    const rows = names.map((name) => {
      const stop = [...list.querySelectorAll("button")].find((b) => b.getAttribute("aria-label") === `Force stop ${name}`)
      const label = [...list.querySelectorAll("[data-shortened],span")].find((s) => s.textContent === name)
      const style = label ? getComputedStyle(label) : null
      return {
        name,
        stop: stop ? stop.getBoundingClientRect().width > 0 : false,
        found: Boolean(label),
        cut: label ? label.scrollWidth > label.clientWidth + 1 : false,
        ellipsis: style ? style.textOverflow === "ellipsis" && style.overflow === "hidden" : false,
      }
    })
    return {
      visible,
      outside,
      headers,
      rows,
      sideways: {
        list: [list.scrollWidth, list.clientWidth],
        table: table ? [table.scrollWidth, table.clientWidth] : null,
        dialog: [dialog.scrollWidth, dialog.clientWidth],
      },
    }
  }, names)
}

async function runOn(ctx, H, spec) {
  const where = `${spec.kind} ${spec.width}x${spec.height}`
  const dev = await H.device(ctx, spec.kind, { width: spec.width, height: spec.height })
  await H.goto(dev, "")
  await H.press(dev, { name: MENU.source, selector: "button" })
  await H.press(dev, { name: "^Task Manager", selector: "[role=menuitem]" })
  await H.waitForDialog(dev, "^Task Manager$")
  await H.waitFor(
    dev.page,
    (names) => names.every((n) => [...document.querySelectorAll("button")].some((b) => b.getAttribute("aria-label") === `Force stop ${n}`)),
    `a Force stop for each long-named agent in the Task Manager (${where})`,
    { timeout: 20000, arg: NAMES },
  )
  await H.frames(dev.page)

  const m = await measure(dev.page, NAMES)
  H.expect(m, `the Task Manager should be open on the ${where}; it is not`)
  if (!dev.phone) {
    H.expect(
      m.headers.join(",") === "Name,PID,Procs,CPU,Memory",
      `on the ${where} the table should head its columns Name, PID, Procs, CPU, Memory; it heads ${JSON.stringify(m.headers)}`,
    )
  }
  H.expect(
    m.outside.length === 0,
    `on the ${where} everything in the Task Manager's list should sit inside the dialog's visible box (${Math.round(m.visible.left)}-${Math.round(m.visible.right)}); outside it: ${m.outside.join("; ")}`,
  )
  for (const [what, pair] of Object.entries(m.sideways)) {
    if (!pair) continue
    H.expect(
      pair[0] <= pair[1] + 1,
      `on the ${where} the Task Manager's ${what} should not scroll sideways; its scrollWidth is ${pair[0]} against a clientWidth of ${pair[1]}`,
    )
  }
  for (const row of m.rows) {
    H.expect(row.found, `on the ${where} the Task Manager should list the agent ${row.name}; it does not`)
    H.expect(row.stop, `on the ${where} the row of ${row.name} should show its Force stop button; it has none on screen`)
    H.expect(
      row.cut && row.ellipsis,
      `on the ${where} the name ${row.name} should be shortened with an ellipsis; it is ${row.cut ? "cut with no ellipsis" : "shown whole"}`,
    )
  }

  // The whole name is still reachable: pointed at on a computer, tapped on a phone.
  const label = (name) => ({ name: `^${name}$`, selector: "[data-shortened]" })
  const longest = NAMES[1]
  await (dev.phone ? H.press(dev, label(longest)) : H.hover(dev, label(longest)))
  const tip = await H.until(
    () =>
      dev.page.evaluate((name) => {
        const t = [...document.querySelectorAll('[data-slot="tooltip-content"]')].find((e) => e.textContent === name)
        if (!t) return null
        const r = t.getBoundingClientRect()
        return { left: r.left, right: r.right }
      }, longest),
    `the full name ${longest} in a tooltip on the ${where}`,
    { timeout: 4000 },
  )
  H.expect(
    tip.left >= 0 && tip.right <= dev.width,
    `on the ${where} the tooltip with the full name should sit inside the screen; it spans ${Math.round(tip.left)}-${Math.round(tip.right)} of ${dev.width}`,
  )
}

module.exports = {
  protects:
    "In the Task Manager, an agent with a very long name keeps every column and its Force stop button inside the dialog on a wide computer, a narrow computer and a phone, nothing scrolls sideways, the name is shortened with an ellipsis, and its full name shows in a tooltip.",
  async run(ctx, H) {
    await createAgents(ctx, H)
    for (const spec of DEVICES) await runOn(ctx, H, spec)
  },
}
