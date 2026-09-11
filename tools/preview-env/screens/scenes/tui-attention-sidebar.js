// One agent waiting on you among two that are idle.
//
// The flag clears the moment you look at an agent's pane, and in this front end
// the selected agent's pane is always on screen: creating the agent selects it,
// and walking the selection back to a quiet row passes over it. A bell rung at
// spawn is therefore cleared by the journey that arranged it, which is how this
// picture came to show three idle agents and no flag at all. So the bell is rung
// LATE, after the selection has settled somewhere else, and the wait below is
// for the flag itself rather than for a guessed number of seconds.
module.exports = async ({ createAgent, selectAgent, setFixture, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await createAgent(0, "cache-warmup")
  // The fake provider reads its fixture from the global environment, so the next
  // agent comes up on the one that rings the bell.
  await setFixture("attention-delayed")
  await createAgent(1, "review-retry-policy")
  // Picked by name, not by counting rows: needs-you floats to the top and the
  // order within a group is the sidebar's business rather than a journey's.
  await selectAgent("retry-budget")
  await waitFor("Needs you", 40000)
  await sleep(1200)
}

// The flag itself, and the two quiet rows it is shown against.
module.exports.expectText = ["Needs you", "retry-budget", "cache-warmup"]

module.exports.file = "tui-attention-sidebar.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.crop = "sidebar"
module.exports.fixture = "steady"
