// `dux server` listening on every interface with no password set: the startup
// banner and, in red, the warning that anyone who can reach it controls your
// agents and terminals.
module.exports = async ({ sleep, waitFor }) => {
  await waitFor("No password is set", 30000)
  await sleep(800)
}

module.exports.launch = "server"

// Every interface, which is what makes dux reachable beyond this machine, on the
// default port the docs name. The canonical config may or may not spell `host`
// out, so it is replaced where it is and added where it is not.
module.exports.config = (text) => {
  let out = text.replace(/^port = \d+$/m, "port = 3890")
  out = /^host = .*$/m.test(out)
    ? out.replace(/^host = .*$/m, 'host = "0.0.0.0"')
    : out.replace(/^\[server\]$/m, '[server]\nhost = "0.0.0.0"')
  return out
}

// The warning, and the command it tells you to run.
module.exports.expectText = [
  "No password is set and dux is reachable beyond this machine",
  "dux config set",
]

module.exports.file = "server-no-password-warning.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
module.exports.crop = "content"
