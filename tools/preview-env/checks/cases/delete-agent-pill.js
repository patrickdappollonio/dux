// The Delete agent dialog says up front, in one pill, whether deleting the
// branch would lose commits, and it does so without the dialog changing size as
// the answer arrives or afterwards.
const { AGENT } = require("../dialogs.js")

const WORKTREE = `/data/dux/worktrees/demo-api/${AGENT}`
const SUBJECT = "checks: a commit on no other branch"

// The pill, read the way a person reads it: its sentence. Its icon (a check or
// an alert) carries no accessible name or role of its own, so nothing but its
// drawing tells the two apart; the sentence is what the check holds it to.
async function pill(page) {
  return page.evaluate(() => {
    const dialogs = [...document.querySelectorAll('[role="dialog"],[role="alertdialog"]')].filter(
      (d) => !d.hasAttribute("data-closed"),
    )
    const dialog = dialogs.at(-1)
    if (!dialog) return { open: false }
    const status = dialog.querySelector('[role="status"]')
    const shown = status && status.getBoundingClientRect().height > 0
    return {
      open: true,
      // Its laid-out height: the card zooms in on open, and a scaled box reads a
      // pixel short for the length of that animation.
      height: dialog.offsetHeight,
      shown: Boolean(shown),
      text: shown ? status.textContent.replace(/\s+/g, " ").trim() : "",
    }
  })
}

const pending = (p) => /^Looking for unpushed work/.test(p.text)

async function check(H, dev, { losing }) {
  await H.paneMenu(dev, "^Delete agent…$")
  await H.waitForDialog(dev, "^Delete agent\\?$")
  const atOpen = await pill(dev.page)
  H.expect(atOpen.shown, `the Delete agent dialog should show its branch-check pill from the moment it opens; it has none`)

  const answered = await H.until(async () => {
    const now = await pill(dev.page)
    return now.shown && !pending(now) ? now : null
  }, "the branch check to answer", { timeout: 8000 })
  await H.sleep(4000)
  const later = await pill(dev.page)

  const what = losing ? "an agent with a commit on no other branch" : "an agent whose commits are all on another branch"
  H.expect(later.shown, `for ${what} the pill should still be there 4 seconds after the answer; it is gone`)
  if (losing) {
    H.expect(
      /commits? on .* (is|are) on no other branch|isn't pushed|aren't pushed/.test(answered.text),
      `for ${what} the pill should say its commit would be lost; it reads "${answered.text}"`,
    )
  } else {
    H.expect(
      /also on another branch|No unpushed work/.test(answered.text),
      `for ${what} the pill should say nothing would be lost; it reads "${answered.text}"`,
    )
  }
  const heights = [atOpen.height, answered.height, later.height].map((h) => Math.round(h * 10) / 10)
  H.expect(
    heights.every((h) => Math.abs(h - heights[0]) < 0.5),
    `for ${what} the dialog's height should not change; it was ${heights[0]}px at open, ${heights[1]}px at the answer ("${answered.text}") and ${heights[2]}px 4 seconds later`,
  )

  await H.press(dev, { name: "^Cancel$", selector: "button", within: H.DIALOG })
  await H.waitForNoDialog(dev, "the Delete agent dialog on Cancel")
}

module.exports = {
  protects:
    "Delete agent shows a pill saying nothing would be lost for an agent whose commits are all on another branch, and one saying its commit would be lost for an agent with a commit on no other branch, from open through 4 seconds later, without the dialog's height changing.",
  async run(ctx, H) {
    const head = H.lib.containerSh(`cd ${WORKTREE} && git rev-list --count HEAD --not --exclude=${AGENT} --branches`).trim()
    H.expect(
      head === "0",
      `${AGENT} should start with no commit of its own; it has ${head}, which the runner's start-of-run check should have put back (run.sh --reseed puts it back)`,
    )

    const dev = await H.device(ctx, "computer")
    await H.goto(dev, H.agentHash(await H.agentId(AGENT)))
    await H.drive(dev)
    await check(H, dev, { losing: false })

    // The undo is registered before the commit is made, so an interrupt between
    // the two still runs it; it does nothing when there is no such commit.
    ctx.after(() =>
      H.lib.containerSh(
        `cd ${WORKTREE} && if [ "$(git log -1 --format=%s)" = '${SUBJECT}' ]; then git reset -q --soft HEAD~1; fi`,
      ),
    )
    H.lib.containerSh(
      `cd ${WORKTREE} && git -c user.name=checks -c user.email=checks@example.invalid commit -q --allow-empty -m '${SUBJECT}'`,
    )
    await check(H, dev, { losing: true })
  },
}
