// The sidebar with one agent waiting on you among five that are working.
const {
  ATTENTION_AGENT,
  ATTENTION_WORD,
  SIDEBAR_ORDER,
  armAttention,
  boxOf,
  clearToasts,
  expectRows,
  expectStateWord,
  goto,
  sleep,
} = require("../lib.js")

module.exports = {
  file: "attention-sidebar.png",
  viewport: "desktop",
  async shoot(page) {
    // Re-armed here rather than trusted from the seed: the flag clears the
    // moment anything looks at that agent's pane, and this scene must not depend
    // on which scenes ran before it.
    await armAttention()
    // This shot is about state, so none of the agents may be selected: opening
    // one both paints a selection and clears the very indicator being shown.
    await goto(page, "")
    await sleep(1500)
    await clearToasts(page)
    // Every row the crop will contain, and the word the caption is entirely
    // about. This picture came back blank once, with nothing to say so.
    await expectRows(page, SIDEBAR_ORDER)
    await expectStateWord(page, ATTENTION_AGENT, ATTENTION_WORD)
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
