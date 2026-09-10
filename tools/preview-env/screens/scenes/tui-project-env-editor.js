// The project environment editor: one KEY=value per line, typed in and then
// left unengaged so the dialog shows how to get back into the field.
module.exports = async ({ createAgent, palette, sendKeys, sendText, sleep, waitFor }) => {
  await createAgent(0, "retry-budget")
  await palette("configure-project-env")
  await waitFor("Configure Project Environment", 15000)
  sendKeys("i")
  await sleep(400)
  sendText("API_URL=http://localhost:8080")
  sendKeys("Enter")
  sendText("LOG_LEVEL=debug")
  sendKeys("Enter")
  sendText("NODE_ENV=development")
  await sleep(400)
  // Escape leaves edit mode without closing the dialog, which is the state the
  // shot is of: the text is in, the caret is gone, and the footer names the key
  // that gets back in.
  sendKeys("Escape")
  await sleep(800)
}

module.exports.file = "tui-project-env-editor.png"
module.exports.cols = 160
module.exports.rows = 30
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
