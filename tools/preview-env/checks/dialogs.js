// The representative dialog of each kind, and how a person opens it. Several
// checks open the same ones, so the way there is written once.
//
// `opener` is the name of the control focus goes back to when the dialog closes:
// a menu item is gone by then, so it is the menu's trigger.
const H = require("./harness.js")

// The agent the dialog checks open things on: a quiet seeded agent no scene is
// about, with nothing staged on it.
const AGENT = "polish-onboarding"

const DIALOGS = {
  confirm: {
    title: "^Detach agent\\?$",
    from: "agent",
    open: (dev) => H.paneMenu(dev, "^Detach agent…$"),
    opener: "Session actions",
    safe: "Cancel",
  },
  form: {
    title: "^Rename agent$",
    from: "agent",
    open: (dev) => H.paneMenu(dev, "^Rename agent…$"),
    opener: "Session actions",
  },
  picker: {
    title: "^New agent$",
    from: "hub",
    open: (dev) => H.press(dev, { name: "^New agent…$", selector: "button" }),
    opener: "New agent…",
  },
  report: {
    title: `^${AGENT}$`,
    from: "agent",
    open: (dev) => H.paneMenu(dev, "^Agent info…$"),
    opener: "Session actions",
    safe: "Close",
  },
}

// Where a dialog's opener lives: the agent's own pane, or the page with the
// launcher on it (the sidebar on a computer, the hub on a phone).
async function arrive(dev, from) {
  if (from === "hub") return H.goto(dev, "")
  await H.goto(dev, H.agentHash(await H.agentId(AGENT)))
  await H.drive(dev)
}

module.exports = { AGENT, DIALOGS, arrive }
