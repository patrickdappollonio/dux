// `dux config set server.auth.password` typed into a plain shell: the hidden
// prompt with its strength meter, the second prompt, and the two lines that say
// where the hash went and when it applies. Nothing is echoed, so the password
// itself is never on screen; it is a made-up passphrase all the same.
//
// No dux is running in the capture, which is why the last line says the change
// applies the next time it starts.
const PASSPHRASE = "lantern-quarry-velvet-otter-89"

module.exports = async ({ sendKeys, sendText, sleep, waitFor }) => {
  await waitFor("$")
  sendText("dux config set server.auth.password")
  sendKeys("Enter")
  await waitFor("New web UI password", 15000)
  // One key at a time, the way a person types: the meter redraws on every key.
  for (const ch of PASSPHRASE) {
    sendText(ch)
    await sleep(15)
  }
  await sleep(400)
  sendKeys("Enter")
  await waitFor("Type it again", 5000)
  sendText(PASSPHRASE)
  sendKeys("Enter")
  await waitFor("applies the next time it starts", 30000)
  await sleep(300)
}

module.exports.launch = "shell"

// The command, the meter as it stood when Enter was pressed, the second prompt,
// where the hash landed, and when it applies.
module.exports.expectText = [
  "$ dux config set server.auth.password",
  "New web UI password [",
  "Type it again:",
  "The web UI password is stored",
  "Argon2id hash is in server.auth.password_hash in /home/you/.config/dux/config.toml",
  "dux is not running",
]

module.exports.file = "config-set-password.png"
module.exports.cols = 200
module.exports.rows = 24
module.exports.theme = "dux_dark"
module.exports.crop = "content"
