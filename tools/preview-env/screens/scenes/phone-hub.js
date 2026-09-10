// The phone hub: the agents list, which is the screen a phone opens on.
const { armAttention, assertNeedsAttention, clearToasts, goto, sleep } = require("../lib.js")

module.exports = {
  file: "phone-hub.png",
  viewport: "phone",
  async shoot(page) {
    // Re-armed here rather than trusted from the seed: the flag clears the
    // moment anything looks at that agent's pane, and this scene must not depend
    // on which scenes ran before it.
    await armAttention()
    // Like the desktop sidebar shot, this one is about state, so no agent is
    // opened: doing so would clear the very indicator being shown.
    await goto(page, "")
    await sleep(1500)
    await clearToasts(page)
    await assertNeedsAttention(page)
    // Cut at the last agent row rather than at the viewport: below it is the
    // hub's bottom bar, which the hub's own caption is not about.
    return { x: 0, y: 0, width: 390, height: 529 }
  },
}
