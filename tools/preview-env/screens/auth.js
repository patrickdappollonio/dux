// The web password, staged for the sign-in screenshots.
//
// Every change goes through `dux config set` inside the preview container, the
// way the docs tell a person to make it, and a running dux reloads on it by
// itself. The preview reaches dux through Docker's published port, so to dux a
// browser on the host is "everyone else": with a password set it gets the
// sign-in page, which is exactly the visitor these pictures are about.
//
// A scene that sets anything here registers `resetAuth` to run after its shot,
// so the workspace every other scene is shot against stays password-free.
const { BASE, DESKTOP, boxOf, containerSh, refusal, sleep, steadyBox } = require("./lib.js")

const DUX_HOME = "/data/dux"

// The passwords the scenes sign in with. Made up for the screenshots, and never
// on screen: every field they are typed into hides them.
const STRONG_PASSWORD = "lantern-quarry-velvet-otter-89"
// Below the default minimums on both counts, so a sign-in with it raises the
// weak-password banner.
const WEAK_PASSWORD = "sunshine1"

// Single quotes around every argument, for the container's own shell.
const quote = (value) => `'${String(value).replace(/'/g, "'\\''")}'`

function configSet(key, value) {
  containerSh(`DUX_HOME=${DUX_HOME} dux config set ${quote(key)} ${quote(value)}`)
}

function setPassword(password) {
  containerSh(
    `printf '%s\\n' ${quote(password)} | DUX_HOME=${DUX_HOME} dux config set server.auth.password --stdin`,
  )
}

async function status() {
  const res = await fetch(`${BASE}/api/v1/auth/status`, { cache: "no-store" })
  return { code: res.status, body: await res.json().catch(() => ({})) }
}

// The reload `dux config set` asks for is a signal, so it lands a moment later;
// wait until the server's own answer says the change is in force.
async function waitForStatus(what, holds, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs
  let last = null
  while (Date.now() < deadline) {
    last = await status().catch(() => null)
    if (last && holds(last)) return last
    await sleep(300)
  }
  throw refusal(`dux never reported ${what}; its last answer was ${JSON.stringify(last)}`)
}

// Back to no password and every [server.auth] setting the scenes touch at its
// default.
async function resetAuth() {
  configSet("server.auth.blocked_addresses", "[]")
  configSet("server.auth.cookie_secure", "auto")
  configSet("server.auth.minimum_password_length", "12")
  configSet("server.auth.minimum_password_score", "2")
  configSet("server.auth.password_hash", "")
  await waitForStatus(
    "the password gone",
    (s) => s.code === 200 && s.body.password_set === false && s.body.transport_encrypted === false,
  )
}

// A strong password, in force.
async function requirePassword({ cookieSecure = "auto" } = {}) {
  configSet("server.auth.cookie_secure", cookieSecure)
  setPassword(STRONG_PASSWORD)
  await waitForStatus(
    "a password in force",
    (s) =>
      s.code === 200 &&
      s.body.password_set === true &&
      s.body.required_here === true &&
      s.body.transport_encrypted === (cookieSecure === "always"),
  )
}

// A password below today's minimums, set the only way dux lets one in: with the
// minimums lowered while it is stored, then put back.
async function requireWeakPassword() {
  configSet("server.auth.minimum_password_length", "8")
  configSet("server.auth.minimum_password_score", "0")
  setPassword(WEAK_PASSWORD)
  configSet("server.auth.minimum_password_length", "12")
  configSet("server.auth.minimum_password_score", "2")
  await waitForStatus(
    "the weak password in force under the default minimums",
    (s) =>
      s.code === 200 &&
      s.body.password_set === true &&
      s.body.minimum_password_length === 12 &&
      s.body.minimum_password_score === 2,
  )
}

// Sign in through the page's own form and wait for the app behind it.
async function signIn(page, password = STRONG_PASSWORD) {
  await page.waitForSelector('[data-testid="login-form"] input[name="password"]', {
    timeout: 20000,
  })
  await page.type('[data-testid="login-form"] input[name="password"]', password)
  await page.keyboard.press("Enter")
  await page.waitForFunction(
    () => {
      const shell = document.querySelector('[data-testid="app-shell"]')
      return shell && !shell.hasAttribute("inert") && !document.querySelector('[data-testid="login-form"]')
    },
    { timeout: 30000 },
  )
  await sleep(1500)
}

// The frame for a page the sign-in gate shows instead of the app: its one
// centered column, with room around it. The rest of the window is the app's
// background and nothing else.
async function gateClip(page, pad = 56) {
  return steadyBox(() => boxOf(page, ["main > .max-w-sm"], pad), {
    what: "the gate page's column",
  })
}

// The frame for a banner across the top of the app: the full width, from the
// top down past the header under it, so the banner reads as part of the page.
async function bannerClip(page, testid, below = 140) {
  const box = await steadyBox(() => boxOf(page, [`[data-testid="${testid}"]`]), {
    what: `the ${testid}`,
  })
  return { x: 0, y: 0, width: DESKTOP.width, height: Math.round(box.y + box.height + below) }
}

module.exports = {
  STRONG_PASSWORD,
  bannerClip,
  gateClip,
  WEAK_PASSWORD,
  requirePassword,
  requireWeakPassword,
  resetAuth,
  signIn,
  status,
}
