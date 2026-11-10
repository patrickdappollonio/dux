// An agent's terminals on a phone: one part at a time, so the dock strip with
// its two pills replaces the agent's own, and the action flap carries the
// switch back to the agent, its icon saying what the agent is doing.
const {
  agents,
  clearToasts,
  dockTerminals,
  expectNoCover,
  expectPanePainted,
  expectVisible,
  get,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "phone-terminal-view.png",
  viewport: "phone",
  async shoot(page, { after }) {
    const by = await agents()
    const id = by["fix-login-redirect"].id
    after(await dockTerminals(id, 2))
    const own = ((await get("/api/v1/workspace")).terminals || [])
      .filter((t) => t.owner.kind === "session" && t.owner.session_id === id)
      .sort((a, b) => a.sort_order - b.sort_order)
    await goto(page, `#/agent/${id}/terminal/${own[0].id}`)
    await takeOver(page)
    await sleep(2500)
    await clearToasts(page)
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectVisible(page, '[data-testid="dock-strip"]', "the dock strip")
    await expectVisible(page, '[aria-label^="Show the agent"]', "the switch back to the agent")
    return { x: 0, y: 0, width: 390, height: 844 }
  },
}
