// The grid bounds a PTY accepts, mirroring `dux_core::pty`'s PTY_MIN_ROWS,
// PTY_MIN_COLS, PTY_MAX_ROWS and PTY_MAX_COLS (a test pins them to the Rust
// source). The server refuses a resize frame outside them silently, so a frame
// out of bounds must never leave this client: a take-over it carried would be
// spent on a claim that was never made.
export const PTY_MIN_ROWS = 1
export const PTY_MIN_COLS = 2
export const PTY_MAX_ROWS = 1024
export const PTY_MAX_COLS = 2048

// Whether `rows` by `cols` is a grid the server will apply.
export function ptyGridInBounds(rows: number, cols: number): boolean {
  return (
    Number.isInteger(rows) &&
    Number.isInteger(cols) &&
    rows >= PTY_MIN_ROWS &&
    rows <= PTY_MAX_ROWS &&
    cols >= PTY_MIN_COLS &&
    cols <= PTY_MAX_COLS
  )
}
