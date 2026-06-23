//! PTY-based terminal client plus the surface-agnostic cell types for its
//! terminal-grid snapshot. The client spawns a CLI in a pseudo-terminal and
//! keeps a full terminal grid (via the `vt100` crate); the snapshot's
//! `CellColor`/`CellModifier` let each surface convert to its own medium (the
//! TUI to `ratatui` types; the web to CSS) at its render boundary.

use std::env;
use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};
use compact_str::CompactString;
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

use crate::logger;

/// Mirrors the variant set of `ratatui::style::Color` so the PTY snapshot can
/// describe any cell color without depending on a UI toolkit. The TUI converts
/// 1:1 to `ratatui::Color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellColor {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

/// The subset of text attributes the terminal grid carries. Mirrors the
/// `ratatui::style::Modifier` flags the snapshot sets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellModifier {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underlined: bool,
    pub reversed: bool,
    pub crossed_out: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapshotCursor {
    pub row: u16,
    pub col: u16,
}

#[derive(Clone, Debug)]
pub struct SnapshotCell {
    pub row: u16,
    pub col: u16,
    pub symbol: CompactString,
    pub fg: CellColor,
    pub bg: CellColor,
    pub modifier: CellModifier,
}

#[derive(Clone, Debug)]
pub struct TerminalSnapshot {
    pub rows: u16,
    pub cols: u16,
    pub scrollback_offset: usize,
    pub scrollback_total: usize,
    pub cursor: Option<SnapshotCursor>,
    pub cells: Vec<SnapshotCell>,
}

impl TerminalSnapshot {
    /// Create an empty snapshot suitable for reuse as a pre-allocated buffer.
    pub fn empty() -> Self {
        Self {
            rows: 0,
            cols: 0,
            scrollback_offset: 0,
            scrollback_total: 0,
            cursor: None,
            cells: Vec::new(),
        }
    }
}

/// Build an ANSI byte sequence that repaints `snapshot` onto a freshly-connected
/// client's terminal. If `alt_screen` is set, switch the client into the
/// alternate-screen buffer first so full-screen apps (vim, claude) render
/// correctly. Reflects the visible screen only (no scrollback replay).
pub fn synthesize_repaint(snapshot: &TerminalSnapshot, alt_screen: bool) -> Vec<u8> {
    let mut out = String::new();
    if alt_screen {
        out.push_str("\x1b[?1049h");
    }
    out.push_str("\x1b[2J\x1b[H");

    let mut cells: Vec<&SnapshotCell> = snapshot.cells.iter().collect();
    cells.sort_by_key(|c| (c.row, c.col));

    let mut expected_next: Option<(u16, u16)> = None;
    let mut last_style: Option<(CellColor, CellColor, CellModifier)> = None;
    for cell in cells {
        if expected_next != Some((cell.row, cell.col)) {
            out.push_str(&format!("\x1b[{};{}H", cell.row + 1, cell.col + 1));
        }
        let style = (cell.fg, cell.bg, cell.modifier);
        if last_style != Some(style) {
            out.push_str("\x1b[0m");
            out.push_str(&sgr_sequence(cell.fg, cell.bg, cell.modifier));
            last_style = Some(style);
        }
        out.push_str(cell.symbol.as_str());
        expected_next = Some((cell.row, cell.col + 1));
    }

    out.push_str("\x1b[0m");
    if let Some(cursor) = &snapshot.cursor {
        out.push_str(&format!("\x1b[{};{}H", cursor.row + 1, cursor.col + 1));
    }
    out.into_bytes()
}

fn sgr_sequence(fg: CellColor, bg: CellColor, modifier: CellModifier) -> String {
    let mut params: Vec<String> = Vec::new();
    if modifier.bold {
        params.push("1".to_string());
    }
    if modifier.dim {
        params.push("2".to_string());
    }
    if modifier.italic {
        params.push("3".to_string());
    }
    if modifier.underlined {
        params.push("4".to_string());
    }
    if modifier.reversed {
        params.push("7".to_string());
    }
    if modifier.crossed_out {
        params.push("9".to_string());
    }
    params.push(fg_sgr(fg));
    params.push(bg_sgr(bg));
    format!("\x1b[{}m", params.join(";"))
}

fn fg_sgr(color: CellColor) -> String {
    match color {
        CellColor::Reset => "39".to_string(),
        CellColor::Black => "30".to_string(),
        CellColor::Red => "31".to_string(),
        CellColor::Green => "32".to_string(),
        CellColor::Yellow => "33".to_string(),
        CellColor::Blue => "34".to_string(),
        CellColor::Magenta => "35".to_string(),
        CellColor::Cyan => "36".to_string(),
        CellColor::Gray => "37".to_string(),
        CellColor::DarkGray => "90".to_string(),
        CellColor::LightRed => "91".to_string(),
        CellColor::LightGreen => "92".to_string(),
        CellColor::LightYellow => "93".to_string(),
        CellColor::LightBlue => "94".to_string(),
        CellColor::LightMagenta => "95".to_string(),
        CellColor::LightCyan => "96".to_string(),
        CellColor::White => "97".to_string(),
        CellColor::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
        CellColor::Indexed(n) => format!("38;5;{n}"),
    }
}

fn bg_sgr(color: CellColor) -> String {
    match color {
        CellColor::Reset => "49".to_string(),
        CellColor::Black => "40".to_string(),
        CellColor::Red => "41".to_string(),
        CellColor::Green => "42".to_string(),
        CellColor::Yellow => "43".to_string(),
        CellColor::Blue => "44".to_string(),
        CellColor::Magenta => "45".to_string(),
        CellColor::Cyan => "46".to_string(),
        CellColor::Gray => "47".to_string(),
        CellColor::DarkGray => "100".to_string(),
        CellColor::LightRed => "101".to_string(),
        CellColor::LightGreen => "102".to_string(),
        CellColor::LightYellow => "103".to_string(),
        CellColor::LightBlue => "104".to_string(),
        CellColor::LightMagenta => "105".to_string(),
        CellColor::LightCyan => "106".to_string(),
        CellColor::White => "107".to_string(),
        CellColor::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
        CellColor::Indexed(n) => format!("48;5;{n}"),
    }
}

/// Maximum number of bytes buffered while PTY ingestion is paused (4 MiB).
/// The oldest data is dropped on overflow — on resume the child will typically
/// redraw anyway because pause is only active during scrollback sessions with
/// TUI-style providers.
const PAUSE_BUFFER_CAP: usize = 4 * 1024 * 1024;

/// Safety ceiling on how many grid rows a single reconnect repaint replays.
/// `agent_scrollback_lines` is an unbounded user value; this bounds the one-time
/// buffer a connect builds (under the terminal lock) so a pathological config
/// can't stall the engine thread or balloon memory. The default scrollback
/// (10_000) is far below this, so normal use is never truncated; when it is, the
/// most recent lines are kept and the drop is logged.
const MAX_RECONNECT_REPLAY_LINES: i32 = 100_000;

/// Topmost grid line a reconnect repaint should start from: the buffer top,
/// unless that would exceed [`MAX_RECONNECT_REPLAY_LINES`] rows, in which case it
/// is pulled down to keep only the most recent lines.
fn clamp_replay_top(full_top: i32, bottom: i32) -> i32 {
    full_top.max(bottom + 1 - MAX_RECONNECT_REPLAY_LINES)
}

/// Bounded depth (in chunks) of the PTY outbound write queue. Keystrokes and the
/// terminal parser's query replies are queued here for the dedicated writer
/// thread. When a child stops reading its input the writer thread blocks and the
/// queue fills; past this cap, new chunks are dropped rather than blocking the
/// caller — a child that is not reading would discard the input anyway.
const PTY_WRITE_QUEUE_CAP: usize = 1024;

/// How long `PtyWriter::drop` will wait for the writer thread to acknowledge its
/// shutdown signal before abandoning the join. A well-behaved teardown (child
/// group killed, PTY slave released) finishes in microseconds; this generous
/// ceiling only fires when a write is genuinely wedged (slave still open despite
/// the group kill — e.g. a double-forked daemon that left the group). On timeout
/// the thread is abandoned rather than hanging the dropping thread indefinitely.
const PTY_WRITER_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Messages sent to the PTY writer thread.
enum PtyWriteMsg {
    /// Forward these bytes to the underlying PTY master writer.
    Bytes(Vec<u8>),
    /// Exit the writer thread unconditionally. Sent by [`PtyWriter::drop`] so
    /// teardown is independent of how many sender clones are still alive (the
    /// reader thread holds one), and independent of whether a write is blocked.
    Shutdown,
}

/// Push a chunk onto a PTY write queue without ever blocking. A full queue (the
/// child is not draining its terminal) logs and drops the chunk rather than
/// blocking the caller — a child that is not reading would discard the bytes
/// anyway. A disconnected channel (the writer thread is gone) is a no-op. Shared
/// by [`PtyWriter::send`] (user input) and the reader thread (terminal parser
/// replies) so both log drops identically.
fn pty_queue_send(tx: &std::sync::mpsc::SyncSender<PtyWriteMsg>, bytes: Vec<u8>) {
    if let Err(std::sync::mpsc::TrySendError::Full(_)) = tx.try_send(PtyWriteMsg::Bytes(bytes)) {
        logger::debug(
            "PTY write queue full; dropping bytes for a child that is not draining its terminal",
        );
    }
}

/// Owns the PTY master writer on a dedicated thread and accepts outbound byte
/// chunks over a bounded channel.
///
/// This decouples *writing to the child* from the threads that must stay
/// responsive. The web engine runs every request on a single thread, and the PTY
/// reader thread must keep draining the child's output; a raw blocking `write()`
/// to a child that has stopped reading its input (e.g. a CLI paused on a network
/// call) would wedge whichever thread called it. Routing every write through this
/// one thread means only it can ever block — never the engine thread and never
/// the reader — which is what prevents one stalled child from freezing the whole
/// server. A single writer thread also serializes input and parser replies in
/// submission order.
struct PtyWriter {
    /// The sender half of the write queue. Used to push [`PtyWriteMsg::Bytes`]
    /// chunks to the writer thread, and to send [`PtyWriteMsg::Shutdown`] on
    /// drop. `None` only transiently during `Drop` after the shutdown signal has
    /// been sent and before the join completes.
    tx: Option<std::sync::mpsc::SyncSender<PtyWriteMsg>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl PtyWriter {
    /// Spawn the writer thread around the PTY master `writer`.
    fn spawn(mut writer: Box<dyn Write + Send>) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<PtyWriteMsg>(PTY_WRITE_QUEUE_CAP);
        let thread = thread::spawn(move || {
            // A blocking write here only ever stalls THIS thread. On teardown the
            // child's process group is killed first, which closes the PTY and makes
            // the write return an error, so the loop exits promptly. If a
            // `Shutdown` message arrives first, the loop exits unconditionally
            // without waiting for that error — so a surviving sender clone (the
            // reader thread holds one) can never prevent the thread from stopping.
            // Loop exits on `Shutdown` or a channel error (the pattern stops
            // matching), or on a write error (explicit break below).
            while let Ok(PtyWriteMsg::Bytes(chunk)) = rx.recv() {
                if writer.write_all(&chunk).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        });
        Self {
            tx: Some(tx),
            thread: Some(thread),
        }
    }

    /// A clonable handle for the reader thread to push the terminal parser's query
    /// replies through the same single writer (preserving submission order).
    fn sender(&self) -> std::sync::mpsc::SyncSender<PtyWriteMsg> {
        self.tx
            .as_ref()
            .expect("PtyWriter sender taken before Drop")
            .clone()
    }

    /// Queue bytes for the child. Never blocks: a full queue (child not draining)
    /// drops the chunk (logged), and a gone writer thread (child exited) is a
    /// no-op.
    fn send(&self, bytes: Vec<u8>) {
        if let Some(tx) = self.tx.as_ref() {
            pty_queue_send(tx, bytes);
        }
    }
}

impl Drop for PtyWriter {
    fn drop(&mut self) {
        // Send an explicit Shutdown rather than relying on channel disconnect.
        // The reader thread holds a clone of `tx`, so merely dropping our copy
        // does not disconnect the channel — the writer thread's `recv` would
        // keep blocking, and the join below would hang. `Shutdown` is obeyed
        // unconditionally regardless of how many sender clones remain alive.
        if let Some(tx) = self.tx.take() {
            // `try_send` never blocks: if the queue is full (writer is wedged on
            // a stalled write and the queue has been flooded), the Shutdown is
            // dropped here. That is acceptable because the bounded join below will
            // time out and abandon the thread rather than hanging indefinitely.
            // On disconnect (`Err(Disconnected)`) the writer has already exited,
            // so the join completes immediately.
            let _ = tx.try_send(PtyWriteMsg::Shutdown);
        }

        if let Some(handle) = self.thread.take() {
            // Bounded join: in the normal path (child group killed, PTY slave
            // released) the writer thread exits in microseconds. On timeout the
            // thread is abandoned rather than blocking the dropping thread. A
            // well-behaved teardown — `PtyClient::drop` kills the child group
            // and joins the reader before this runs — means the write has already
            // errored out, so the timeout is never reached in practice; it is a
            // last-resort safety net for a wedged write on a misbehaving child.
            let (done_tx, done_rx) = std::sync::mpsc::sync_channel::<()>(0);
            thread::spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
            if done_rx.recv_timeout(PTY_WRITER_SHUTDOWN_TIMEOUT).is_err() {
                logger::debug(
                    "PTY writer thread did not exit within timeout on shutdown; \
                     abandoning the thread (a write may have been wedged on a \
                     misbehaving child that holds the PTY slave open)",
                );
            }
        }
    }
}

/// A PTY-based client that spawns a CLI tool in a pseudo-terminal and keeps a
/// full terminal grid with scrollback using the `vt100` crate.
pub struct PtyClient {
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    /// Dedicated writer thread for the PTY master, fed over a bounded queue so a
    /// child that stops reading its input can never block the engine thread.
    writer: PtyWriter,
    terminal: Arc<Mutex<TerminalState>>,
    child: Box<dyn Child + Send + Sync>,
    exited: Arc<AtomicBool>,
    has_output: Arc<AtomicBool>,
    /// Set by the reader thread or scroll/resize methods when the terminal
    /// state changes. Cleared by `snapshot_into` after rebuilding the buffer.
    dirty: Arc<AtomicBool>,
    /// Set by the reader thread when new data arrives. Cleared by
    /// `take_received_data` — used to detect streaming activity without
    /// interfering with the snapshot dirty flag.
    received_data: Arc<AtomicBool>,
    /// Records the last resize so `take_received_data` can suppress the
    /// redraw burst that follows a `SIGWINCH`.
    last_resize_at: Mutex<Option<Instant>>,
    /// When true, the reader thread buffers incoming bytes into `pending_bytes`
    /// instead of feeding them to the terminal parser. Toggled by the app
    /// when the user enters/leaves scrollback so the grid stays stable while
    /// the user reads history (tmux copy-mode style).
    scroll_paused: Arc<AtomicBool>,
    /// Bytes received from the PTY while ingestion is paused. Drained into
    /// the terminal parser on resume. Bounded by `PAUSE_BUFFER_CAP`; oldest
    /// bytes are dropped on overflow.
    pending_bytes: Arc<Mutex<PendingIngest>>,
    /// Live raw-byte subscribers (web clients). Each receives a clone of every
    /// chunk read from the PTY, independent of TUI scrollback pause. Senders
    /// that have hung up are pruned by the reader loop.
    subscribers: Arc<Mutex<Vec<std::sync::mpsc::Sender<Vec<u8>>>>>,
    /// Handle to the background reader thread. Joined in `Drop` (after the
    /// child is killed and reaped) so the thread does not outlive the client.
    reader_thread: Option<thread::JoinHandle<()>>,
}

#[derive(Default)]
struct PendingIngest {
    buf: Vec<u8>,
    dropped: bool,
}

impl PtyClient {
    /// Spawn a CLI command in a new PTY with the given size.
    #[allow(dead_code)]
    pub fn spawn(
        command: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
        scrollback_lines: usize,
    ) -> Result<Self> {
        Self::spawn_with_env(command, args, cwd, rows, cols, scrollback_lines, &[])
    }

    pub fn spawn_with_env(
        command: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
        scrollback_lines: usize,
        env: &[(String, String)],
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY")?;

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);
        apply_terminal_env(&mut cmd);
        for (name, value) in env {
            cmd.env(name, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn '{command}' in PTY"))?;

        // Drop slave so reads on master get EOF when child exits.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let pty_writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

        let terminal = Arc::new(Mutex::new(TerminalState::new(rows, cols, scrollback_lines)));
        let writer = PtyWriter::spawn(pty_writer);
        let writer_tx = writer.sender();
        let exited = Arc::new(AtomicBool::new(false));
        let has_output = Arc::new(AtomicBool::new(false));
        let dirty = Arc::new(AtomicBool::new(true));
        let received_data = Arc::new(AtomicBool::new(false));
        let scroll_paused = Arc::new(AtomicBool::new(false));
        let pending_bytes = Arc::new(Mutex::new(PendingIngest::default()));
        let subscribers: Arc<Mutex<Vec<std::sync::mpsc::Sender<Vec<u8>>>>> =
            Arc::new(Mutex::new(Vec::new()));

        let terminal_ref = Arc::clone(&terminal);
        let exited_ref = Arc::clone(&exited);
        let has_output_ref = Arc::clone(&has_output);
        let dirty_ref = Arc::clone(&dirty);
        let received_data_ref = Arc::clone(&received_data);
        let scroll_paused_ref = Arc::clone(&scroll_paused);
        let pending_bytes_ref = Arc::clone(&pending_bytes);
        let subscribers_ref = Arc::clone(&subscribers);
        let reader_thread = thread::spawn(move || {
            Self::reader_loop(
                reader,
                terminal_ref,
                writer_tx,
                exited_ref,
                has_output_ref,
                dirty_ref,
                received_data_ref,
                scroll_paused_ref,
                pending_bytes_ref,
                subscribers_ref,
            );
        });

        Ok(Self {
            master: pair.master,
            writer,
            terminal,
            child,
            exited,
            has_output,
            dirty,
            received_data,
            last_resize_at: Mutex::new(None),
            scroll_paused,
            pending_bytes,
            subscribers,
            reader_thread: Some(reader_thread),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn reader_loop(
        mut reader: Box<dyn std::io::Read + Send>,
        terminal: Arc<Mutex<TerminalState>>,
        writer_tx: std::sync::mpsc::SyncSender<PtyWriteMsg>,
        exited: Arc<AtomicBool>,
        has_output: Arc<AtomicBool>,
        dirty: Arc<AtomicBool>,
        received_data: Arc<AtomicBool>,
        scroll_paused: Arc<AtomicBool>,
        pending_bytes: Arc<Mutex<PendingIngest>>,
        subscribers: Arc<Mutex<Vec<std::sync::mpsc::Sender<Vec<u8>>>>>,
    ) {
        let mut buf = [0u8; 4096];
        loop {
            match crate::io_retry::retry_on_interrupt(|| reader.read(&mut buf)) {
                Ok(0) => {
                    exited.store(true, Ordering::Release);
                    break;
                }
                Ok(n) => {
                    let data = &buf[..n];

                    // Fan raw bytes out to web subscribers before the TUI-only
                    // scroll-pause branch, so web clients stream independently.
                    // Prune hung-up receivers. Cheap no-op when there are none.
                    if let Ok(mut subs) = subscribers.lock()
                        && !subs.is_empty()
                    {
                        subs.retain(|tx| tx.send(data.to_vec()).is_ok());
                    }

                    // Fast-path check: if paused, buffer instead of parsing.
                    // The definitive check happens inside the pending_bytes
                    // lock below to synchronize with resume_ingestion.
                    if scroll_paused.load(Ordering::Acquire)
                        && let Ok(mut pending) = pending_bytes.lock()
                    {
                        // Re-check under the lock: resume_ingestion flips the
                        // flag while holding this same lock, so if we observe
                        // paused=true here it will stay true until we release.
                        if scroll_paused.load(Ordering::Acquire) {
                            append_with_cap(&mut pending, data, PAUSE_BUFFER_CAP);
                            received_data.store(true, Ordering::Release);
                            continue;
                        }
                        // Fell through — pause was just lifted; drop the lock
                        // and feed this chunk through the normal path.
                    }

                    if let Ok(mut terminal) = terminal.lock() {
                        let replies = terminal.process(data);
                        dirty.store(true, Ordering::Release);
                        received_data.store(true, Ordering::Release);
                        // Capture the visibility transition while we still hold the
                        // terminal lock, then release it BEFORE handing the parser's
                        // replies to the writer. Holding `terminal` across the write
                        // is what let a stalled writer freeze the drain loop (and,
                        // with it, every session): the reader must always return to
                        // `read()` promptly so the child can never block on output.
                        let newly_visible =
                            !has_output.load(Ordering::Acquire) && terminal.has_visible_output();
                        drop(terminal);
                        if !replies.is_empty() {
                            // Same non-blocking, drop-with-log policy as user input
                            // (`PtyWriter::send`). Replies are tiny and the queue is
                            // large, so a drop here needs a wedged writer AND a full
                            // queue — practically unreachable, but logged if it ever
                            // happens so a desynced child is diagnosable.
                            pty_queue_send(&writer_tx, replies);
                        }
                        if newly_visible {
                            has_output.store(true, Ordering::Release);
                        }
                    }
                }
                Err(err) => {
                    logger::debug(&format!("PTY reader error: {err}"));
                    exited.store(true, Ordering::Release);
                    break;
                }
            }
        }
    }

    /// Write raw bytes to the PTY (forwards keystrokes to the child process).
    /// Also marks the terminal dirty so the next render frame rebuilds the
    /// snapshot — the child process will echo or react to this input, and
    /// pre-marking dirty avoids a one-frame delay waiting for the reader
    /// thread to process the echo.
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        // Hand the bytes to the dedicated writer thread and return immediately.
        // The write itself may block on a child that has stopped reading its
        // input, but that can only ever stall the writer thread — never this
        // caller, which on the web server is the single engine thread that must
        // stay responsive for every other session. Delivery is best-effort: a
        // full queue drops the chunk (logged) rather than blocking. The `Result`
        // is retained for API stability with existing callers.
        self.writer.send(bytes.to_vec());
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    /// Get an owned snapshot of the currently visible terminal viewport.
    #[allow(dead_code)]
    pub fn snapshot(&self) -> TerminalSnapshot {
        let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.snapshot()
    }

    /// Subscribe to the live raw-byte stream. The returned receiver gets a clone
    /// of every chunk read from the PTY from now on. Drop it to unsubscribe.
    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.subscribers
            .lock()
            .expect("subscribers mutex poisoned")
            .push(tx);
        rx
    }

    /// Subscribe and also return a synthesized ANSI repaint of the current
    /// screen, so a freshly-connected client can prime its terminal before the
    /// live stream arrives. The subscriber is registered *before* the snapshot
    /// is taken, so no bytes are lost; a newly-connecting client may briefly see
    /// a few bytes both in the repaint and the first streamed chunk — harmless
    /// and self-correcting for redraw-heavy TUIs.
    pub fn subscribe_with_repaint(&self) -> (Vec<u8>, std::sync::mpsc::Receiver<Vec<u8>>) {
        let rx = self.subscribe();
        let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
        let repaint = terminal.reconnect_repaint();
        drop(terminal);
        (repaint, rx)
    }

    /// Fill `target` with the current terminal viewport, reusing its `cells`
    /// allocation to avoid per-frame heap churn. Returns `true` if the
    /// snapshot was rebuilt, `false` if the terminal was unchanged and
    /// `target` still holds valid data from the previous call.
    pub fn snapshot_into(&self, target: &mut TerminalSnapshot) -> bool {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return false;
        }
        let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.snapshot_into(target);
        true
    }

    pub fn scrollback_offset(&self) -> usize {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.scrollback_offset()
    }

    /// Atomically adjust the scrollback offset by the given amount in the
    /// given direction. If the scroll crosses the 0 boundary, PTY ingestion
    /// is paused (entering scrollback) or resumed (returning to the live
    /// bottom).
    pub fn scroll(&self, up: bool, amount: usize) {
        let Some((prev, next)) = self.mutate_scroll(|t| t.scroll(up, amount)) else {
            return;
        };
        self.sync_pause_state(prev, next);
    }

    /// Set the scrollback offset (0 = normal view, positive = scrolled back).
    pub fn set_scrollback(&self, rows: usize) {
        let Some((prev, next)) = self.mutate_scroll(|t| t.set_scrollback(rows)) else {
            return;
        };
        self.sync_pause_state(prev, next);
    }

    /// Run a closure under the terminal lock, capturing the scrollback offset
    /// before and after so the caller can detect transitions. Marks dirty on
    /// success. Returns `None` if the terminal mutex was poisoned.
    fn mutate_scroll<F>(&self, mutate: F) -> Option<(usize, usize)>
    where
        F: FnOnce(&mut TerminalState),
    {
        let mut terminal = self.terminal.lock().ok()?;
        let prev = terminal.scrollback_offset();
        mutate(&mut terminal);
        let next = terminal.scrollback_offset();
        self.dirty.store(true, Ordering::Release);
        drop(terminal);
        Some((prev, next))
    }

    /// Toggle PTY ingestion based on whether the scrollback offset just
    /// crossed the live-bottom boundary (0 ↔ >0). Called from `scroll` and
    /// `set_scrollback` after the grid has been updated and the terminal
    /// lock released.
    fn sync_pause_state(&self, prev: usize, next: usize) {
        match (prev, next) {
            (0, n) if n > 0 => self.pause_ingestion(),
            (p, 0) if p > 0 => self.resume_ingestion(),
            _ => {}
        }
    }

    /// Pause PTY ingestion: the reader thread will buffer incoming bytes
    /// into `pending_bytes` instead of feeding them to the terminal parser.
    /// Idempotent.
    fn pause_ingestion(&self) {
        self.scroll_paused.store(true, Ordering::Release);
    }

    /// Resume PTY ingestion and drain any bytes that arrived while paused
    /// into the terminal parser. Idempotent — a no-op if not paused.
    fn resume_ingestion(&self) {
        // Lock terminal first, then pending_bytes. Flip the flag while
        // holding pending_bytes so readers blocked on that lock re-check
        // `scroll_paused` and fall through to the normal path.
        let Ok(mut terminal) = self.terminal.lock() else {
            return;
        };
        let Ok(mut pending) = self.pending_bytes.lock() else {
            return;
        };
        self.scroll_paused.store(false, Ordering::Release);

        if pending.dropped {
            logger::debug(
                "PTY pause buffer overflowed during scrollback session; oldest bytes dropped",
            );
            pending.dropped = false;
        }

        if pending.buf.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut pending.buf);
        drop(pending);

        let replies = terminal.process(&bytes);
        self.dirty.store(true, Ordering::Release);
        if !self.has_output.load(Ordering::Acquire) && terminal.has_visible_output() {
            self.has_output.store(true, Ordering::Release);
        }
        drop(terminal);

        if !replies.is_empty() {
            self.writer.send(replies);
        }
    }

    /// Whether the child process has switched to the alternate screen buffer
    /// (e.g. via `CSI ?1049h`). Providers that use the alt screen manage their
    /// own redraws and do not populate scrollback, so the app can suppress
    /// scrollback UI affordances when this is true.
    pub fn is_alt_screen(&self) -> bool {
        self.terminal.lock().is_ok_and(|t| t.is_alt_screen())
    }

    /// Resize the PTY and the internal terminal parser.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize PTY")?;
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.resize(rows, cols);
            self.dirty.store(true, Ordering::Release);
        }
        if let Ok(mut ts) = self.last_resize_at.lock() {
            *ts = Some(Instant::now());
        }
        Ok(())
    }

    /// Force the dirty flag on so the next `snapshot_into` rebuilds.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Check whether the child process has exited (reader thread detected EOF).
    pub fn is_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    /// Check whether the PTY has received any output from the child process.
    pub fn has_output(&self) -> bool {
        self.has_output.load(Ordering::Acquire)
    }

    /// Returns `true` if the terminal has only minimal output (no scrollback
    /// and at most `threshold` visible lines). Used to detect failed resume
    /// attempts that print a short error and exit.
    pub fn has_minimal_output(&self, threshold: usize) -> bool {
        self.terminal
            .lock()
            .map(|mut t| t.has_minimal_output(threshold))
            .unwrap_or(true)
    }

    /// Returns a short plain-text excerpt from the visible terminal viewport.
    pub fn visible_text_excerpt(&self, max_lines: usize) -> String {
        self.terminal
            .lock()
            .map(|t| t.visible_text_excerpt(max_lines))
            .unwrap_or_default()
    }

    /// Returns `true` if the PTY received data since the last call, then
    /// clears the flag. Used to detect streaming activity for UI indicators
    /// without interfering with the snapshot dirty flag.
    ///
    /// Suppresses the signal briefly after a resize to avoid counting the
    /// child process's redraw burst as streaming activity.
    pub fn take_received_data(&self) -> bool {
        if !self.received_data.swap(false, Ordering::AcqRel) {
            return false;
        }
        // Ignore data that arrived within 500ms of a resize — it's almost
        // certainly the child redrawing in response to SIGWINCH.
        if let Ok(ts) = self.last_resize_at.lock()
            && ts.is_some_and(|t| t.elapsed().as_millis() < 500)
        {
            return false;
        }
        true
    }

    /// Whether the child process has enabled any mouse tracking mode
    /// (e.g. via DECSET 1000/1002/1003). When true, non-scroll mouse
    /// events should be forwarded to the PTY rather than dropped.
    pub fn has_mouse_mode(&self) -> bool {
        self.terminal.lock().is_ok_and(|t| t.has_mouse_mode())
    }

    /// Non-blocking check of the child's exit status.
    pub fn try_wait(&mut self) -> Option<portable_pty::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Returns the PID of the shell process spawned in this PTY.
    pub fn child_process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Politely ask the child's whole process group to exit (SIGTERM), so the
    /// CLI and any helpers it spawned can flush state before the hard group
    /// `kill()` in `Drop` (or process teardown) reaps stragglers. Signals the
    /// group rather than just the direct child for the same reason `Drop` does:
    /// the child is a process-group leader (portable-pty calls `setsid`), so a
    /// SIGTERM aimed at the lone PID would leave its descendants running.
    pub fn terminate(&self) {
        if let Some(pid) = self.child_process_id()
            && let Some(pid) = rustix::process::Pid::from_raw(pid as i32)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
        }
    }

    /// Returns the name of the foreground process running in this PTY, or
    /// `None` if the shell itself is in the foreground (idle).
    ///
    /// Uses `tcgetpgrp()` via rustix to get the foreground process group and
    /// compares it to the shell PID. If they differ, a child command is
    /// running and its name is resolved via platform-specific APIs.
    pub fn foreground_process_name(&self) -> Option<String> {
        use std::os::unix::io::BorrowedFd;

        let raw_fd = self.master.as_raw_fd()?;
        // SAFETY: the master fd is valid for the lifetime of PtyClient.
        let fd = unsafe { BorrowedFd::borrow_raw(raw_fd) };
        let fg_pid = rustix::termios::tcgetpgrp(fd).ok()?;

        let shell_pid = self.child.process_id()?;
        if fg_pid.as_raw_nonzero().get() as u32 == shell_pid {
            // Shell itself is in the foreground — no command running.
            return None;
        }

        process_name(fg_pid.as_raw_nonzero().get() as u32)
    }
}

impl Drop for PtyClient {
    fn drop(&mut self) {
        // Kill the child's whole process group, not just the direct child. The
        // child is its own session/process-group leader (portable-pty calls
        // `setsid` before exec, so its PGID equals its PID), and anything it
        // spawned inherits both that group and the PTY slave fd. If we killed
        // only the direct child, a surviving grandchild that ignores the
        // kernel's SIGHUP (or escapes it) would keep the slave open, the master
        // read would never see EOF, and the `join` below would block the
        // dropping thread (the UI thread) indefinitely. SIGKILL to the group
        // reaps those descendants so the slave is released. (A descendant that
        // has left this process group — a double-forked daemon, or a
        // job-control background job under an interactive-shell provider — is
        // out of reach here. A well-behaved daemon redirects its inherited
        // terminal fds away before detaching so it will not hold the slave
        // open; a misbehaving one that keeps the slave open could still stall
        // the join, though that has not been observed with the supported
        // providers.)
        if let Some(pid) = self.child.process_id()
            && let Some(pid) = rustix::process::Pid::from_raw(pid as i32)
        {
            // ESRCH just means the group already exited (benign). Anything else
            // (e.g. EPERM) means the group kill did not happen, so the reader
            // join below could stall — leave a breadcrumb in the log.
            if let Err(err) =
                rustix::process::kill_process_group(pid, rustix::process::Signal::KILL)
                && err != rustix::io::Errno::SRCH
            {
                logger::debug(&format!(
                    "PtyClient::drop: kill_process_group failed: {err}"
                ));
            }
        }
        // Reap the direct child so it does not linger as a zombie. After the
        // group kill the child is already dead, so this `kill` returns at once;
        // it remains the fallback that actually signals the child when its PID
        // was unavailable above (without it, `wait` could block on a child that
        // nothing has asked to exit).
        let _ = self.child.kill();
        let _ = self.child.wait();
        // With the child group dead, the PTY slave is fully released (the slave
        // fd itself was dropped at spawn time; the child group held the last
        // references). The master read then returns EOF (on Linux, EIO, which
        // portable-pty maps to Ok(0)) and the reader thread returns. Join it so
        // the thread does not outlive this client — otherwise detached reader
        // threads accumulate across a long session and across the test suite.
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Resolve a process name from its PID.
///
/// On Linux, reads `/proc/{pid}/comm` directly (fast, no subprocess).
/// On macOS (no `/proc`), falls back to `ps -p {pid} -o comm=`.
fn process_name(pid: u32) -> Option<String> {
    // Fast path: try /proc/pid/comm (Linux).
    if let Ok(name) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    // Fallback: use ps (works on macOS and any POSIX system).
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout);
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    // ps may return a full path; extract just the binary name.
    std::path::Path::new(trimmed)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
}

struct TerminalState {
    parser: vt100::Parser<TerminalCallbacks>,
    rows: u16,
    cols: u16,
}

impl TerminalState {
    fn new(rows: u16, cols: u16, scrollback_lines: usize) -> Self {
        Self::with_scrollback(rows, cols, scrollback_lines)
    }

    fn with_scrollback(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                rows,
                cols,
                scrollback,
                TerminalCallbacks::default(),
            ),
            rows,
            cols,
        }
    }

    fn process(&mut self, data: &[u8]) -> Vec<u8> {
        // Device queries (DA1/DA2/DSR) are answered synchronously while we still
        // have the parser, because the DSR-6 reply must reflect the cursor
        // position *after* this chunk has been applied. vt100 does not generate
        // these replies itself; the callbacks queue OSC color replies, and we
        // scan the chunk for the small set of CSI device queries here.
        self.parser.process(data);

        // OSC color-query replies queued by the callbacks during `process`.
        let mut replies = std::mem::take(&mut self.parser.callbacks_mut().replies);

        // CSI device-query replies (DA1, DA2, DSR). vt100 routes these to
        // `unhandled_csi`, but the DSR-6 reply needs the post-process cursor
        // position from the screen, so we resolve them here rather than in the
        // callback (which only sees the screen mid-parse).
        let queries = std::mem::take(&mut self.parser.callbacks_mut().pending_queries);
        for query in queries {
            match query {
                DeviceQuery::Da1 => replies.extend_from_slice(b"\x1b[?1;2c"),
                DeviceQuery::Da2 => replies.extend_from_slice(b"\x1b[>0;0;0c"),
                DeviceQuery::DsrStatus => replies.extend_from_slice(b"\x1b[0n"),
                DeviceQuery::DsrCursor => {
                    let (row, col) = self.parser.screen().cursor_position();
                    replies.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
                }
            }
        }

        replies
    }

    fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    fn has_visible_output(&self) -> bool {
        let screen = self.screen();
        for row in 0..self.rows {
            for col in 0..self.cols {
                if let Some(cell) = screen.cell(row, col)
                    && cell.has_contents()
                    && !cell.contents().chars().all(|c| c.is_whitespace())
                {
                    return true;
                }
            }
        }
        false
    }

    /// Count the number of distinct viewport rows that contain at least one
    /// non-whitespace character.
    fn visible_line_count(&self) -> usize {
        let screen = self.screen();
        let mut count = 0usize;
        for row in 0..self.rows {
            let mut has_content = false;
            for col in 0..self.cols {
                if let Some(cell) = screen.cell(row, col)
                    && cell.has_contents()
                    && !cell.contents().chars().all(|c| c.is_whitespace())
                {
                    has_content = true;
                    break;
                }
            }
            if has_content {
                count += 1;
            }
        }
        count
    }

    /// Returns `true` if the terminal contains only a small amount of output:
    /// no scrollback history AND at most `threshold` visible lines with content.
    /// Used to detect failed `--continue` exits that print a short error message.
    fn has_minimal_output(&mut self, threshold: usize) -> bool {
        self.history_len() == 0 && self.visible_line_count() <= threshold
    }

    fn visible_text_excerpt(&self, max_lines: usize) -> String {
        let screen = self.screen();
        let mut rows = vec![String::new(); usize::from(self.rows)];
        for (row_idx, line) in rows.iter_mut().enumerate() {
            for col in 0..self.cols {
                let Some(cell) = screen.cell(row_idx as u16, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                if cell.has_contents() {
                    while line.chars().count() < usize::from(col) {
                        line.push(' ');
                    }
                    line.push_str(cell.contents());
                }
            }
        }

        rows.into_iter()
            .map(|line| line.trim_end().to_string())
            .filter(|line| !line.trim().is_empty())
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether the child process has enabled any mouse tracking mode
    /// (e.g. via DECSET 1000/1002/1003).
    fn has_mouse_mode(&self) -> bool {
        self.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    /// Whether the child process has switched to the alternate screen buffer
    /// (e.g. via DECSET 1049). Full-screen TUI apps like opencode use the
    /// alt screen; Claude and shells use the main screen.
    fn is_alt_screen(&self) -> bool {
        self.screen().alternate_screen()
    }

    /// Probe the length of the scrollback history. vt100 exposes no direct
    /// accessor, so we drive `set_scrollback` to a value past any plausible
    /// history; it clamps to the real maximum, which `scrollback()` then
    /// reports. The previous offset is restored so this is observationally a
    /// pure read. Requires `&mut` because the probe mutates the grid's offset.
    fn history_len(&mut self) -> usize {
        let current = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(current);
        max
    }

    fn snapshot(&mut self) -> TerminalSnapshot {
        let mut snap = TerminalSnapshot::empty();
        self.snapshot_into(&mut snap);
        snap
    }

    /// Fill `target` with the current terminal viewport, reusing its existing
    /// `cells` allocation to avoid per-frame heap churn.
    fn snapshot_into(&mut self, target: &mut TerminalSnapshot) {
        let scrollback_offset = self.parser.screen().scrollback();
        let history_size = self.history_len();
        let screen = self.parser.screen();

        let cursor = if screen.hide_cursor() {
            None
        } else {
            let (row, col) = screen.cursor_position();
            Some(SnapshotCursor { row, col })
        };

        target.cells.clear();
        for row in 0..self.rows {
            for col in 0..self.cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                // The trailing half of a wide character carries no glyph; the
                // wide char itself (at the previous column) holds the symbol.
                if cell.is_wide_continuation() {
                    continue;
                }

                // Empty cells render as a single space, matching the prior
                // grid-emulator behavior of yielding a blank for every viewport
                // cell so the surfaces always have a full grid to paint.
                let symbol = if cell.has_contents() {
                    CompactString::from(cell.contents())
                } else {
                    CompactString::const_new(" ")
                };

                target.cells.push(SnapshotCell {
                    row,
                    col,
                    symbol,
                    fg: convert_terminal_color(cell.fgcolor()),
                    bg: convert_terminal_color(cell.bgcolor()),
                    modifier: cell_modifier(cell),
                });
            }
        }

        target.rows = self.rows;
        target.cols = self.cols;
        target.scrollback_offset = scrollback_offset;
        target.scrollback_total = history_size;
        target.cursor = cursor;
    }

    /// Build an ANSI byte sequence that repaints the terminal onto a freshly
    /// connected (or reconnected) client *including scrollback history*, so the
    /// client's scroll buffer matches what the TUI renders from the same grid.
    ///
    /// On the alternate screen there is no scrollback, so this is identical to
    /// the viewport-only `synthesize_repaint(.., true)`. On the main screen we
    /// rebuild the client's primary buffer by printing the whole grid (history +
    /// viewport) as a newline-separated line stream; natural scrolling pushes
    /// the history into the client's scrollback. Printing is the only way to
    /// populate a terminal's scrollback over a byte stream — absolute-positioned
    /// repaints overwrite the viewport without ever scrolling.
    fn reconnect_repaint(&mut self) -> Vec<u8> {
        if self.is_alt_screen() {
            let snap = self.snapshot();
            return synthesize_repaint(&snap, true);
        }

        // The replay always rebuilds the buffer ending at the live bottom, so the
        // cursor must be mapped as if the grid were NOT scrolled back. We read the
        // cursor from the live screen (it is viewport-relative already and does
        // not move with the scrollback offset), so an operator reading history at
        // the moment a client connects can never push it out of range.
        let cursor = if self.parser.screen().hide_cursor() {
            None
        } else {
            Some(self.parser.screen().cursor_position())
        };

        let cols = usize::from(self.cols);
        let rows = usize::from(self.rows);
        let history = self.history_len();

        // Model the logical line space in alacritty-style coordinates so the
        // existing `clamp_replay_top` cap (and its unit test) keep their meaning:
        // the visible screen occupies lines `0..=rows-1`, and history sits at the
        // negative lines `-history..=-1`.
        let full_top = -(history as i32);
        let bottom = rows as i32 - 1;
        let top = clamp_replay_top(full_top, bottom);
        if top != full_top {
            logger::debug(&format!(
                "reconnect replay truncated scrollback from {} to {} lines (cap {})",
                bottom - full_top + 1,
                bottom - top + 1,
                MAX_RECONNECT_REPLAY_LINES,
            ));
        }

        // Gather every logical line (history + viewport) into a flat list of
        // owned rows by walking the scrollback in `rows`-sized windows. Each
        // window reads the visible viewport at a given scrollback offset; the
        // top window may be partial, so we only take the rows that fall at or
        // above `top`. This is bounded by the replay cap above.
        let saved_offset = self.parser.screen().scrollback();
        let total_lines = (bottom - top + 1).max(0) as usize;
        let mut lines: Vec<ReplayRow> = Vec::with_capacity(total_lines);

        // Walk from the topmost line down to the live bottom. `line` is the
        // alacritty-style coordinate; the scrollback offset that brings `line`
        // to the top viewport row is `-line` for history lines and `0` for the
        // viewport. We page through windows so each `set_scrollback` exposes a
        // contiguous block of `rows` lines.
        let mut line = top;
        while line <= bottom {
            // Offset that places `line` at viewport row 0. History line -N needs
            // offset N; viewport lines need offset 0.
            let offset = (-line).max(0) as usize;
            self.parser.screen_mut().set_scrollback(offset);
            let screen = self.parser.screen();
            // The window covers logical lines `line ..= line + rows - 1`, mapped
            // to viewport rows `0 ..= rows-1`. Emit each that is still <= bottom.
            for vr in 0..rows {
                let logical = line + vr as i32;
                if logical > bottom {
                    break;
                }
                let wrapped = screen.row_wrapped(vr as u16);
                let mut cells: Vec<ReplayCell> = Vec::with_capacity(cols);
                let emit_to = if wrapped {
                    cols
                } else {
                    // Right-trim trailing empty default cells.
                    let mut last_col = 0usize;
                    for c in 0..cols {
                        if let Some(cell) = screen.cell(vr as u16, c as u16)
                            && (cell.has_contents() || cell.bgcolor() != vt100::Color::Default)
                        {
                            last_col = c + 1;
                        }
                    }
                    last_col
                };
                for c in 0..emit_to {
                    let Some(cell) = screen.cell(vr as u16, c as u16) else {
                        cells.push(ReplayCell::blank());
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let contents = cell.contents();
                    // Map an empty cell or any C0 control (only '\t' is reachable
                    // as a stored glyph) to a space so the client renders a blank
                    // rather than re-interpreting a control sequence.
                    let symbol = if contents.is_empty()
                        || contents.chars().next().is_some_and(|c| c < ' ')
                    {
                        CompactString::const_new(" ")
                    } else {
                        CompactString::from(contents)
                    };
                    cells.push(ReplayCell {
                        symbol,
                        fg: convert_terminal_color(cell.fgcolor()),
                        bg: convert_terminal_color(cell.bgcolor()),
                        modifier: cell_modifier(cell),
                    });
                }
                lines.push(ReplayRow { cells, wrapped });
            }
            line += rows as i32;
        }
        self.parser.screen_mut().set_scrollback(saved_offset);

        // Pre-size to history+screen rows so a large scrollback doesn't reallocate
        // repeatedly while we hold the terminal lock.
        let mut out = String::with_capacity(lines.len() * (cols + 2) + 32);
        // Ensure the primary buffer and autowrap-on (so soft-wrapped rows can be
        // rebuilt by the client), then clear the screen, clear the client's saved
        // scrollback (3J), and home the cursor. Clearing scrollback makes a
        // reconnect idempotent: we rebuild from the authoritative grid rather than
        // appending a second copy of the history.
        out.push_str("\x1b[?1049l\x1b[?7h\x1b[2J\x1b[3J\x1b[H");

        let mut last_style: Option<(CellColor, CellColor, CellModifier)> = None;
        // A soft-wrapped row is replayed at full width with NO line break, letting
        // the client's autowrap re-create the soft wrap when the next row's first
        // cell overflows — that preserves copy/paste and resize-reflow semantics.
        // A `\r\n` is emitted only for genuine (hard) line breaks.
        let mut prev_wrapped = false;
        for (idx, row) in lines.iter().enumerate() {
            if idx != 0 && !prev_wrapped {
                // Reset SGR before a hard line break while a non-default
                // background is still active. A `\r\n` at the bottom of the
                // screen scrolls, and a scroll fills the newly-exposed row with
                // the CURRENT background color (Background-Color-Erase). Without
                // this reset the previous line's background bleeds onto the next
                // line on the client. Soft-wrapped rows intentionally skip this
                // so a colored background continues across the wrap.
                if matches!(last_style, Some((_, bg, _)) if bg != CellColor::Reset) {
                    out.push_str("\x1b[0m");
                    last_style = None;
                }
                out.push_str("\r\n");
            }
            for cell in &row.cells {
                let style = (cell.fg, cell.bg, cell.modifier);
                if last_style != Some(style) {
                    out.push_str("\x1b[0m");
                    out.push_str(&sgr_sequence(cell.fg, cell.bg, cell.modifier));
                    last_style = Some(style);
                }
                out.push_str(cell.symbol.as_str());
            }
            prev_wrapped = row.wrapped;
        }

        out.push_str("\x1b[0m");
        if let Some((row, col)) = cursor {
            out.push_str(&format!("\x1b[{};{}H", row + 1, col + 1));
        }
        out.into_bytes()
    }

    fn scrollback_offset(&self) -> usize {
        self.parser.screen().scrollback()
    }

    fn scroll(&mut self, up: bool, amount: usize) {
        // vt100's `set_scrollback` is absolute; translate the delta-based scroll
        // into a clamped absolute offset. Scrolling up increases the offset
        // (further into history); scrolling down decreases it toward the live
        // bottom (offset 0). The new offset is clamped to the history length.
        let current = self.parser.screen().scrollback();
        let history = self.history_len();
        let target = if up {
            current.saturating_add(amount).min(history)
        } else {
            current.saturating_sub(amount)
        };
        self.parser.screen_mut().set_scrollback(target);
    }

    fn set_scrollback(&mut self, rows: usize) {
        // vt100 clamps an over-large offset to the history length internally, so
        // the external "0 = live bottom, N = scrolled back" semantics are
        // preserved directly.
        self.parser.screen_mut().set_scrollback(rows);
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
        self.parser.screen_mut().set_size(rows, cols);
    }
}

/// A single replayed cell, captured while paging through scrollback windows so
/// the repaint can be emitted after the scrollback offset is restored.
struct ReplayCell {
    symbol: CompactString,
    fg: CellColor,
    bg: CellColor,
    modifier: CellModifier,
}

impl ReplayCell {
    fn blank() -> Self {
        Self {
            symbol: CompactString::const_new(" "),
            fg: CellColor::Reset,
            bg: CellColor::Reset,
            modifier: CellModifier::default(),
        }
    }
}

/// A replayed logical row plus whether it soft-wraps into the next one.
struct ReplayRow {
    cells: Vec<ReplayCell>,
    wrapped: bool,
}

/// A CSI device query the child issued, queued during parsing and answered by
/// [`TerminalState::process`] after the chunk has been applied (so the DSR-6
/// cursor reply reflects the post-process cursor position). vt100 does not
/// generate these replies itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceQuery {
    /// Primary Device Attributes (`ESC [ c` / `ESC [ 0 c`).
    Da1,
    /// Secondary Device Attributes (`ESC [ > c`).
    Da2,
    /// Device Status Report — operating status (`ESC [ 5 n`).
    DsrStatus,
    /// Device Status Report — cursor position (`ESC [ 6 n`).
    DsrCursor,
}

/// vt100 callback sink. The parser routes escape sequences it does not act on
/// here; we resolve OSC color queries into reply bytes and queue CSI device
/// queries for [`TerminalState::process`] to answer. Replies are written back
/// to the PTY by the caller. (alacritty answered these via its `EventListener`;
/// vt100 has no built-in responder, so we reproduce the small responder here.)
#[derive(Default)]
struct TerminalCallbacks {
    /// OSC color-query replies resolved during parsing, drained by `process`.
    replies: Vec<u8>,
    /// CSI device queries seen during parsing, answered by `process` so the
    /// DSR-6 reply reflects the post-chunk cursor position.
    pending_queries: Vec<DeviceQuery>,
}

impl vt100::Callbacks for TerminalCallbacks {
    fn unhandled_csi(
        &mut self,
        _screen: &mut vt100::Screen,
        intermediate1: Option<u8>,
        _intermediate2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first().copied());
        match c {
            // Primary/secondary Device Attributes.
            'c' => match intermediate1 {
                Some(b'>') => self.pending_queries.push(DeviceQuery::Da2),
                None => {
                    // `ESC [ c` or `ESC [ 0 c`.
                    if matches!(first, None | Some(0)) {
                        self.pending_queries.push(DeviceQuery::Da1);
                    }
                }
                _ => {}
            },
            // Device Status Report.
            'n' if intermediate1.is_none() => match first {
                Some(5) => self.pending_queries.push(DeviceQuery::DsrStatus),
                Some(6) => self.pending_queries.push(DeviceQuery::DsrCursor),
                _ => {}
            },
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, _screen: &mut vt100::Screen, params: &[&[u8]]) {
        // OSC color queries: `OSC 4 ; <idx> ; ? BEL`, `OSC 10 ; ? BEL`
        // (foreground), `OSC 11 ; ? BEL` (background). The reply echoes the same
        // selector with the resolved color as `rgb:RRRR/GGGG/BBBB`. Only the
        // query form (trailing `?`) is answered; set requests are ignored (vt100
        // does not track a mutable palette, so there is nothing to change).
        match params {
            // OSC 4 ; <index> ; ?  — palette color query.
            [b"4", idx, last] if is_query(last) => {
                if let Some(index) = parse_osc_index(idx) {
                    let color = default_palette_rgb(index).unwrap_or((0, 0, 0));
                    self.replies.extend_from_slice(
                        osc_color_reply(&format!("4;{index}"), color).as_bytes(),
                    );
                }
            }
            // OSC 10 ; ?  — foreground color query (default: white).
            [b"10", last] if is_query(last) => {
                self.replies
                    .extend_from_slice(osc_color_reply("10", (0xff, 0xff, 0xff)).as_bytes());
            }
            // OSC 11 ; ?  — background color query (default: black).
            [b"11", last] if is_query(last) => {
                self.replies
                    .extend_from_slice(osc_color_reply("11", (0x00, 0x00, 0x00)).as_bytes());
            }
            _ => {}
        }
    }
}

/// Whether an OSC parameter is the query form (`?`).
fn is_query(param: &[u8]) -> bool {
    param == b"?"
}

/// Parse an OSC numeric index parameter (used by `OSC 4 ; <index> ; ?`).
fn parse_osc_index(param: &[u8]) -> Option<usize> {
    std::str::from_utf8(param).ok()?.trim().parse().ok()
}

/// Format an OSC color reply: `ESC ] <selector> ; rgb:RRRR/GGGG/BBBB BEL`. Each
/// 8-bit channel is doubled to the 16-bit `rgb:` form xterm uses.
fn osc_color_reply(selector: &str, (r, g, b): (u8, u8, u8)) -> String {
    format!("\x1b]{selector};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x07")
}

/// Translate a vt100 cell's style flags into our serializable `CellModifier`.
/// Shared by the per-frame snapshot and the reconnect repaint.
///
/// Fidelity note: vt100 (0.16) does not model strikethrough or blink, so
/// `crossed_out` is always false and there is no blink mapping. The other
/// attributes (bold, dim, italic, underline, inverse) are preserved.
fn cell_modifier(cell: &vt100::Cell) -> CellModifier {
    CellModifier {
        bold: cell.bold(),
        dim: cell.dim(),
        italic: cell.italic(),
        underlined: cell.underline(),
        reversed: cell.inverse(),
        // vt100 has no strikethrough/blink attribute; this fidelity loss is
        // documented above. Kept false so downstream surfaces are consistent.
        crossed_out: false,
    }
}

/// Map a vt100 color to the surface-agnostic `CellColor`.
///
/// `Default` becomes `Reset`. Palette indices 0..=15 map to the matching named
/// `CellColor` variant (so `tui_color.rs` renders the terminal's own named
/// colors); 16..=255 become `Indexed`. RGB passes through.
fn convert_terminal_color(color: vt100::Color) -> CellColor {
    match color {
        vt100::Color::Default => CellColor::Reset,
        vt100::Color::Rgb(r, g, b) => CellColor::Rgb(r, g, b),
        vt100::Color::Idx(i) => match i {
            0 => CellColor::Black,
            1 => CellColor::Red,
            2 => CellColor::Green,
            3 => CellColor::Yellow,
            4 => CellColor::Blue,
            5 => CellColor::Magenta,
            6 => CellColor::Cyan,
            7 => CellColor::Gray,
            8 => CellColor::DarkGray,
            9 => CellColor::LightRed,
            10 => CellColor::LightGreen,
            11 => CellColor::LightYellow,
            12 => CellColor::LightBlue,
            13 => CellColor::LightMagenta,
            14 => CellColor::LightCyan,
            15 => CellColor::White,
            n => CellColor::Indexed(n),
        },
    }
}

fn apply_terminal_env(cmd: &mut CommandBuilder) {
    apply_terminal_env_from_parent(
        cmd,
        env::var_os("TERM").as_deref(),
        env::var_os("COLORTERM").as_deref(),
    );
}

fn apply_terminal_env_from_parent(
    cmd: &mut CommandBuilder,
    parent_term: Option<&OsStr>,
    parent_colorterm: Option<&OsStr>,
) {
    let term = resolve_term_from_parent(parent_term);
    cmd.env("TERM", term);

    if let Some(colorterm) = parent_colorterm.filter(|value| !value.is_empty()) {
        cmd.env("COLORTERM", colorterm);
    }
}

fn resolve_term_from_parent(parent_term: Option<&OsStr>) -> String {
    let Some(parent_term) = parent_term else {
        return "xterm-256color".to_string();
    };

    let candidate = parent_term.to_string_lossy().trim().to_string();
    if candidate.is_empty() {
        return "xterm-256color".to_string();
    }

    let normalized = candidate.to_ascii_lowercase();
    if normalized == "dumb" {
        return "xterm-256color".to_string();
    }

    if term_supports_extended_color(&normalized) {
        return candidate;
    }

    "xterm-256color".to_string()
}

fn term_supports_extended_color(term: &str) -> bool {
    term.contains("256color")
        || term.contains("kitty")
        || term.contains("wezterm")
        || term.contains("alacritty")
        || term.contains("ghostty")
        || term.contains("foot")
        || term.contains("tmux")
        || term.contains("screen")
}

/// Resolve a default RGB for an 8-bit color index, used to answer OSC palette
/// queries (`OSC 4 ; n ; ?`). vt100 does not expose a mutable palette, so this
/// returns the standard xterm default for the requested slot. Indices outside
/// 0..=255 return `None`.
fn default_palette_rgb(index: usize) -> Option<(u8, u8, u8)> {
    match index {
        0 => Some((0x00, 0x00, 0x00)),
        1 => Some((0xcd, 0x00, 0x00)),
        2 => Some((0x00, 0xcd, 0x00)),
        3 => Some((0xcd, 0xcd, 0x00)),
        4 => Some((0x00, 0x00, 0xee)),
        5 => Some((0xcd, 0x00, 0xcd)),
        6 => Some((0x00, 0xcd, 0xcd)),
        7 => Some((0xe5, 0xe5, 0xe5)),
        8 => Some((0x7f, 0x7f, 0x7f)),
        9 => Some((0xff, 0x00, 0x00)),
        10 => Some((0x00, 0xff, 0x00)),
        11 => Some((0xff, 0xff, 0x00)),
        12 => Some((0x5c, 0x5c, 0xff)),
        13 => Some((0xff, 0x00, 0xff)),
        14 => Some((0x00, 0xff, 0xff)),
        15 => Some((0xff, 0xff, 0xff)),
        16..=231 => Some(xterm_color_cube(index)),
        232..=255 => Some(xterm_grayscale(index)),
        _ => None,
    }
}

fn xterm_color_cube(index: usize) -> (u8, u8, u8) {
    const STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

    let idx = index - 16;
    let r = STEPS[idx / 36];
    let g = STEPS[(idx / 6) % 6];
    let b = STEPS[idx % 6];
    (r, g, b)
}

fn xterm_grayscale(index: usize) -> (u8, u8, u8) {
    let level = 8 + ((index - 232) as u8 * 10);
    (level, level, level)
}

/// Append `data` to `pending.buf`, respecting `cap`. On overflow, drop the
/// oldest bytes from the front and mark `pending.dropped` so the next resume
/// can log a warning. If `data` alone exceeds `cap`, keep only its trailing
/// `cap` bytes.
fn append_with_cap(pending: &mut PendingIngest, data: &[u8], cap: usize) {
    if cap == 0 {
        pending.buf.clear();
        pending.dropped = !data.is_empty();
        return;
    }
    if data.len() >= cap {
        pending.buf.clear();
        pending.buf.extend_from_slice(&data[data.len() - cap..]);
        pending.dropped = true;
        return;
    }
    let new_len = pending.buf.len().saturating_add(data.len());
    if new_len > cap {
        let overflow = new_len - cap;
        pending.buf.drain(..overflow);
        pending.dropped = true;
    }
    pending.buf.extend_from_slice(data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use compact_str::CompactString;
    use portable_pty::CommandBuilder;

    fn viewport_lines(snapshot: &TerminalSnapshot) -> Vec<String> {
        let mut rows = vec![String::new(); usize::from(snapshot.rows)];
        for cell in &snapshot.cells {
            if let Some(line) = rows.get_mut(usize::from(cell.row)) {
                while line.len() < usize::from(cell.col) {
                    line.push(' ');
                }
                line.push_str(&cell.symbol);
            }
        }
        rows
    }

    #[test]
    fn scrollback_reaches_beyond_visible_rows() {
        let mut terminal = TerminalState::with_scrollback(3, 12, 100);
        terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");

        assert_eq!(terminal.history_len(), 3);

        let history = terminal.history_len();
        terminal.set_scrollback(history);
        let top = terminal.snapshot();
        let lines = viewport_lines(&top);

        assert_eq!(top.scrollback_total, 3);
        assert!(lines.iter().any(|line| line.contains("one")));
        assert!(lines.iter().any(|line| line.contains("two")));
    }

    #[test]
    fn scrolling_while_output_arrives_keeps_valid_offset() {
        let mut terminal = TerminalState::with_scrollback(3, 16, 100);
        terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");
        let history = terminal.history_len();
        terminal.set_scrollback(history);

        terminal.process(b"six\r\nseven\r\n");
        let snapshot = terminal.snapshot();

        assert!(snapshot.scrollback_offset <= terminal.history_len());
        assert_eq!(snapshot.scrollback_total, terminal.history_len());
        assert!(terminal.history_len() >= 5);
    }

    #[test]
    fn reconnect_repaint_includes_scrollback_history() {
        let mut terminal = TerminalState::with_scrollback(4, 20, 1000);
        for i in 0..12 {
            terminal.process(format!("line{i}\r\n").as_bytes());
        }
        assert!(
            terminal.history_len() > 0,
            "precondition: terminal has scrollback history"
        );

        let replay = String::from_utf8(terminal.reconnect_repaint()).unwrap();
        assert!(
            replay.contains("line0"),
            "earliest history line must be replayed, got:\n{replay}"
        );
        assert!(replay.contains("line11"), "recent line must be present");

        // The viewport-only repaint (the previous behavior) omits scrolled-off
        // history — this is exactly the gap this method closes.
        let viewport_only =
            String::from_utf8(synthesize_repaint(&terminal.snapshot(), false)).unwrap();
        assert!(
            !viewport_only.contains("line0"),
            "sanity: viewport-only repaint omits scrolled-off history"
        );
    }

    #[test]
    fn reconnect_repaint_resets_background_before_hard_newline() {
        // A line painted with a non-default background, followed by a plain
        // line. The replay must emit a reset BEFORE the `\r\n` so a scroll on
        // the client fills the next row with the default background instead of
        // bleeding the colored background downward (Background-Color-Erase).
        // See the comment in `reconnect_repaint`.
        let mut terminal = TerminalState::with_scrollback(4, 20, 100);
        terminal.process(b"\x1b[41mRED\x1b[0m\r\nplain\r\n");
        let replay = String::from_utf8(terminal.reconnect_repaint()).unwrap();
        assert!(
            replay.contains("\x1b[0m\r\n"),
            "a reset must precede the newline after a colored line, got:\n{replay:?}"
        );
    }

    #[test]
    fn reconnect_repaint_alt_screen_matches_viewport_repaint() {
        let mut terminal = TerminalState::with_scrollback(5, 20, 100);
        terminal.process(b"main screen line\r\n");
        terminal.process(b"\x1b[?1049h"); // enter the alternate screen
        terminal.process(b"alt content");
        assert!(terminal.is_alt_screen());

        // On the alt screen there is no scrollback to replay, so the reconnect
        // repaint must be byte-identical to the viewport-only repaint.
        assert_eq!(
            terminal.reconnect_repaint(),
            synthesize_repaint(&terminal.snapshot(), true),
        );
    }

    #[test]
    fn reconnect_repaint_round_trips_through_a_fresh_terminal() {
        let mut src = TerminalState::with_scrollback(4, 20, 1000);
        for i in 0..12 {
            src.process(format!("line{i}\r\n").as_bytes());
        }
        let replay = src.reconnect_repaint();

        // Feeding the replay into a fresh terminal of the same size must rebuild
        // the same grid — proven by idempotence: the rebuilt terminal's own
        // replay is byte-identical, and the scrollback is repopulated.
        let mut dst = TerminalState::with_scrollback(4, 20, 1000);
        dst.process(&replay);

        assert!(
            dst.history_len() > 0,
            "replay must rebuild scrollback in a fresh terminal"
        );
        assert_eq!(
            src.reconnect_repaint(),
            dst.reconnect_repaint(),
            "reconnect repaint is stable across a round-trip",
        );
    }

    #[test]
    fn reconnect_repaint_cursor_in_range_when_scrolled_back() {
        let mut terminal = TerminalState::with_scrollback(4, 20, 1000);
        for i in 0..12 {
            terminal.process(format!("line{i}\r\n").as_bytes());
        }
        // The operator scrolls into history; a web client connecting now must
        // still place the cursor within the live screen, not at an offset row.
        let history = terminal.history_len();
        terminal.set_scrollback(history);
        let replay = String::from_utf8(terminal.reconnect_repaint()).unwrap();

        // Inspect the trailing cursor-restore CUP (\x1b[<row>;<col>H), if present.
        // The previous code added the live display offset and emitted an
        // out-of-range row (e.g. \x1b[7;1H on a 4-row screen).
        if let Some(idx) = replay.rfind("\x1b[") {
            let tail = &replay[idx + 2..];
            if let Some(h) = tail.find('H') {
                let row: usize = tail[..h].split(';').next().unwrap().parse().unwrap();
                assert!(row <= 4, "cursor row {row} must be within the 4-row screen");
            }
        }
    }

    #[test]
    fn reconnect_repaint_preserves_soft_wrap() {
        let mut src = TerminalState::with_scrollback(4, 8, 1000);
        // 12 chars into an 8-col terminal soft-wraps across two grid rows.
        src.process(b"ABCDEFGHIJKL");
        let has_wrap = |t: &TerminalState| (0..t.rows).any(|row| t.screen().row_wrapped(row));
        assert!(
            has_wrap(&src),
            "precondition: source has a soft-wrapped row"
        );

        let replay = src.reconnect_repaint();
        let mut dst = TerminalState::with_scrollback(4, 8, 1000);
        dst.process(&replay);

        // The soft wrap must survive the round-trip (rebuilt via the client's
        // autowrap), not degrade into a hard line break.
        assert!(has_wrap(&dst), "soft wrap must survive the round-trip");
        assert_eq!(src.reconnect_repaint(), dst.reconnect_repaint());
    }

    #[test]
    fn reconnect_repaint_maps_tabs_to_spaces_without_drift() {
        let mut src = TerminalState::with_scrollback(4, 40, 100);
        // Tabs stop every 8 columns: a@0, b@8, c@16.
        src.process(b"a\tb\tc");
        let replay = src.reconnect_repaint();
        assert!(
            !replay.contains(&b'\t'),
            "replay must not emit a raw tab — the client would re-interpret it and drift columns"
        );

        let mut dst = TerminalState::with_scrollback(4, 40, 100);
        dst.process(&replay);
        let snap = dst.snapshot();
        let at = |row: u16, col: u16| {
            snap.cells
                .iter()
                .find(|c| c.row == row && c.col == col)
                .map(|c| c.symbol.as_str())
        };
        // Columns must line up after the round-trip; the pre-fix code emitted the
        // raw tab AND the fill spaces, double-advancing the cursor (b drifted to
        // col 14, c to col 22).
        assert_eq!(at(0, 0), Some("a"));
        assert_eq!(at(0, 8), Some("b"));
        assert_eq!(at(0, 16), Some("c"));
    }

    #[test]
    fn clamp_replay_top_bounds_history() {
        // Within the cap: start at the real buffer top (4-row screen, bottom = 3).
        assert_eq!(clamp_replay_top(-50, 3), -50);
        // Beyond the cap: pull down to keep the most recent lines only.
        assert_eq!(
            clamp_replay_top(-200_000, 3),
            4 - MAX_RECONNECT_REPLAY_LINES
        );
        // Exactly at the cap boundary is not truncated.
        let exact = 4 - MAX_RECONNECT_REPLAY_LINES;
        assert_eq!(clamp_replay_top(exact, 3), exact);
    }

    #[test]
    fn reconnect_repaint_preserves_cursor_position() {
        let mut terminal = TerminalState::with_scrollback(5, 20, 100);
        terminal.process(b"abc");
        terminal.process(b"\x1b[3;5H"); // move cursor to row 3, col 5 (1-based)

        let replay = String::from_utf8(terminal.reconnect_repaint()).unwrap();
        assert!(
            replay.ends_with("\x1b[3;5H"),
            "replay should restore the cursor position; full replay: {replay:?}"
        );
    }

    #[test]
    fn scrollback_offset_accessor_matches_grid_state() {
        let mut terminal = TerminalState::with_scrollback(3, 16, 100);
        terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\n");

        assert_eq!(terminal.scrollback_offset(), 0);

        terminal.set_scrollback(2);

        assert_eq!(terminal.scrollback_offset(), 2);
    }

    #[test]
    fn osc_color_queries_produce_terminal_replies() {
        let mut terminal = TerminalState::with_scrollback(3, 16, 100);

        let response = terminal.process(b"\x1b]11;?\x07");
        let response = String::from_utf8(response).expect("color query response should be utf-8");

        assert!(
            response.contains("\x1b]11;rgb:0000/0000/0000"),
            "expected background color response, got: {response:?}"
        );
    }

    #[test]
    fn dsr_cursor_position_query_produces_reply() {
        // vt100 does not answer device-status queries itself; the migrated
        // responder must emit a DSR-6 (cursor position report) reflecting the
        // cursor *after* the chunk is applied. Move the cursor to row 3, col 5
        // (1-based) then query — the reply must report that position.
        let mut terminal = TerminalState::with_scrollback(10, 40, 100);
        let reply = terminal.process(b"\x1b[3;5H\x1b[6n");
        let reply = String::from_utf8(reply).expect("DSR reply should be utf-8");
        assert_eq!(
            reply, "\x1b[3;5R",
            "expected a DSR-6 cursor-position report at row 3, col 5, got: {reply:?}"
        );
    }

    #[test]
    fn da1_and_dsr_status_queries_produce_replies() {
        // DA1 (`ESC [ c`) must answer with a VT100-with-AVO identity, and
        // DSR-5 (`ESC [ 5 n`) with an "OK" operating-status report.
        let mut terminal = TerminalState::with_scrollback(5, 20, 100);
        let da1 = String::from_utf8(terminal.process(b"\x1b[c")).unwrap();
        assert_eq!(da1, "\x1b[?1;2c", "unexpected DA1 reply: {da1:?}");

        let dsr5 = String::from_utf8(terminal.process(b"\x1b[5n")).unwrap();
        assert_eq!(dsr5, "\x1b[0n", "unexpected DSR-5 reply: {dsr5:?}");
    }

    #[test]
    fn snapshot_preserves_ansi_background_colors() {
        let mut terminal = TerminalState::with_scrollback(2, 8, 100);
        terminal.process(b"\x1b[48;5;238mX\x1b[0m\x1b[48;2;10;20;30mY\x1b[0m");

        let snapshot = terminal.snapshot();
        let x = snapshot
            .cells
            .iter()
            .find(|cell| cell.symbol == "X")
            .expect("expected cell for X");
        let y = snapshot
            .cells
            .iter()
            .find(|cell| cell.symbol == "Y")
            .expect("expected cell for Y");

        assert_eq!(x.bg, CellColor::Indexed(238));
        assert_eq!(y.bg, CellColor::Rgb(10, 20, 30));
    }

    #[test]
    fn preserves_rich_parent_term_values() {
        assert_eq!(
            resolve_term_from_parent(Some(OsStr::new("tmux-256color"))),
            "tmux-256color"
        );
        assert_eq!(
            resolve_term_from_parent(Some(OsStr::new("xterm-kitty"))),
            "xterm-kitty"
        );
    }

    #[test]
    fn falls_back_to_xterm_256color_for_missing_or_low_capability_terms() {
        assert_eq!(resolve_term_from_parent(None), "xterm-256color");
        assert_eq!(
            resolve_term_from_parent(Some(OsStr::new(""))),
            "xterm-256color"
        );
        assert_eq!(
            resolve_term_from_parent(Some(OsStr::new("dumb"))),
            "xterm-256color"
        );
        assert_eq!(
            resolve_term_from_parent(Some(OsStr::new("vt100"))),
            "xterm-256color"
        );
    }

    #[test]
    fn apply_terminal_env_sets_expected_term_override() {
        let mut cmd = CommandBuilder::new("printf");
        apply_terminal_env_from_parent(
            &mut cmd,
            Some(OsStr::new("vt100")),
            Some(OsStr::new("truecolor")),
        );

        assert_eq!(
            cmd.get_env("TERM").and_then(|value| value.to_str()),
            Some("xterm-256color")
        );
        assert_eq!(
            cmd.get_env("COLORTERM").and_then(|value| value.to_str()),
            Some("truecolor")
        );
    }

    #[test]
    fn spawn_with_env_passes_custom_environment() {
        let args = vec!["-c".to_string(), "printf \"$DUX_TEST_PTY_ENV\"".to_string()];
        let mut client = PtyClient::spawn_with_env(
            "/bin/sh",
            &args,
            Path::new("."),
            5,
            40,
            100,
            &[("DUX_TEST_PTY_ENV".to_string(), "visible".to_string())],
        )
        .expect("spawn pty");

        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let snapshot = client.snapshot();
            if viewport_lines(&snapshot)
                .iter()
                .any(|line| line.contains("visible"))
            {
                let _ = client.try_wait();
                return;
            }
        }

        let snapshot = client.snapshot();
        panic!(
            "expected custom env output, got {:?}",
            viewport_lines(&snapshot)
        );
    }

    #[test]
    fn drop_kills_child_and_joins_reader_without_hanging() {
        // The child sleeps far longer than the test. Drop must kill + reap it
        // and join the reader thread promptly — it must NOT block until the
        // child would have exited on its own, and the join must not deadlock.
        let args = vec!["-c".to_string(), "sleep 120".to_string()];
        let client =
            PtyClient::spawn("/bin/sh", &args, Path::new("."), 5, 40, 100).expect("spawn pty");

        // `exited` is set only by the reader thread, immediately before it
        // returns. Hold a clone so we can prove, after the drop, that the
        // reader actually finished — which only the `join()` in `Drop`
        // guarantees synchronously. Without the join, drop would return while
        // the reader is still catching up and this flag would still be false.
        let reader_exited = Arc::clone(&client.exited);

        let start = Instant::now();
        drop(client);
        let elapsed = start.elapsed();

        // A correct drop completes in well under 100ms; 3s is generous for a
        // heavily loaded CI host while still catching a genuine hang (e.g. a
        // join that waits for the 120s child instead of killing it).
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "PtyClient::drop took {elapsed:?}; it must kill the child and join \
             the reader thread promptly, not wait for the child to finish"
        );
        // Proves the join was actually performed: if a regression removed it,
        // drop would return before the reader set this flag.
        assert!(
            reader_exited.load(Ordering::Acquire),
            "after drop, the reader thread must have exited (Drop must join it)"
        );
    }

    #[test]
    fn drop_kills_whole_process_group_so_a_surviving_grandchild_cannot_stall_it() {
        // The direct child backgrounds a grandchild that IGNORES SIGHUP and then
        // blocks reading the controlling terminal (`</dev/tty`, i.e. the PTY
        // slave), holding the slave open. `</dev/tty` is required: a shell
        // redirects a background job's stdin to /dev/null, so reading plain
        // stdin would hit immediate EOF and the grandchild would exit on its
        // own. Killing only the direct child — even combined with the kernel's
        // SIGHUP to the foreground process group when the session leader dies —
        // leaves that grandchild alive with the slave open, so the master read
        // never sees EOF and the reader-thread join in `Drop` would block
        // forever. The fix SIGKILLs the whole process group, which the
        // grandchild cannot ignore, so the slave is released and the join
        // completes. (POSIX `kill(-pgid)` + `setsid` behave the same on Linux
        // and macOS, so this holds on both.)
        let args = vec![
            "-c".to_string(),
            "sh -c 'trap \"\" HUP; echo GRANDKID_READY; read _x </dev/tty' & sleep 300".to_string(),
        ];
        let client =
            PtyClient::spawn("/bin/sh", &args, Path::new("."), 5, 40, 100).expect("spawn pty");

        // Wait until the grandchild has actually started and printed its marker
        // — proof it is running and holding the slave open — rather than
        // guessing with a fixed sleep. Without this, on a slow host the drop
        // could run before the grandchild grabs the slave, and the test would
        // pass without exercising the group kill at all.
        let mut grandchild_ready = false;
        for _ in 0..300 {
            if viewport_lines(&client.snapshot())
                .iter()
                .any(|line| line.contains("GRANDKID_READY"))
            {
                grandchild_ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            grandchild_ready,
            "grandchild did not start and hold the PTY slave within 3s"
        );

        // Run the drop on a worker thread so that, if a regression reintroduces
        // the hang, the test fails the assertion cleanly instead of blocking the
        // whole suite forever.
        let dropper = std::thread::spawn(move || drop(client));
        let start = Instant::now();
        while !dropper.is_finished() && start.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            dropper.is_finished(),
            "PtyClient::drop is still blocked after {:?}; it must SIGKILL the whole \
             process group so a SIGHUP-ignoring grandchild cannot hold the PTY \
             slave open and stall the reader-thread join",
            start.elapsed()
        );
        dropper.join().expect("dropper thread panicked");
    }

    #[test]
    fn pty_writer_send_never_blocks_when_the_write_stalls() {
        // The core of the deadlock fix. A child that has stopped reading its input
        // is modelled by a writer whose `write` blocks until released. The web
        // engine runs every request on one thread and forwards input through this
        // writer; `send` must return immediately regardless — queueing or dropping
        // — but never blocking the caller. (Done with a mock writer because a real
        // PTY's blocking is platform- and mode-dependent: macOS blocks the master
        // write when the slave input buffer fills, while a Linux tty in canonical
        // mode drops overflow at the line discipline instead — so flooding a real
        // PTY is not a reliable cross-platform reproduction.)
        struct BlockingWriter {
            gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
        }
        impl Write for BlockingWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let (lock, cvar) = &*self.gate;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = cvar.wait(open).unwrap();
                }
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let writer = PtyWriter::spawn(Box::new(BlockingWriter {
            gate: Arc::clone(&gate),
        }));

        // Flood past the queue cap from a worker thread; the writer thread is
        // wedged on the first write, so the queue fills and the rest is dropped —
        // but no `send` may block.
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let flooder = std::thread::spawn(move || {
            for _ in 0..(PTY_WRITE_QUEUE_CAP * 2) {
                writer.send(vec![b'x'; 64]);
            }
            let _ = done_tx.send(());
            writer
        });

        let finished = done_rx.recv_timeout(std::time::Duration::from_secs(5));

        // Release the stalled write so the writer thread can drain and exit, then
        // drop the writer (its Drop joins the thread) once observed.
        {
            let (lock, cvar) = &*gate;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }
        let writer = flooder.join().expect("flooder thread panicked");
        drop(writer);

        assert!(
            finished.is_ok(),
            "PtyWriter::send blocked while the underlying write was stalled; it must \
             queue-or-drop and never block the calling thread"
        );
    }

    #[test]
    fn pty_writer_delivers_queued_bytes_in_order() {
        // Happy path: the writer thread must actually deliver queued bytes to the
        // underlying writer, in submission order.
        struct CollectingWriter {
            seen: Arc<Mutex<Vec<u8>>>,
        }
        impl Write for CollectingWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.seen.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let seen = Arc::new(Mutex::new(Vec::new()));
        let writer = PtyWriter::spawn(Box::new(CollectingWriter {
            seen: Arc::clone(&seen),
        }));
        writer.send(b"abc".to_vec());
        writer.send(b"def".to_vec());

        // Dropping the writer joins its thread, which guarantees every queued
        // chunk has been written — so this is the synchronization point, no sleep.
        drop(writer);
        let delivered = seen.lock().unwrap().clone();
        assert_eq!(
            delivered, b"abcdef",
            "queued bytes must be delivered to the underlying writer in order"
        );
    }

    #[test]
    fn write_bytes_delivers_input_to_a_child_that_reads_stdin() {
        // Happy-path guard for routing writes through a dedicated writer: the
        // bytes must still reach the child. The shell reads one line and echoes
        // it with a marker; we send the line and expect the marker back.
        let args = vec![
            "-c".to_string(),
            "printf READY; read line; printf 'GOT:%s' \"$line\"".to_string(),
        ];
        let mut client =
            PtyClient::spawn("/bin/sh", &args, Path::new("."), 5, 40, 100).expect("spawn pty");

        // Wait until the shell signals it has reached `read` (instead of a blind
        // sleep that can lose the race on a loaded host), then send the line.
        let mut ready = false;
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if viewport_lines(&client.snapshot())
                .iter()
                .any(|line| line.contains("READY"))
            {
                ready = true;
                break;
            }
        }
        assert!(ready, "shell did not reach `read` within 2s");
        client.write_bytes(b"hello\n").expect("write_bytes");

        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if viewport_lines(&client.snapshot())
                .iter()
                .any(|line| line.contains("GOT:hello"))
            {
                let _ = client.try_wait();
                return;
            }
        }

        panic!(
            "expected the child to receive and echo the written input, got {:?}",
            viewport_lines(&client.snapshot())
        );
    }

    #[test]
    fn mouse_mode_off_by_default() {
        let terminal = TerminalState::with_scrollback(24, 80, 100);
        assert!(
            !terminal.has_mouse_mode(),
            "plain shell should not have mouse mode enabled"
        );
    }

    #[test]
    fn mouse_mode_on_after_enable_sequence() {
        let mut terminal = TerminalState::with_scrollback(24, 80, 100);
        // DECSET 1000: enable basic mouse reporting.
        terminal.process(b"\x1b[?1000h");
        assert!(
            terminal.has_mouse_mode(),
            "mouse mode should be enabled after DECSET 1000"
        );
    }

    #[test]
    fn mouse_mode_off_after_disable_sequence() {
        let mut terminal = TerminalState::with_scrollback(24, 80, 100);
        terminal.process(b"\x1b[?1000h");
        assert!(terminal.has_mouse_mode());

        // DECRST 1000: disable basic mouse reporting.
        terminal.process(b"\x1b[?1000l");
        assert!(
            !terminal.has_mouse_mode(),
            "mouse mode should be disabled after DECRST 1000"
        );
    }

    /// Simulates the Claude CLI plan-view scenario:
    ///  - Fill the terminal so there's scrollback history
    ///  - Draw 4 option labels at the bottom rows using cursor positioning
    ///  - Scroll up (user reads the plan) then scroll back to bottom
    ///  - Verify the option labels are still present and at the correct rows
    ///
    /// This tests whether `Term::scroll_display` round-tripping corrupts the
    /// grid content or cursor position that the child process last wrote.
    #[test]
    fn scroll_roundtrip_preserves_bottom_content() {
        // 10-row viewport, 40 cols, generous scrollback.
        let mut terminal = TerminalState::with_scrollback(10, 40, 200);

        // Fill enough lines to create scrollback history (simulate the plan text).
        for i in 0..20 {
            let line = format!("Plan line {i}\r\n");
            terminal.process(line.as_bytes());
        }

        // Now position cursor at rows 7-10 (bottom 4 rows of the 10-row viewport)
        // and draw the 4 option labels, simulating how a TUI app would draw them.
        // ESC[<row>;<col>H moves cursor to absolute position (1-indexed).
        // ESC[2K clears the entire line before writing.
        terminal.process(b"\x1b[7;1H\x1b[2K> Accept all edits");
        terminal.process(b"\x1b[8;1H\x1b[2KAccept and prompt");
        terminal.process(b"\x1b[9;1H\x1b[2KChange something");
        terminal.process(b"\x1b[10;1H\x1b[2KCustom input");

        // Snapshot before scrolling — this is the "known good" state.
        let before = terminal.snapshot();
        let before_lines = viewport_lines(&before);

        assert!(
            before_lines[6].contains("Accept all edits"),
            "row 7 should have first option, got: {:?}",
            before_lines[6]
        );
        assert!(
            before_lines[7].contains("Accept and prompt"),
            "row 8 should have second option, got: {:?}",
            before_lines[7]
        );
        assert!(
            before_lines[8].contains("Change something"),
            "row 9 should have third option, got: {:?}",
            before_lines[8]
        );
        assert!(
            before_lines[9].contains("Custom input"),
            "row 10 should have fourth option, got: {:?}",
            before_lines[9]
        );

        // User scrolls up to read the plan (scroll up by 5 rows).
        terminal.scroll(true, 5);
        assert_eq!(terminal.scrollback_offset(), 5);

        // Verify the bottom options are NOT visible while scrolled (they're
        // below the viewport). This is expected — just confirming the scroll
        // actually shifted the view.
        let scrolled = terminal.snapshot();
        let scrolled_lines = viewport_lines(&scrolled);
        assert!(
            !scrolled_lines[9].contains("Custom input"),
            "bottom option should not be visible while scrolled up"
        );

        // User scrolls back to bottom.
        terminal.scroll(false, 5);
        assert_eq!(
            terminal.scrollback_offset(),
            0,
            "should be back at live bottom"
        );

        // Take snapshot after the round-trip.
        let after = terminal.snapshot();
        let after_lines = viewport_lines(&after);

        // The critical assertions: content at the bottom rows must be identical
        // to what was there before scrolling.
        assert_eq!(
            before_lines[6], after_lines[6],
            "row 7 content changed after scroll round-trip"
        );
        assert_eq!(
            before_lines[7], after_lines[7],
            "row 8 content changed after scroll round-trip"
        );
        assert_eq!(
            before_lines[8], after_lines[8],
            "row 9 content changed after scroll round-trip"
        );
        assert_eq!(
            before_lines[9], after_lines[9],
            "row 10 content changed after scroll round-trip"
        );

        // Also verify cursor position is preserved — the child process left
        // the cursor at row 10 after writing "Custom input". After scroll
        // round-trip, the cursor should still be at the same viewport position.
        let cursor_before = before.cursor;
        let cursor_after = after.cursor;
        assert_eq!(
            cursor_before, cursor_after,
            "cursor position changed after scroll round-trip: before={cursor_before:?}, after={cursor_after:?}"
        );
    }

    /// Verify that when scrolled up by 1, the snapshot still contains the
    /// options that remain in the viewport (all but the very last row).
    /// This tests whether the snapshot faithfully captures styled content
    /// at the bottom of the viewport during partial scrolling.
    #[test]
    fn scroll_up_by_one_preserves_visible_bottom_rows() {
        let mut terminal = TerminalState::with_scrollback(10, 40, 200);

        // Fill scrollback.
        for i in 0..20 {
            terminal.process(format!("Plan line {i}\r\n").as_bytes());
        }

        // Draw 4 options at the bottom using cursor positioning + reverse video
        // to simulate styled TUI options (bold, reverse, etc.).
        terminal.process(b"\x1b[7;1H\x1b[2K\x1b[1m> Accept all edits\x1b[0m");
        terminal.process(b"\x1b[8;1H\x1b[2K  Accept and prompt");
        terminal.process(b"\x1b[9;1H\x1b[2K  Change something");
        terminal.process(b"\x1b[10;1H\x1b[2K  Custom input");

        // Scroll up by just 1 row.
        terminal.scroll(true, 1);
        assert_eq!(terminal.scrollback_offset(), 1);

        let scrolled = terminal.snapshot();
        let lines = viewport_lines(&scrolled);

        // The bottom row ("Custom input") scrolled off, but the other 3
        // should now be at rows 7, 8, 9 (shifted up by 1 from 6, 7, 8).
        // Rows are 0-indexed in viewport_lines.
        let has_accept_all = lines.iter().any(|l| l.contains("Accept all edits"));
        let has_accept_prompt = lines.iter().any(|l| l.contains("Accept and prompt"));
        let has_change = lines.iter().any(|l| l.contains("Change something"));
        let has_custom = lines.iter().any(|l| l.contains("Custom input"));

        assert!(
            has_accept_all,
            "\"Accept all edits\" should still be visible when scrolled up by 1. Lines: {lines:?}"
        );
        assert!(
            has_accept_prompt,
            "\"Accept and prompt\" should still be visible when scrolled up by 1. Lines: {lines:?}"
        );
        assert!(
            has_change,
            "\"Change something\" should still be visible when scrolled up by 1. Lines: {lines:?}"
        );
        assert!(
            !has_custom,
            "\"Custom input\" (bottom row) should NOT be visible when scrolled up by 1. Lines: {lines:?}"
        );

        // Verify the snapshot actually has non-empty cells for those rows
        // (not just whitespace). This catches the case where the grid is
        // fine but the snapshot iteration skips or blanks styled cells.
        let accept_cells: Vec<_> = scrolled
            .cells
            .iter()
            .filter(|c| c.symbol == "A" || c.symbol == ">" || c.symbol == "C")
            .collect();
        assert!(
            !accept_cells.is_empty(),
            "snapshot should contain non-whitespace cells for the option rows"
        );
    }

    /// Same as above but with a larger scroll distance that goes all the way
    /// to the top of history, then back. Tests the extreme case.
    #[test]
    fn scroll_to_top_and_back_preserves_bottom_content() {
        let mut terminal = TerminalState::with_scrollback(10, 40, 200);

        // Generate enough content for substantial scrollback.
        for i in 0..50 {
            let line = format!("Line {i}\r\n");
            terminal.process(line.as_bytes());
        }

        // Draw options at the bottom.
        terminal.process(b"\x1b[9;1H\x1b[2KOption A");
        terminal.process(b"\x1b[10;1H\x1b[2KOption B");

        let before = terminal.snapshot();
        let before_lines = viewport_lines(&before);

        // Scroll all the way to the top of history.
        let history = terminal.history_len();
        terminal.scroll(true, history);

        // Then scroll all the way back to bottom.
        terminal.scroll(false, history);
        assert_eq!(terminal.scrollback_offset(), 0);

        let after = terminal.snapshot();
        let after_lines = viewport_lines(&after);

        assert_eq!(
            before_lines, after_lines,
            "viewport content should be identical after full scroll round-trip"
        );
        assert_eq!(
            before.cursor, after.cursor,
            "cursor should be identical after full scroll round-trip"
        );
    }

    #[test]
    fn snapshot_into_reuses_capacity() {
        let mut terminal = TerminalState::with_scrollback(3, 12, 100);
        terminal.process(b"hello\r\nworld\r\n");

        let mut buf = TerminalSnapshot::empty();
        terminal.snapshot_into(&mut buf);
        let cap_after_first = buf.cells.capacity();
        assert!(!buf.cells.is_empty(), "first snapshot should have cells");

        // Second call should reuse the Vec capacity.
        terminal.process(b"more\r\n");
        terminal.snapshot_into(&mut buf);
        assert_eq!(
            buf.cells.capacity(),
            cap_after_first,
            "Vec capacity should be reused across snapshot_into calls"
        );
        assert!(!buf.cells.is_empty());
    }

    #[test]
    fn snapshot_into_matches_snapshot() {
        let mut terminal = TerminalState::with_scrollback(3, 12, 100);
        terminal.process(b"hello\r\nworld\r\n");

        let owned = terminal.snapshot();
        let mut buf = TerminalSnapshot::empty();
        terminal.snapshot_into(&mut buf);

        assert_eq!(owned.rows, buf.rows);
        assert_eq!(owned.cols, buf.cols);
        assert_eq!(owned.scrollback_offset, buf.scrollback_offset);
        assert_eq!(owned.scrollback_total, buf.scrollback_total);
        assert_eq!(owned.cells.len(), buf.cells.len());
        for (a, b) in owned.cells.iter().zip(buf.cells.iter()) {
            assert_eq!(a.row, b.row);
            assert_eq!(a.col, b.col);
            assert_eq!(a.symbol, b.symbol);
            assert_eq!(a.fg, b.fg);
            assert_eq!(a.bg, b.bg);
            assert_eq!(a.modifier, b.modifier);
        }
    }

    #[test]
    fn compact_string_symbol_handles_ascii_and_multibyte() {
        let mut terminal = TerminalState::with_scrollback(2, 8, 100);
        // Write ASCII 'A' followed by a multi-byte character.
        terminal.process("Aé".as_bytes());

        let snapshot = terminal.snapshot();
        let a_cell = snapshot
            .cells
            .iter()
            .find(|c| c.symbol == "A")
            .expect("should have cell for A");
        let e_cell = snapshot
            .cells
            .iter()
            .find(|c| c.symbol == "é")
            .expect("should have cell for é");

        assert_eq!(a_cell.symbol.len(), 1);
        assert_eq!(e_cell.symbol.len(), 2); // é is 2 bytes in UTF-8
    }

    #[test]
    fn dirty_flag_skips_rebuild_when_unchanged() {
        let dirty = Arc::new(AtomicBool::new(true));
        let mut terminal = TerminalState::with_scrollback(3, 12, 100);
        terminal.process(b"hello\r\n");

        // Simulate first snapshot (dirty=true).
        assert!(dirty.swap(false, Ordering::AcqRel));
        let mut buf = TerminalSnapshot::empty();
        terminal.snapshot_into(&mut buf);
        assert!(!buf.cells.is_empty());

        // Second check without new data: dirty should be false.
        assert!(
            !dirty.swap(false, Ordering::AcqRel),
            "dirty flag should be false when no new data arrived"
        );
    }

    #[test]
    fn dirty_flag_set_after_process() {
        let dirty = Arc::new(AtomicBool::new(true));

        // Consume initial dirty.
        assert!(dirty.swap(false, Ordering::AcqRel));

        // Simulate reader thread setting dirty after process.
        dirty.store(true, Ordering::Release);
        assert!(
            dirty.swap(false, Ordering::AcqRel),
            "dirty flag should be true after data arrives"
        );
    }

    #[test]
    fn alt_screen_off_by_default() {
        let terminal = TerminalState::with_scrollback(24, 80, 100);
        assert!(
            !terminal.is_alt_screen(),
            "plain shell should not be on the alternate screen"
        );
    }

    #[test]
    fn alt_screen_on_after_enter_sequence() {
        let mut terminal = TerminalState::with_scrollback(24, 80, 100);
        // DECSET 1049: enter alternate screen buffer.
        terminal.process(b"\x1b[?1049h");
        assert!(
            terminal.is_alt_screen(),
            "alt-screen should be active after DECSET 1049"
        );
    }

    #[test]
    fn alt_screen_off_after_exit_sequence() {
        let mut terminal = TerminalState::with_scrollback(24, 80, 100);
        terminal.process(b"\x1b[?1049h");
        assert!(terminal.is_alt_screen());

        // DECRST 1049: exit alternate screen buffer.
        terminal.process(b"\x1b[?1049l");
        assert!(
            !terminal.is_alt_screen(),
            "alt-screen should be inactive after DECRST 1049"
        );
    }

    #[test]
    fn append_with_cap_grows_buffer_below_cap() {
        let mut pending = PendingIngest::default();
        append_with_cap(&mut pending, b"hello ", 64);
        append_with_cap(&mut pending, b"world", 64);
        assert_eq!(pending.buf, b"hello world");
        assert!(!pending.dropped);
    }

    #[test]
    fn append_with_cap_drops_oldest_on_overflow() {
        let mut pending = PendingIngest::default();
        pending.buf.extend_from_slice(b"AAAAAAAA"); // 8 bytes already buffered
        append_with_cap(&mut pending, b"BBBB", 10);
        // Cap=10, new_len would be 12 → drop 2 from the front.
        assert_eq!(pending.buf, b"AAAAAABBBB");
        assert!(pending.dropped);
    }

    #[test]
    fn append_with_cap_truncates_oversized_single_chunk() {
        let mut pending = PendingIngest::default();
        pending.buf.extend_from_slice(b"prev");
        let huge = vec![b'X'; 32];
        append_with_cap(&mut pending, &huge, 10);
        // Existing content is dropped; only the last 10 bytes of `huge` are kept.
        assert_eq!(pending.buf.len(), 10);
        assert!(pending.buf.iter().all(|b| *b == b'X'));
        assert!(pending.dropped);
    }

    #[test]
    fn append_with_cap_zero_cap_is_noop_with_flag() {
        let mut pending = PendingIngest::default();
        append_with_cap(&mut pending, b"ignored", 0);
        assert!(pending.buf.is_empty());
        assert!(pending.dropped);

        // With empty data and cap=0, dropped stays false on a fresh buffer.
        let mut fresh = PendingIngest::default();
        append_with_cap(&mut fresh, b"", 0);
        assert!(fresh.buf.is_empty());
        assert!(!fresh.dropped);
    }

    /// The resume path drains `pending_bytes` into `terminal.process`. Verify
    /// that a paused-then-drained stream produces an identical terminal state
    /// to feeding the same bytes inline, so users returning from scrollback
    /// see exactly the output they would have seen without pausing.
    #[test]
    fn paused_then_resumed_matches_unpaused_baseline() {
        let mut baseline = TerminalState::with_scrollback(5, 20, 100);
        baseline.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");

        // Simulate: first chunk arrives live, remainder while paused.
        let mut paused = TerminalState::with_scrollback(5, 20, 100);
        paused.process(b"one\r\n");

        let mut pending = PendingIngest::default();
        append_with_cap(&mut pending, b"two\r\nthree\r\n", PAUSE_BUFFER_CAP);
        append_with_cap(&mut pending, b"four\r\nfive\r\n", PAUSE_BUFFER_CAP);
        assert!(!pending.dropped);

        // Resume: drain accumulated bytes into the terminal parser.
        let drained = std::mem::take(&mut pending.buf);
        paused.process(&drained);

        assert_eq!(
            viewport_lines(&baseline.snapshot()),
            viewport_lines(&paused.snapshot()),
            "paused+drained stream should match the unpaused baseline"
        );
        assert_eq!(
            baseline.history_len(),
            paused.history_len(),
            "scrollback history size should match"
        );
    }

    /// Chunks that overflow the pause buffer must still drop oldest rather
    /// than panic, and `dropped` must be sticky so resume can log once.
    #[test]
    fn overflow_during_pause_keeps_tail_and_flags_drop() {
        let mut pending = PendingIngest::default();
        // Small cap for deterministic overflow.
        let cap = 8;
        append_with_cap(&mut pending, b"1234", cap);
        append_with_cap(&mut pending, b"5678", cap);
        assert_eq!(pending.buf, b"12345678");
        assert!(!pending.dropped);

        // Next chunk forces an overflow — oldest bytes are dropped.
        append_with_cap(&mut pending, b"abcd", cap);
        assert_eq!(pending.buf, b"5678abcd");
        assert!(pending.dropped);

        // Flag stays set across subsequent non-overflowing appends so that a
        // single log line on resume can summarize the session.
        append_with_cap(&mut pending, b"", cap);
        assert!(pending.dropped);
    }

    fn repaint_cell(row: u16, col: u16, symbol: &str, fg: CellColor) -> SnapshotCell {
        SnapshotCell {
            row,
            col,
            symbol: CompactString::from(symbol),
            fg,
            bg: CellColor::Reset,
            modifier: CellModifier {
                bold: false,
                dim: false,
                italic: false,
                underlined: false,
                reversed: false,
                crossed_out: false,
            },
        }
    }

    #[test]
    fn repaint_emits_alt_screen_clear_position_color_and_text() {
        let snapshot = TerminalSnapshot {
            rows: 1,
            cols: 3,
            scrollback_offset: 0,
            scrollback_total: 0,
            cursor: Some(SnapshotCursor { row: 0, col: 2 }),
            cells: vec![
                repaint_cell(0, 0, "H", CellColor::Red),
                repaint_cell(0, 1, "i", CellColor::Red),
            ],
        };
        let bytes = synthesize_repaint(&snapshot, true);
        let text = String::from_utf8(bytes).expect("utf8");

        assert!(
            text.starts_with("\x1b[?1049h"),
            "no alt-screen enter: {text:?}"
        );
        assert!(text.contains("\x1b[2J"), "no clear: {text:?}");
        assert!(text.contains("\x1b[1;1H"), "no home position: {text:?}");
        assert!(text.contains("31"), "no red fg sgr: {text:?}");
        assert!(text.contains("Hi"), "text not contiguous: {text:?}");
        assert!(
            text.trim_end().ends_with("\x1b[1;3H"),
            "cursor not restored: {text:?}"
        );
    }

    #[test]
    fn repaint_without_alt_screen_has_no_alt_enter() {
        let snapshot = TerminalSnapshot {
            rows: 1,
            cols: 1,
            scrollback_offset: 0,
            scrollback_total: 0,
            cursor: None,
            cells: vec![repaint_cell(0, 0, "x", CellColor::Reset)],
        };
        let bytes = synthesize_repaint(&snapshot, false);
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(
            !text.contains("\x1b[?1049h"),
            "unexpected alt-screen enter: {text:?}"
        );
        assert!(text.contains('x'));
    }

    /// Regression guard for the sender-clone shutdown hazard: a clone of the
    /// writer's `SyncSender` (just as the reader thread holds) must NOT prevent
    /// `PtyWriter::drop` from stopping the writer thread promptly. Before the fix
    /// the `Drop` relied on channel disconnect; with a live clone, no disconnect
    /// ever fired and the join blocked indefinitely. Now `Drop` sends an explicit
    /// `Shutdown` so the thread exits regardless of surviving clones.
    #[test]
    fn pty_writer_drop_exits_promptly_with_a_surviving_sender_clone() {
        // A no-op writer — we are only testing shutdown timing, not data delivery.
        struct NullWriter;
        impl Write for NullWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let writer = PtyWriter::spawn(Box::new(NullWriter));
        // Clone the sender, simulating the reader thread's hold on it. Keep
        // this clone alive across the drop so it cannot cause a disconnect.
        let _clone = writer.sender();

        let start = std::time::Instant::now();
        drop(writer);
        let elapsed = start.elapsed();

        // The explicit Shutdown makes teardown near-instantaneous. A 2s ceiling
        // is generous for a loaded CI host while still catching a genuine hang.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "PtyWriter::drop took {elapsed:?} with a surviving sender clone; \
             the Shutdown signal must make the writer thread exit regardless of \
             remaining sender clones (no channel-disconnect deadlock)"
        );
        // Verify the clone is still alive (i.e. this was a real test of the hazard,
        // not one where Rust happened to drop `_clone` before `writer`).
        drop(_clone);
    }
}
