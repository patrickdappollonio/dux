// The phone hub: the agents list, which is the screen a phone opens on.
const { clearToasts, goto, sleep } = require("../lib.js")

module.exports = {
  file: "phone-hub.png",
  viewport: "phone",
  async shoot(page) {
    // Like the desktop sidebar shot, this one is about state, so no agent is
    // opened: doing so would clear the very indicator being shown.
    await goto(page, "")
    await sleep(1500)
    await clearToasts(page)
    return { x: 0, y: 0, width: 390, height: 529 }
  },
}
