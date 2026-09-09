// Decides how the terminal sizes its PTY on the first frame after a socket opens.
//
// On the first open the PTY was created at a default size the snapshot may not
// match, so the width is jiggled down one column and back: a same-size resize is a
// kernel no-op, and only a real winsize change raises the SIGWINCH that makes a
// full-screen agent repaint over an imperfect snapshot.
//
// A reconnect sends a single resize to the true size instead: the server keeps the
// PTY at its prior size and replays a repaint anyway, so jiggling would cost two
// needless full-screen repaints on every one of a phone's constant reconnects.
export type FirstFrameResizePlan = "jiggle" | "single"

export function firstFrameResizePlan(isFirstOpen: boolean): FirstFrameResizePlan {
  return isFirstOpen ? "jiggle" : "single"
}
