// The New agent form waits `[server] new_agent_request_timeout_seconds` for the
// server's first answer to Create, as the setting stands after a reload on a
// page that was already open, then frees the form with the name still typed
// and a warning naming the wait; a late failed answer replaces that warning.

const DUX_HOME = "/data/dux"
const CONFIG = `${DUX_HOME}/config.toml`
// The config file as it was before this check, copied aside before it changes
// anything; the runner puts it back at the start of a run if it is still here.
const BACKUP = "/data/checks-config.toml.orig"
const KEY = "server.new_agent_request_timeout_seconds"
const WAIT_SECONDS = 3
const PROJECT = "demo-api"
const NAME = "checks-slow-create"

const isCreate = (request) => request.method() === "POST" && new URL(request.url()).pathname === "/api/v1/sessions"
const isBootstrap = (response) => new URL(response.url()).pathname === "/api/v1/bootstrap"

const publishedWait = async (H) => (await H.lib.get("/api/v1/bootstrap")).new_agent_request_timeout_seconds

async function waitPublished(H, seconds) {
  await H.until(async () => (await publishedWait(H)) === seconds, `the server to publish ${KEY} = ${seconds}`, { timeout: 10000 })
}

// `dux config set` has the running dux reload, and fails when it does not.
async function setWait(H, seconds) {
  H.lib.containerSh(`DUX_HOME=${DUX_HOME} dux config set ${KEY} ${seconds}`)
  await waitPublished(H, seconds)
}

// The config file back byte for byte, and the running dux reloaded onto it.
async function restoreConfig(H, seconds) {
  const restored = H.lib.containerSh(`if [ -e ${BACKUP} ]; then cp ${BACKUP} ${CONFIG} && rm ${BACKUP} && echo restored; fi`)
  if (!restored.trim()) return
  await H.lib.api("POST", "/api/v1/config/reload")
  await waitPublished(H, seconds)
}

// The Create button's state and the text of every toast about this agent, read
// together. Toasts about anything else (the server's own, replayed to a page
// that opens) are not this check's business.
async function formState(page) {
  return page.evaluate((name) => {
    const dialog = [...document.querySelectorAll('[role="dialog"]')].at(-1)
    const create = dialog
      ? [...dialog.querySelectorAll('[data-slot="modal-footer"] button')].find((b) => /Create agent/.test(window.__checkNameOf(b)))
      : null
    const field = dialog ? dialog.querySelector("input:not([type=checkbox]):not([type=radio])") : null
    return {
      dialog: dialog ? window.__checkNameOf(dialog) : null,
      busy: create ? create.getAttribute("aria-busy") === "true" : null,
      value: field ? field.value : null,
      toasts: [...document.querySelectorAll("[data-sonner-toast]")]
        .filter((t) => t.getBoundingClientRect().height > 0 && t.getAttribute("data-removed") !== "true")
        .map((t) => t.textContent.replace(/\s+/g, " ").trim())
        .filter((text) => text.includes(name)),
    }
  }, NAME)
}

module.exports = {
  protects: `New agent: after a config reload sets ${KEY} to ${WAIT_SECONDS} on an open page, a held create frees the form after ${WAIT_SECONDS} seconds with the name kept and a warning naming ${WAIT_SECONDS} seconds, and a late 502 replaces that warning with one saying it is not known whether the agent was created.`,
  async run(ctx, H) {
    const before = await publishedWait(H)
    ctx.after(() => restoreConfig(H, before))
    H.lib.containerSh(`test -e ${BACKUP} || cp ${CONFIG} ${BACKUP}`)

    const dev = await H.device(ctx, "computer")
    const creates = []
    await dev.page.setRequestInterception(true)
    dev.page.on("request", (request) => {
      if (isCreate(request)) {
        creates.push({ request, at: Date.now() })
        return
      }
      request.continue()
    })
    const published = []
    dev.page.on("response", (response) => {
      if (!isBootstrap(response)) return
      response
        .json()
        .then((doc) => published.push(doc.new_agent_request_timeout_seconds))
        .catch(() => {})
    })

    await H.goto(dev, "")
    await setWait(H, WAIT_SECONDS)
    await H.until(() => published.includes(WAIT_SECONDS), `the open page to fetch the reloaded settings carrying ${WAIT_SECONDS} seconds`, {
      timeout: 8000,
    })

    await H.press(dev, { name: "^New agent…$", selector: "button" })
    await H.waitForDialog(dev, "^New agent$")
    await H.press(dev, { name: `^${PROJECT}$`, selector: "button", within: H.DIALOG })
    const form = await H.waitForDialog(dev, `^New agent in ${PROJECT}$`)
    await dev.page.keyboard.type(NAME)
    await dev.page.keyboard.press("Enter")
    await H.until(() => creates.length > 0, "the form to send its create", { timeout: 3000 })
    const sent = creates[0].at
    const first = await formState(dev.page)
    H.expect(first.busy === true, `right after Create the button should be busy; it is ${first.busy === null ? "not found" : "not busy"}`)

    const freed = await H.until(
      async () => {
        const now = await formState(dev.page)
        return now.busy === false ? { ...now, after: Date.now() - sent } : null
      },
      `the form to stop waiting after ${WAIT_SECONDS} seconds`,
      { timeout: WAIT_SECONDS * 1000 + 4000 },
    )
    H.expect(
      freed.after >= WAIT_SECONDS * 1000 - 300,
      `the form should wait ${WAIT_SECONDS} seconds before it stops being busy; it stopped after ${(freed.after / 1000).toFixed(1)}s`,
    )
    H.expect(freed.dialog === form.name, `the form should stay open once it stops waiting; ${freed.dialog ? `"${freed.dialog}" is on top` : "no dialog is open"}`)
    H.expect(freed.value === NAME, `the form should keep the typed name "${NAME}"; the field holds "${freed.value}"`)
    const warning = await H.until(
      async () => (await formState(dev.page)).toasts[0],
      "the warning that the server has not answered",
      { timeout: 2000 },
    )
    H.expect(
      new RegExp(`after ${WAIT_SECONDS} seconds`).test(warning),
      `the warning should say the server has not answered after ${WAIT_SECONDS} seconds; it says "${warning}"`,
    )

    await creates[0].request.respond({ status: 502, contentType: "text/plain", body: "" })
    const late = await H.until(
      async () => {
        const { toasts } = await formState(dev.page)
        return toasts.some((t) => /It is not known whether/.test(t)) ? toasts : null
      },
      "the late 502 to say it is not known whether the agent was created",
      { timeout: 4000 },
    )
    await H.frames(dev.page)
    const after = (await formState(dev.page)).toasts
    H.expect(
      after.length === 1 && /It is not known whether the agent/.test(after[0]) && after[0].includes(NAME),
      `the late 502 should replace the warning with one toast saying it is not known whether the agent "${NAME}" was created; on screen: ${JSON.stringify(after.length ? after : late)}`,
    )
    H.expect(creates.length === 1, `one Enter should send one create; ${creates.length} were sent`)
  },
}
