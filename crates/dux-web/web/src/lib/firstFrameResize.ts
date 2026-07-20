// Decides how the terminal sizes its PTY on the first frame after a socket
// (re)open.
//
// Background: on the first frame after each socket open the client resizes the
// PTY to the true viewport size. On the VERY FIRST open the PTY was created at
// some default size and the server's initial snapshot may not match this
// viewport, so we "jiggle" the width down one column and back: each step is a
// real winsize change, so the kernel raises SIGWINCH and the full-screen agent
// repaints its true UI over the imperfect snapshot. A same-size resize is a
// kernel no-op (no SIGWINCH), which is exactly why the plain resize would not
// force that repaint on the first open.
//
// On a RECONNECT the server keeps the PTY alive at its prior size and replays a
// fresh repaint as the first frame. If the viewport is unchanged the PTY is
// already the right size, so jiggling would force TWO needless full-screen agent
// repaints (at two different widths) on every reconnect. On mobile the socket
// reconnects constantly, so that is a lot of rewrapping the desktop never sees.
// Instead we send a SINGLE resize to the true size: it still re-asserts our
// ownership server-side, it is a kernel no-op (no repaint) when the size is
// unchanged, and it raises exactly one natural SIGWINCH (one repaint) only when
// the viewport genuinely changed while disconnected.
export type FirstFrameResizePlan = "jiggle" | "single"

export function firstFrameResizePlan(isFirstOpen: boolean): FirstFrameResizePlan {
  return isFirstOpen ? "jiggle" : "single"
}
