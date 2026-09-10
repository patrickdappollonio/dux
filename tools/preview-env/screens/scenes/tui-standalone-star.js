// A standalone agent selected: its row wears the star over the folder path, and
// the changes panel says why a plain folder has nothing to show.
module.exports = async ({ createStandaloneAgent, sleep }) => {
  await createStandaloneAgent("/root/design-notes", "design-notes")
  await sleep(1500)
}

module.exports.file = "tui-standalone-star.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
