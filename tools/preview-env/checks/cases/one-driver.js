// When another device takes an agent over, this device can no longer change
// it: the open confirm's action and the agent menu's changing entries disable,
// and each says which device is driving.
const { AGENT } = require("../dialogs.js")

// The name the server gives the second device, from its user agent.
const OTHER = "Firefox on Windows"
const CHANGING = ["Detach agent…", "Rename agent…", "Delete agent…"]

module.exports = {
  protects:
    "With a second device having taken over an agent, a confirm that was open on the first device has its action disabled with a reason naming that device, and the agent menu's changing entries are disabled under a reason naming it.",
  async run(ctx, H) {
    const id = await H.agentId(AGENT)
    const first = await H.device(ctx, "computer")
    await H.goto(first, H.agentHash(id))
    await H.drive(first)
    await H.paneMenu(first, "^Detach agent…$")
    const confirm = await H.waitForDialog(first, "^Detach agent\\?$")
    H.expect(
      confirm.buttons.some((b) => b.name === "Detach" && !b.disabled),
      `while this device drives, "${confirm.name}" should offer Detach enabled; its buttons are ${JSON.stringify(confirm.buttons.map((b) => b.name + (b.disabled ? " (disabled)" : "")))}`,
    )

    const second = await H.device(ctx, "computer", { userAgent: H.OTHER_DEVICE_UA })
    await H.goto(second, H.agentHash(id))
    await H.takeOver(second)

    await H.until(async () => {
      const top = await H.topDialog(first.page)
      return top && top.buttons.some((b) => b.name === "Detach" && b.disabled)
    }, "Detach to disable", { timeout: 6000 }).catch(async () => {
      const top = await H.topDialog(first.page)
      H.fail(
        `after ${OTHER} took the agent over, the open "${confirm.name}" should have its Detach disabled; ${top ? `its buttons are ${JSON.stringify(top.buttons.map((b) => b.name + (b.disabled ? " (disabled)" : "")))}` : "the dialog is gone"}`,
      )
    })
    const reason = await H.reasonFor(first, "Detach")
    H.expect(reason.includes(OTHER), `the disabled Detach's reason should name ${OTHER}; it says "${reason}"`)

    await H.press(first, { name: "^Cancel$", selector: "button", within: H.DIALOG })
    await H.waitForNoDialog(first, `"${confirm.name}" on Cancel`)
    await H.paneMenu(first)
    const menu = await H.waitFor(
      first.page,
      () => {
        const menu = [...document.querySelectorAll('[role="menu"]')].find((m) => m.getBoundingClientRect().height > 0)
        if (!menu) return null
        return {
          text: menu.textContent.replace(/\s+/g, " "),
          items: [...menu.querySelectorAll('[role="menuitem"]')].map((i) => ({
            name: window.__checkNameOf(i),
            disabled: i.getAttribute("aria-disabled") === "true" || i.hasAttribute("data-disabled"),
          })),
        }
      },
      "the agent menu to open",
    )
    for (const name of CHANGING) {
      const item = menu.items.find((i) => i.name === name)
      H.expect(item, `the agent menu should list "${name}"; it lists ${JSON.stringify(menu.items.map((i) => i.name))}`)
      H.expect(item.disabled, `with ${OTHER} driving, the agent menu's "${name}" should be disabled; it is enabled`)
    }
    H.expect(
      menu.text.includes(`active on ${OTHER}`),
      `the agent menu should say why its entries are disabled, naming ${OTHER}; it reads "${menu.text.slice(0, 200)}"`,
    )
  },
}
