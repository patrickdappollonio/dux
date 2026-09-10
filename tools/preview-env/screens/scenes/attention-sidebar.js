// The sidebar with one agent waiting on you among five that are working.
const { boxOf, clearToasts, goto, sleep } = require("../lib.js")

module.exports = {
  file: "attention-sidebar.png",
  viewport: "desktop",
  async shoot(page) {
    // This shot is about state, so none of the agents may be selected: opening
    // one both paints a selection and clears the very indicator being shown.
    await goto(page, "")
    await sleep(1500)
    await clearToasts(page)
    const side = await boxOf(page, ["[data-slot=sidebar], aside, nav"])
    const rows = await boxOf(page, ['[aria-label="Session actions"]'])
    return {
      x: 0,
      y: 0,
      width: Math.round(side.width),
      height: Math.min(900, Math.round(rows.y + rows.height + 26)),
    }
  },
}
