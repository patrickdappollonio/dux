module.exports = async ({ createAgent, waitFor }) => {
  await createAgent(1, "polished-dawn")
  await waitFor("demo-web")
  await waitFor("polished-dawn")
}

// What the captured screen has to say for the capture to be exported at all. A
// journey that ends on the wrong screen captures a perfectly valid grid of it,
// so every committed scene names the words its picture is about; a missing one
// refuses the capture and prints the grid it got instead.
module.exports.expectText = ["demo-web", "polished-dawn"]
