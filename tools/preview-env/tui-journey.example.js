module.exports = async ({ createAgent, waitFor }) => {
  await createAgent(1, "polished-dawn")
  await waitFor("demo-web")
  await waitFor("polished-dawn")
}
