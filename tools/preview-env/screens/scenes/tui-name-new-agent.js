// The naming step, on an empty workspace so the dialog is the only thing on
// screen.
//
// The generated pet name is random by design, so this is the one capture whose
// text a reshoot can legitimately change.
module.exports = async ({ sendKeys, sleep, waitFor }) => {
  sendKeys("n")
  await waitFor("New agent in project")
  sendKeys("Enter")
  await waitFor("Name New Agent", 20000)
  await sleep(1000)
}

// The pet-name box is off by default, and the shot is of it on: a generated name
// is what the docs describe, and ticking it through the dialog would leave the
// focus ring on the checkbox rather than on the field.
module.exports.config = (text) =>
  text.replace(
    /^enable_randomized_pet_name_by_default = false$/m,
    "enable_randomized_pet_name_by_default = true",
  )

module.exports.file = "tui-name-new-agent.png"
module.exports.cols = 160
module.exports.rows = 30
module.exports.theme = "dux_dark"
module.exports.fixture = "steady"
