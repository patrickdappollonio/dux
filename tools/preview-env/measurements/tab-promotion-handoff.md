# What the browser does when the first tab's provider exits cleanly

Measured on 2026-09-17 in the preview container (`DUX_SRC=<worktree> ./up.sh`),
driving `measurements/tab-promotion-handoff.js`: an agent on the fake provider
with two tabs, the browser parked on the FIRST one, and that tab's provider
quit by typing its quit command into the terminal (fixture `quit-on-command`,
status 0, real typed input). The page was sampled every 50 ms for 5 s.

The question was whether the connection-lost box appears between the socket's
close and the promotion the spine reports a moment later, and how long for.

## Before the hold (`attachCover` with no handoff grace)

| t | what the page showed |
| --- | --- |
| +0 ms | `quit` typed, Enter |
| +83 ms | **"Connection lost." and Reconnect**, over the pane |
| +136 ms | the toast: `Tab (fake) of agent "tab-handoff" exited cleanly and was closed; the pane now shows its fake tab.` |
| +956 ms | box gone, tab strip gone, pane showing the surviving tab |

So the box was up for roughly **870 ms** over a pane that was about to be
replaced, for an agent that was never in trouble.

## After the hold

| t | what the page showed |
| --- | --- |
| +0 ms | `quit` typed, Enter |
| +126 ms | the toast, wording as above |
| +5000 ms | no sample ever carried "Connection lost." or a Reconnect button |

The picture the exited tab left holds for `HANDOFF_GRACE_MS`, and the pane is
replaced inside that window, so nothing about the handover reads as a fault.

Everything else, read at the end of the run:

- Pane: the surviving tab's live stream, header crumb `demo-api / tab-handoff / fake`.
- Tab strip: absent, as one tab is below the strip's threshold.
- Sidebar row: `tab-handoff / demo-api / Working` (the `2 tabs` count gone).
- Server: `slot_tab_id` is the sibling's, one tab row left, session still `active`.

`tab-promotion-before.png` and `tab-promotion-after.png` are the pane either
side of the exit; `tab-promotion-handoff.log` is the last run's raw samples.
