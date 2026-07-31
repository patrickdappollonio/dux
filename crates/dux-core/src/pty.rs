//! PTY-based terminal client plus the surface-agnostic cell types for its
//! terminal-grid snapshot. The client spawns a CLI in a pseudo-terminal and
//! keeps a full terminal grid (via `alacritty_terminal`); the snapshot's
//! `CellColor`/`CellModifier` let each surface convert to its own medium (the
//! TUI to `ratatui` types; the web to CSS) at its render boundary.

use std::collections::VecDeque;
use std::env;
use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, GridCell, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{self, Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as TermColor, CursorShape, NamedColor, Processor, Rgb, StdSyncHandler,
};
use anyhow::{Context, Result};
use compact_str::CompactString;
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

use crate::logger;
use crate::scroll_margins::{ScrollRegion, ScrollRegionTracker};

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
    /// Index into [`TerminalSnapshot::links`] when this cell is part of an OSC 8
    /// hyperlink, else `None`. The TUI wraps the cell symbol in an OSC 8 open/close
    /// pair so a host terminal that supports hyperlinks renders it clickable.
    pub link: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct TerminalSnapshot {
    pub rows: u16,
    pub cols: u16,
    pub scrollback_offset: usize,
    pub scrollback_total: usize,
    pub cursor: Option<SnapshotCursor>,
    pub cells: Vec<SnapshotCell>,
    /// Interned OSC 8 hyperlink URIs referenced by `cells[..].link`. A per-snapshot
    /// table so a link spanning many cells stores its URI once.
    pub links: Vec<String>,
}

/// Maximum number of distinct OSC 8 hyperlink URIs interned in a single snapshot.
/// A real terminal frame references only a handful; this bounds the per-frame link
/// table so an agent that emits a flood of unique links cannot balloon it. Cells
/// beyond the cap render as plain text (`link = None`).
const MAX_SNAPSHOT_LINKS: usize = 256;

/// Whether an OSC 8 hyperlink URI is safe to forward to the host terminal: an
/// `http://` or `https://` scheme (case-insensitive) with no control bytes. This
/// mirrors the web link handler's gate so both surfaces treat the same links as
/// clickable; a `file://`, `javascript:`, or control-laced URI is treated as plain
/// text on both.
fn is_forwardable_link_uri(uri: &str) -> bool {
    if uri.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return false;
    }
    let lower = uri.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
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
            links: Vec::new(),
        }
    }
}

/// Build an ANSI byte sequence that repaints `snapshot` onto a freshly-connected
/// client's terminal. If `alt_screen` is set, switch the client into the
/// alternate-screen buffer first so full-screen apps (vim, claude) render
/// correctly. Reflects the visible screen only (no scrollback replay).
///
/// `scroll_region` is the child's scrolling region, restored at the end. The
/// ordering is load bearing and pinned by its own test: the painting addresses
/// cells absolutely, so it has to run with the whole screen scrolling or every
/// cell lands somewhere else, and setting a region homes the cursor, so the
/// cursor can only be placed once the region is already in place. Hence the whole
/// screen up front, cells, region, cursor.
pub fn synthesize_repaint(
    snapshot: &TerminalSnapshot,
    alt_screen: bool,
    scroll_region: ScrollRegion,
) -> Vec<u8> {
    let mut out = String::new();
    if alt_screen {
        out.push_str("\x1b[?1049h");
    }
    // Widen to the whole screen and turn origin mode off before painting. A
    // reconnecting client has reset and already has both, but a client arriving
    // from another PTY carries whatever that one had, and either a leftover region
    // or a leftover origin mode would misplace every cell this paints.
    out.push_str("\x1b[?6l");
    out.push_str("\x1b[r");
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
    out.push_str(&scroll_region.decstbm_sequence());
    if let Some(cursor) = &snapshot.cursor {
        out.push_str(&format!("\x1b[{};{}H", cursor.row + 1, cursor.col + 1));
    }
    out.into_bytes()
}

/// DECSET/DECRST sequences that re-assert the child's private terminal MODES on
/// a client that has just (re)connected.
///
/// A repaint rebuilds CELLS. Modes are not cells: the child enabled them once,
/// at its own startup, and never emits them again. A reconnecting web client
/// resets its terminal before applying the replay, so without this block it comes
/// back with a correct-looking screen and default modes. The visible symptom was
/// touch scrolling over a full-screen agent: the web pane forwards a finger drag
/// as SGR wheel reports only while the app has mouse tracking on, so a lost mode
/// left a drag with nowhere to go and the gesture did nothing at all.
///
/// Both polarities are always emitted so the result is a full assignment rather
/// than a set of deltas against an assumed-default client. That makes the block
/// correct for a client that was NOT freshly reset (a client switching between
/// PTYs, say) at a cost of well under 100 bytes on a frame that can carry a
/// hundred thousand replayed lines.
///
/// Both the private (DEC) modes and the two ANSI modes the engine tracks are
/// covered. The ANSI pair is spelled WITHOUT the `?`: insert mode is IRM,
/// `CSI 4 h`, and line-feed/new-line mode is LNM, `CSI 20 h`. The private `?4` is
/// a different setting entirely (DECSCLM, smooth scrolling) that the engine does
/// not track at all, so do not reach for it here. Insert mode is the one whose
/// loss is immediately visible: a program sitting in it comes back with the
/// client OVERWRITING at the cursor where the program expects each character to
/// push the rest of the line right.
///
/// Deliberately absent: origin mode (`?6`). Origin mode
/// makes every coordinate relative to the scrolling region's top margin, so it is
/// not a flag the repaint can simply assert alongside the others: the repaint
/// paints with absolute addressing, and the one coordinate that outlives the
/// painting, the final cursor position, would have to be translated into
/// region-relative space to survive it. The repaint does no such translation, so
/// it does not restore origin mode. It does CLEAR it, up front, in the same place
/// it widens the scrolling region, which is a different thing and a necessary
/// one: the repaint now sets a region before it places the cursor, so a client
/// that arrived with the flag already on would take that final position relative
/// to the top margin and land the cursor a whole margin too low. Clearing it
/// costs one sequence and makes the frame's own absolute positioning true
/// regardless of what the client was carrying. The margins themselves ARE
/// restored (see [`ScrollRegion::decstbm_sequence`]), so a program that scrolls a
/// pinned region keeps its region across a reconnect even though it does not keep
/// this flag.
fn mode_restore_sequence(mode: TermMode) -> String {
    let mut out = String::with_capacity(96);
    let mut set = |code: u16, on: bool| {
        out.push_str("\x1b[?");
        out.push_str(&code.to_string());
        out.push(if on { 'h' } else { 'l' });
    };
    set(1, mode.contains(TermMode::APP_CURSOR));
    set(7, mode.contains(TermMode::LINE_WRAP));
    set(25, mode.contains(TermMode::SHOW_CURSOR));
    // The three mouse-TRACKING modes are one escalating setting on the receiving
    // end, not three independent flags: xterm.js keeps a single active protocol
    // and a DECRST of ANY of 1000/1002/1003 drops it to none, so emitting the
    // disables after the enable would silently undo the enable (measured: a
    // `1000l 1002h 1003l` block leaves `mouseTrackingMode === "none"`). Emit
    // every unset one first and the set one(s) last, ascending, so the most
    // capable tracking mode is what lands.
    //
    // On OUR side at most one of the three is ever set. alacritty_terminal makes
    // the protocols mutually exclusive the same way xterm.js does: setting any of
    // 1000/1002/1003 clears `MOUSE_MODE` (all three bits) before inserting the one
    // asked for, so `mode` can never carry two at once. The second loop therefore
    // emits at most one enable in practice; it is written to handle several
    // because that costs nothing and keeps the ordering rule true whatever the
    // flags come from. When the child has none set, all three disables go out,
    // which is the full assignment a client re-used from another PTY needs,
    // whichever one of them it happens to be carrying.
    let tracking = [
        (1000u16, TermMode::MOUSE_REPORT_CLICK),
        (1002, TermMode::MOUSE_DRAG),
        (1003, TermMode::MOUSE_MOTION),
    ];
    for (code, flag) in tracking {
        if !mode.contains(flag) {
            set(code, false);
        }
    }
    for (code, flag) in tracking {
        if mode.contains(flag) {
            set(code, true);
        }
    }
    set(1004, mode.contains(TermMode::FOCUS_IN_OUT));
    // The mouse ENCODING modes are independent of the protocol above and of each
    // other, so plain both-polarity assignment is correct here.
    set(1005, mode.contains(TermMode::UTF8_MOUSE));
    set(1006, mode.contains(TermMode::SGR_MOUSE));
    set(1007, mode.contains(TermMode::ALTERNATE_SCROLL));
    set(2004, mode.contains(TermMode::BRACKETED_PASTE));
    // The two ANSI (non-private) modes, so no `?`. These go out AFTER the block
    // above and, at both call sites, after the repaint has finished painting:
    // asserting insert mode before the cells would make the client push the
    // replay's own output sideways as it lands.
    let mut ansi_set = |code: u16, on: bool| {
        out.push_str("\x1b[");
        out.push_str(&code.to_string());
        out.push(if on { 'h' } else { 'l' });
    };
    ansi_set(4, mode.contains(TermMode::INSERT));
    ansi_set(20, mode.contains(TermMode::LINE_FEED_NEW_LINE));
    // Application keypad has no DECSET form; it is the two-byte DECKPAM/DECKPNM.
    out.push_str(if mode.contains(TermMode::APP_KEYPAD) {
        "\x1b="
    } else {
        "\x1b>"
    });
    out
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

/// Shared subscriber list: id-tagged senders fanned out by the PTY reader loop.
type SubscriberList = Arc<Mutex<Vec<(u64, std::sync::mpsc::Sender<Vec<u8>>)>>>;

/// Hand one chunk of raw PTY output to every live subscriber, pruning senders
/// whose receiver has hung up. Cheap no-op when there are none.
///
/// The caller MUST already hold the terminal lock: the fan-out and the ingest of
/// the same chunk are one atomic step with respect to
/// [`PtyClient::subscribe_with_repaint`], which holds that same lock while it
/// registers a subscriber and snapshots the grid. See the comment at the reader's
/// call site for why.
fn fan_out_to_subscribers(subscribers: &SubscriberList, data: &[u8]) {
    if let Ok(mut subs) = subscribers.lock()
        && !subs.is_empty()
    {
        subs.retain(|(_, tx)| tx.send(data.to_vec()).is_ok());
    }
}

/// RAII guard returned by [`PtyClient::subscribe`] and
/// [`PtyClient::subscribe_with_repaint`]. Dropping it immediately removes the
/// subscriber from the fan-out list without waiting for the next PTY output.
pub struct PtyViewerGuard {
    id: u64,
    subs: SubscriberList,
}

impl Drop for PtyViewerGuard {
    fn drop(&mut self) {
        if let Ok(mut subs) = self.subs.lock() {
            subs.retain(|(id, _)| *id != self.id);
        }
    }
}

/// A PTY-based client that spawns a CLI tool in a pseudo-terminal and keeps a
/// full terminal grid with scrollback using `alacritty_terminal`.
pub struct PtyClient {
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    /// Dedicated writer thread for the PTY master, fed over a bounded queue so a
    /// child that stops reading its input can never block the engine thread.
    writer: PtyWriter,
    terminal: Arc<Mutex<TerminalState>>,
    child: Box<dyn Child + Send + Sync>,
    /// The child's exit status the first time [`PtyClient::try_wait`] observed
    /// it, plus when that reap happened. `Child::try_wait` yields the status
    /// EXACTLY ONCE (the second call sees no zombie and returns `None`), so
    /// without this cache the first caller to poll consumes the status out from
    /// under every later one. The reap instant is what lets callers tell
    /// "the child just died" from "the child died a while ago and this PTY's
    /// read side is still being held open by something else".
    reaped: Option<(portable_pty::ExitStatus, Instant)>,
    exited: Arc<AtomicBool>,
    /// When the reader thread reached end of input, written once immediately
    /// BEFORE it sets `exited`, so anyone who observes `exited` (an `Acquire`
    /// load paired with the `Release` store below) also sees this instant. It is
    /// what bounds a wait that is keyed on EOF: `reaped` cannot bound one,
    /// because a child that closes its descriptors and keeps running reaches EOF
    /// and is never reaped at all.
    exited_at: Arc<OnceLock<Instant>>,
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
    /// Live raw-byte subscribers (web clients). Each receives a clone of every
    /// chunk read from the PTY. Each entry
    /// is tagged with a stable id so the RAII [`PtyViewerGuard`] can remove its
    /// own slot on drop without waiting for the next PTY output. The reader loop
    /// also prunes hung-up senders reactively as a backstop.
    subscribers: SubscriberList,
    /// Monotonically increasing counter used to assign unique ids to subscribers.
    next_sub_id: AtomicU64,
    /// Handle to the background reader thread. Joined in `Drop` (after the
    /// child is killed and reaped) so the thread does not outlive the client.
    reader_thread: Option<thread::JoinHandle<()>>,
    /// Consuming flag set by the reader loop's raw-byte scanner when it sees a
    /// bare terminal bell (`0x07` outside any escape sequence). Drained by
    /// [`PtyClient::take_attention`]. Never set for companion terminals, which
    /// spawn with signal tracking off.
    attention_bell: Arc<AtomicBool>,
    /// Consuming flag set by the reader loop's raw-byte scanner when an `OSC 9` /
    /// `OSC 777` notification is seen. Drained by [`PtyClient::take_attention`].
    attention_notify: Arc<AtomicBool>,
    /// The most recent `OSC 9;4` progress report, or `None` if the agent has not
    /// emitted one. Read (not consumed) by the engine's working predicate; the
    /// engine applies its own staleness window.
    progress: Arc<Mutex<Option<ProgressReport>>>,
    /// Bounded ring of whitelisted passthrough sequences the reader loop captured
    /// (notifications, progress, clipboard SETs, kitty notifications) awaiting
    /// forwarding to the host terminal. Drained by [`PtyClient::take_passthrough`].
    /// Capped ([`PASSTHROUGH_RING_CAP`] entries, [`PASSTHROUGH_SEQ_MAX`] bytes each)
    /// so a headless server that never drains, or an agent that floods sequences,
    /// cannot grow it without bound; the oldest entries are dropped. Empty for
    /// companion terminals (signal tracking off).
    passthrough: Arc<Mutex<VecDeque<crate::attention::CapturedSeq>>>,
}

/// Maximum number of captured passthrough sequences retained before the oldest is
/// dropped. Small: the host is expected to drain every tick, so this only bounds a
/// burst or a never-draining headless server.
const PASSTHROUGH_RING_CAP: usize = 64;
/// Maximum size of a single captured passthrough sequence. A larger one (a huge
/// OSC 52 clipboard payload, say) is dropped rather than buffered.
const PASSTHROUGH_SEQ_MAX: usize = 8 * 1024;

/// The most recent `OSC 9;4` progress report an agent emitted, with the moment
/// it arrived. The engine reads this to drive a truer "working" indicator: while
/// the report is fresh it overrides the output-activity heuristic, and it goes
/// stale (falling back to the heuristic) if no newer report arrives. `working`
/// mirrors [`crate::attention::AttentionEvent::Progress`].
#[derive(Debug, Clone, Copy)]
pub struct ProgressReport {
    pub working: bool,
    pub at: Instant,
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

    /// Spawn with an explicit environment. Agent signal tracking (the OSC / bell
    /// [`crate::attention::AttentionScanner`]) is ON: this is the path agent tabs
    /// use. Companion terminals want it off and go through
    /// [`PtyClient::spawn_with_env_opts`]. No terminal-identity mutation is applied
    /// (the caller env is the only override).
    pub fn spawn_with_env(
        command: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
        scrollback_lines: usize,
        env: &[(String, String)],
    ) -> Result<Self> {
        Self::spawn_with_env_opts(
            command,
            args,
            cwd,
            rows,
            cols,
            scrollback_lines,
            PtySpawnOptions {
                env,
                track_agent_signals: true,
                identity: &crate::term_identity::TerminalIdentity::default(),
            },
        )
    }

    /// Spawn with an explicit environment and control over agent signal tracking
    /// plus the terminal-identity mutation. When `opts.track_agent_signals` is
    /// false the reader loop skips the attention scanner entirely, so
    /// `take_attention` stays `false` and `progress_report` stays `None` for the
    /// life of the client. Companion terminals pass `false`: they are plain shells,
    /// not agents, so scanning their every byte for OSC / bell signals only burns
    /// cycles and could raise spurious attention.
    pub fn spawn_with_env_opts(
        command: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
        scrollback_lines: usize,
        opts: PtySpawnOptions<'_>,
    ) -> Result<Self> {
        let PtySpawnOptions {
            env,
            track_agent_signals,
            identity,
        } = opts;
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
        // Apply the resolved terminal identity between the baseline TERM/COLORTERM
        // and the caller's `[env]`: remove first (a trailing `*` scrubs a whole
        // prefix family against the real environment), then set, so the user's own
        // `[env]` overrides below always win.
        apply_identity_env(&mut cmd, identity);
        for (name, value) in env {
            cmd.env(name, value);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn '{command}' in PTY"))?;

        // Drop slave so reads on master get EOF when child exits.
        drop(pair.slave);

        // A child is already forked at this point. If the reader/writer setup
        // fails we must reap it before returning `Err`, or a live orphaned
        // process leaks with no `PtyClient` (and no `providers` entry) to track
        // or terminate it — the tab-create failure cleanup relies on a spawn
        // `Err` meaning "no live process".
        let reader = match pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")
        {
            Ok(reader) => reader,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        };
        let pty_writer = match pair
            .master
            .take_writer()
            .context("failed to take PTY writer")
        {
            Ok(writer) => writer,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        };

        let terminal_state = TerminalState::new(rows, cols, scrollback_lines);
        let terminal = Arc::new(Mutex::new(terminal_state));
        // The bell flag is driven by the raw-byte scanner in the reader loop (the
        // single bell-detection path), not by the terminal emulator, so it is an
        // independent flag like `attention_notify`.
        let attention_bell = Arc::new(AtomicBool::new(false));
        let writer = PtyWriter::spawn(pty_writer);
        let writer_tx = writer.sender();
        let exited = Arc::new(AtomicBool::new(false));
        let exited_at: Arc<OnceLock<Instant>> = Arc::new(OnceLock::new());
        let has_output = Arc::new(AtomicBool::new(false));
        let dirty = Arc::new(AtomicBool::new(true));
        let received_data = Arc::new(AtomicBool::new(false));
        let subscribers: SubscriberList = Arc::new(Mutex::new(Vec::new()));
        let attention_notify = Arc::new(AtomicBool::new(false));
        let progress: Arc<Mutex<Option<ProgressReport>>> = Arc::new(Mutex::new(None));
        let passthrough: Arc<Mutex<VecDeque<crate::attention::CapturedSeq>>> =
            Arc::new(Mutex::new(VecDeque::new()));

        let terminal_ref = Arc::clone(&terminal);
        let exited_ref = Arc::clone(&exited);
        let exited_at_ref = Arc::clone(&exited_at);
        let has_output_ref = Arc::clone(&has_output);
        let dirty_ref = Arc::clone(&dirty);
        let received_data_ref = Arc::clone(&received_data);
        let subscribers_ref = Arc::clone(&subscribers);
        let attention_bell_ref = Arc::clone(&attention_bell);
        let attention_notify_ref = Arc::clone(&attention_notify);
        let progress_ref = Arc::clone(&progress);
        let passthrough_ref = Arc::clone(&passthrough);
        let reader_thread = thread::spawn(move || {
            Self::reader_loop(
                reader,
                terminal_ref,
                writer_tx,
                exited_ref,
                exited_at_ref,
                has_output_ref,
                dirty_ref,
                received_data_ref,
                subscribers_ref,
                attention_bell_ref,
                attention_notify_ref,
                progress_ref,
                passthrough_ref,
                track_agent_signals,
            );
        });

        Ok(Self {
            master: pair.master,
            writer,
            terminal,
            child,
            reaped: None,
            exited,
            exited_at,
            has_output,
            dirty,
            received_data,
            last_resize_at: Mutex::new(None),
            subscribers,
            next_sub_id: AtomicU64::new(0),
            reader_thread: Some(reader_thread),
            attention_bell,
            attention_notify,
            progress,
            passthrough,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn reader_loop(
        mut reader: Box<dyn std::io::Read + Send>,
        terminal: Arc<Mutex<TerminalState>>,
        writer_tx: std::sync::mpsc::SyncSender<PtyWriteMsg>,
        exited: Arc<AtomicBool>,
        exited_at: Arc<OnceLock<Instant>>,
        has_output: Arc<AtomicBool>,
        dirty: Arc<AtomicBool>,
        received_data: Arc<AtomicBool>,
        subscribers: SubscriberList,
        attention_bell: Arc<AtomicBool>,
        attention_notify: Arc<AtomicBool>,
        progress: Arc<Mutex<Option<ProgressReport>>>,
        passthrough: Arc<Mutex<VecDeque<crate::attention::CapturedSeq>>>,
        track_agent_signals: bool,
    ) {
        let mut buf = [0u8; 4096];
        // Raw-byte scanner for bell / OSC notifications and progress reports. Lives
        // on the reader thread's stack so its cross-chunk carry persists between
        // reads. It is the ONLY detection path for the notification and progress
        // sequences, because the emulator does not report them: alacritty's
        // `Event` carries a bell, a title/icon change, a clipboard store, a colour
        // request and a PTY write, and nothing else, so an `OSC 9` notification or
        // an `OSC 9;4` progress report reaching `process` is simply consumed. The
        // bell is scanned here too rather than taken from `Event::Bell`, so that
        // all three signals come from one place and cannot double-fire. Only agent
        // tabs track signals; companion terminals leave the scanner unused.
        let mut scanner = crate::attention::AttentionScanner::new();
        let mut overflow_seen = 0u64;
        // End of input, from either arm below. The instant is stamped BEFORE the
        // flag is published so an observer that sees `is_exited()` (an `Acquire`
        // load) is guaranteed to see the instant too, never `is_exited() == true`
        // with `exited_at() == None`. Prune's readiness rule reads them as a pair.
        let mark_eof = || {
            let _ = exited_at.set(Instant::now());
            exited.store(true, Ordering::Release);
        };
        loop {
            match crate::io_retry::retry_on_interrupt(|| reader.read(&mut buf)) {
                Ok(0) => {
                    mark_eof();
                    break;
                }
                Ok(n) => {
                    let data = &buf[..n];

                    // Scan for attention/progress signals on the raw bytes, before
                    // the parser gets them and swallows the two it has no event
                    // for (see the scanner's declaration). The same
                    // pass captures whitelisted passthrough sequences into the ring
                    // (unconditionally: the host decides whether to forward them at
                    // drain time, so a live config toggle applies immediately and a
                    // never-draining headless server keeps the ring bounded).
                    if track_agent_signals {
                        let mut captures: Vec<crate::attention::CapturedSeq> = Vec::new();
                        for event in scanner.scan_full(data, Some(&mut captures)) {
                            match event {
                                crate::attention::AttentionEvent::Bell => {
                                    attention_bell.store(true, Ordering::Release);
                                }
                                crate::attention::AttentionEvent::Notify => {
                                    attention_notify.store(true, Ordering::Release);
                                }
                                crate::attention::AttentionEvent::Progress { working } => {
                                    if let Ok(mut slot) = progress.lock() {
                                        *slot = Some(ProgressReport {
                                            working,
                                            at: Instant::now(),
                                        });
                                    }
                                }
                            }
                        }
                        // A runaway unterminated sequence was dropped: surface it
                        // at debug level (the scanner stays pure and only counts).
                        let drops = scanner.overflow_drops();
                        if drops != overflow_seen {
                            overflow_seen = drops;
                            logger::debug(
                                "attention scanner dropped an over-long unterminated escape sequence",
                            );
                        }
                        // Push captured passthrough sequences into the bounded ring,
                        // dropping the oldest on overflow and any single sequence
                        // that exceeds the per-seq cap.
                        if !captures.is_empty()
                            && let Ok(mut ring) = passthrough.lock()
                        {
                            let mut dropped = 0u64;
                            for seq in captures {
                                if seq.bytes.len() > PASSTHROUGH_SEQ_MAX {
                                    dropped += 1;
                                    continue;
                                }
                                if ring.len() >= PASSTHROUGH_RING_CAP {
                                    ring.pop_front();
                                    dropped += 1;
                                }
                                ring.push_back(seq);
                            }
                            if dropped != 0 {
                                logger::warn(&format!(
                                    "passthrough ring dropped {dropped} captured sequence(s) \
                                     (ring cap reached or an individual sequence exceeded the \
                                     per-sequence size limit); the host will not see them"
                                ));
                            }
                        }
                    }

                    // Take the terminal lock BEFORE the subscriber fan-out and
                    // hold it across both, so "this chunk reached the
                    // subscribers" and "this chunk reached the grid" are ONE
                    // atomic step as far as `subscribe_with_repaint` can tell.
                    // That is what makes a fresh connection see every byte
                    // exactly once.
                    //
                    // The fan-out used to run outside this lock, and the gap was
                    // not the "few bytes" the old comment claimed: `Mutex` is not
                    // fair, so the reader barges: it releases the lock, reads the
                    // next chunk and re-acquires while a subscriber that has
                    // ALREADY registered is still parked in the futex waiting for
                    // its snapshot. Every chunk that lands in that window is fanned
                    // out to that subscriber AND parsed into the grid it is about to
                    // be handed, so the client renders the snapshot's tail and then
                    // a replay of bytes already inside it (measured: thousands of
                    // duplicated lines, which reads as a jump forward followed by a
                    // jump back). Duplication is invisible in a full-screen TUI that
                    // repaints over itself; line-oriented output appends, so it is
                    // corruption. Keep the two under one lock.
                    //
                    // What it costs is nothing the reader did not already pay: the
                    // ingest below takes this same lock, on every chunk, so the
                    // reader waits exactly where it always waited. It does mean a
                    // browser building a reconnect replay stalls the reader for as
                    // long as the build takes, and everything downstream of the
                    // read waits with it, including the attention scan above (bell
                    // and notification detection for THIS one terminal lands late,
                    // and is not lost). Measured on one machine: about 10ms to
                    // build a replay at the default 10_000-line scrollback and
                    // about 100ms at the `MAX_RECONNECT_REPLAY_LINES` cap. Treat
                    // those as a floor rather than the number: the build loop is
                    // rows-times-COLUMNS unconditionally, because every row is
                    // scanned from its right edge to right-trim trailing empty
                    // cells even when the row is blank, so a wide terminal costs
                    // several times a narrow one at the same row count. The trade
                    // is the cheap side: a reader running ahead of the grid is
                    // exactly the state in which a connecting client loses bytes
                    // outright, and the only consequence of making it wait is that
                    // the child blocks on a full PTY buffer, which loses nothing.
                    //
                    // A poisoned terminal mutex means some other thread panicked
                    // while holding it. Nothing here can fix that, but the fan-out
                    // is independent of the grid, so keep streaming to web clients
                    // exactly as this loop did before and skip only the ingest.
                    let Ok(mut terminal) = terminal.lock() else {
                        fan_out_to_subscribers(&subscribers, data);
                        continue;
                    };
                    fan_out_to_subscribers(&subscribers, data);

                    // Every chunk is parsed, unconditionally. There is deliberately
                    // no "the operator is reading scrollback, hold this back" branch
                    // here: dux used to have one, buffering unparsed bytes in a
                    // 4 MiB side buffer and DROPPING THE OLDEST on overflow, so a
                    // scrollback session across a busy build lost the middle of it
                    // for good. Reading history is a view operation and must not
                    // change what the terminal records, which is how real terminals
                    // behave (tmux parses pty data regardless of copy mode; copy
                    // mode routes the user's KEYS, not the child's bytes). The
                    // stable-view part of that behaviour is the display offset's
                    // job, in `TerminalState`, not the reader's.
                    let replies = terminal.process(data);
                    dirty.store(true, Ordering::Release);
                    // Streaming/"working" signal: only a real content change in the
                    // ACTIVE AREA counts as the agent producing output. OSC status
                    // sequences (OSC 9;4 progress) and other non-rendering bytes
                    // advance the parser without changing the grid, so they must
                    // not read as activity; `is_agent_streaming` consults them only
                    // as a fallback. The active area, not the DISPLAYED viewport:
                    // while the operator is scrolled back the viewport is immutable
                    // history and its fingerprint can never change, so hashing it
                    // would make a still-producing agent read as idle (see
                    // `take_content_change`). The raw `dirty` flag above still fires
                    // for rendering regardless.
                    if terminal.take_content_change() {
                        received_data.store(true, Ordering::Release);
                    }
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
                Err(err) => {
                    logger::debug(&format!("PTY reader error: {err}"));
                    mark_eof();
                    break;
                }
            }
        }
        // The PTY is gone (EOF/error): drop every subscriber sender so each live
        // web viewer's receiver disconnects promptly. Without this the senders
        // linger in the shared list (each `PtyViewerGuard` holds an `Arc` clone
        // that keeps the `Vec`, and therefore the `Sender`s, alive), so a
        // web forwarder blocked on `recv_timeout` would only ever see `Timeout`,
        // never `Disconnected` — its task would never end and its PTY socket
        // would dangle (pinning a connection-cap permit) until the browser
        // itself disconnected. Clearing here is what lets the socket's
        // forwarder-completion arm reap the connection on server-side teardown.
        if let Ok(mut subs) = subscribers.lock() {
            subs.clear();
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
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.snapshot()
    }

    /// Subscribe to the live raw-byte stream. Returns a [`PtyViewerGuard`] and a
    /// `Receiver`. The receiver gets a clone of every chunk read from the PTY
    /// from now on. Dropping the guard immediately removes this subscriber from
    /// the fan-out list; the receiver will observe disconnection on the next
    /// `recv` or `try_recv` call after the guard is dropped.
    pub fn subscribe(&self) -> (PtyViewerGuard, std::sync::mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let id = self.next_sub_id.fetch_add(1, Ordering::Relaxed);
        // Guard against the subscribe-after-clear window: the reader thread
        // sets `exited` and then does a ONE-SHOT `subs.clear()` on EOF/error
        // (see `spawn_reader` above); `prune_exited_ptys` is the only later
        // remover, running once per tick. A subscribe landing after that
        // one-shot clear but before the next prune tick used to attach to a
        // client that will never see another `subs.clear()` call — the
        // forwarder's `recv_timeout` would only ever see `Timeout`, never
        // `Disconnected`, so its task (and the PTY socket, connection-cap
        // permit, and per-tab subscriber-quota slot it holds) would never be
        // reaped until the browser itself disconnected. `exited` is monotonic
        // (set once, never reset), so checking it here — instead of pushing
        // unconditionally — closes the window: if the PTY has already exited,
        // don't register at all. `tx` is dropped without being stored, so `rx`
        // observes `Disconnected` on its very next `recv`/`try_recv`, letting
        // the caller's forwarder complete and reap immediately instead of
        // leaking until the next prune tick (which would still miss it, since
        // the entry was never in `subscribers` for `prune_exited_ptys` to see
        // in the first place — the leak was in never delivering `Disconnected`
        // at all).
        if !self.is_exited() {
            self.subscribers
                .lock()
                .expect("subscribers mutex poisoned")
                .push((id, tx));
        }
        let guard = PtyViewerGuard {
            id,
            subs: Arc::clone(&self.subscribers),
        };
        (guard, rx)
    }

    /// Subscribe and also return a synthesized ANSI repaint of the current
    /// screen, so a freshly-connected client can prime its terminal before the
    /// live stream arrives.
    ///
    /// Registration and the snapshot happen under ONE hold of the terminal lock,
    /// which the reader thread also holds across its fan-out plus ingest of each
    /// chunk (see `spawn_reader`). That makes the handoff exact: every chunk is
    /// either wholly before this subscriber existed (so it is in the repaint and
    /// not in the channel) or wholly after (so it is in the channel and not in the
    /// repaint). The client therefore sees each byte exactly once, in order, with
    /// nothing lost.
    ///
    /// The repaint is the grid and only the grid, which is exact because the
    /// reader parses every chunk as it arrives. There is no third place a byte
    /// can be sitting: it is either already in the grid (so the repaint has it)
    /// or it has not been read yet (so the channel will get it).
    ///
    /// Returns `(guard, repaint_bytes, receiver)`. Hold the guard for the
    /// connection's lifetime; dropping it removes the subscriber immediately.
    pub fn subscribe_with_repaint(
        &self,
    ) -> (PtyViewerGuard, Vec<u8>, std::sync::mpsc::Receiver<Vec<u8>>) {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        // Order within the lock is immaterial (the reader cannot run either step
        // while we hold it), so keep registering first: the receiver exists before
        // anything is read off the grid.
        let (guard, rx) = self.subscribe();
        let repaint = terminal.reconnect_repaint();
        drop(terminal);
        (guard, repaint, rx)
    }

    /// Fill `target` with the current terminal viewport, reusing its `cells`
    /// allocation to avoid per-frame heap churn. Returns `true` if the
    /// snapshot was rebuilt, `false` if the terminal was unchanged and
    /// `target` still holds valid data from the previous call.
    ///
    /// `collect_links` controls whether OSC 8 hyperlinks are interned into
    /// `target.links` (the TUI passes `config.capabilities.hyperlinks`): when
    /// `false` no cell carries a `link` and the interning work is skipped
    /// entirely, so a config that disables hyperlinks pays nothing.
    pub fn snapshot_into(&self, target: &mut TerminalSnapshot, collect_links: bool) -> bool {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return false;
        }
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.snapshot_into(target, collect_links);
        true
    }

    pub fn scrollback_offset(&self) -> usize {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.scrollback_offset()
    }

    /// Adjust the scrollback offset by the given amount in the given direction.
    /// Scrolling only moves the VIEW: the reader keeps parsing the child's
    /// output into the grid the whole time.
    pub fn scroll(&self, up: bool, amount: usize) {
        self.mutate_scroll(|t| t.scroll(up, amount));
    }

    /// Set the scrollback offset (0 = normal view, positive = scrolled back).
    pub fn set_scrollback(&self, rows: usize) {
        self.mutate_scroll(|t| t.set_scrollback(rows));
    }

    /// Run a closure under the terminal lock and mark the grid dirty so the next
    /// snapshot rebuilds.
    fn mutate_scroll<F>(&self, mutate: F)
    where
        F: FnOnce(&mut TerminalState),
    {
        let Ok(mut terminal) = self.terminal.lock() else {
            return;
        };
        mutate(&mut terminal);
        self.dirty.store(true, Ordering::Release);
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
        // A resize can move the scrollback offset (growing the viewport pulls
        // history into the grid and resets the display offset to 0). That is
        // purely a view change now, so there is nothing to synchronize beyond
        // marking the grid dirty.
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
            .map(|t| t.has_minimal_output(threshold))
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

    /// Consume any pending attention signals, returning `true` if the agent
    /// asked for attention since the last call. Both the bell and notification
    /// flags are drained every call (so a suppressed signal never lingers);
    /// `count_bell` gates whether a plain bell counts (the `attention_on_bell`
    /// preference). Notifications always count when the feature is enabled.
    pub fn take_attention(&self, count_bell: bool) -> bool {
        let notify = self.attention_notify.swap(false, Ordering::AcqRel);
        let bell = self.attention_bell.swap(false, Ordering::AcqRel);
        notify || (count_bell && bell)
    }

    /// Drain and return every captured passthrough sequence, oldest first. Mirrors
    /// [`PtyClient::take_attention`]: the caller (the engine) decides which kinds to
    /// forward. Companion terminals never capture, so this is always empty for them.
    pub fn take_passthrough(&self) -> Vec<crate::attention::CapturedSeq> {
        self.passthrough
            .lock()
            .map(|mut ring| ring.drain(..).collect())
            .unwrap_or_default()
    }

    /// How many captured passthrough sequences are currently buffered (not yet
    /// drained). Test/diagnostic support: the ring push is the LAST step of a
    /// reader-loop scan pass, so a non-zero count proves the reader has fully
    /// processed a chunk that emitted a capture. Engine tests wait on this instead
    /// of `progress_report()` (a separate mutex set earlier in the same pass) to
    /// avoid observing a half-applied scan.
    #[cfg(test)]
    pub(crate) fn passthrough_pending(&self) -> usize {
        self.passthrough.lock().map(|ring| ring.len()).unwrap_or(0)
    }

    /// The most recent `OSC 9;4` progress report, if the agent emitted one. This
    /// is a non-consuming read: the value persists until a newer report replaces
    /// it. Staleness is the engine's concern.
    pub fn progress_report(&self) -> Option<ProgressReport> {
        self.progress.lock().ok().and_then(|slot| *slot)
    }

    /// Whether the child process has enabled any mouse tracking mode
    /// (e.g. via DECSET 1000/1002/1003). When true, non-scroll mouse
    /// events should be forwarded to the PTY rather than dropped.
    pub fn has_mouse_mode(&self) -> bool {
        self.terminal.lock().is_ok_and(|t| t.has_mouse_mode())
    }

    /// Non-blocking check of the child's exit status, memoized.
    ///
    /// The underlying `Child::try_wait` reaps the zombie and so yields the
    /// status exactly once; every later call returns `None`. Several call sites
    /// poll the same client (the exit prune, the shutdown sweep, the
    /// terminating-PTY reaper, tests), so the raw behaviour means whichever one
    /// polls first silently steals the status. This caches the first observed
    /// status and replays it, making the call idempotent: once a child has been
    /// reaped, `try_wait` keeps saying so.
    pub fn try_wait(&mut self) -> Option<portable_pty::ExitStatus> {
        if let Some((status, _)) = &self.reaped {
            return Some(status.clone());
        }
        let status = self.child.try_wait().ok().flatten()?;
        self.reaped = Some((status.clone(), Instant::now()));
        Some(status)
    }

    /// When this client's child was first observed to have exited, or `None` if
    /// it has not been reaped yet (i.e. no [`PtyClient::try_wait`] call has seen
    /// a status). Note this is the REAP instant, which is not the same moment as
    /// [`PtyClient::is_exited`] flipping: the child can die while the PTY read
    /// side is still open, so callers that need the child's output fully
    /// ingested must wait for `is_exited`, and use this only to bound that wait.
    pub fn reaped_at(&self) -> Option<Instant> {
        self.reaped.as_ref().map(|(_, at)| *at)
    }

    /// When the reader thread reached end of input, or `None` while the PTY read
    /// side is still open. `Some` exactly when [`PtyClient::is_exited`] is true.
    ///
    /// This is the counterpart clock to [`PtyClient::reaped_at`], and the two
    /// bound different waits. A caller that waits for the exit STATUS after EOF
    /// cannot bound that wait on the reap, because the wait exists precisely for
    /// the case where no reap has happened: a child that closes its descriptors
    /// and keeps running reaches EOF and is never reapable at all.
    pub fn exited_at(&self) -> Option<Instant> {
        self.exited_at.get().copied()
    }

    /// Returns the PID of the shell process spawned in this PTY.
    pub fn child_process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Politely ask the child to exit (SIGTERM, then SIGHUP), so the CLI or the
    /// app running in a terminal can flush state before the hard group `kill()`
    /// in `Drop` (or process teardown) reaps stragglers. Signals the child's
    /// whole process group (the child is a process-group leader -- portable-pty
    /// calls `setsid` -- so a signal aimed at the lone PID would leave its
    /// descendants running) AND the foreground process group when a
    /// job-controlled app owns a different one: an interactive shell (a
    /// terminal) puts each foreground command in its own pgroup, so signaling
    /// only the shell's group would never reach the running app. See
    /// [`Self::signal_process_groups`].
    ///
    /// SIGHUP rides along because SIGTERM alone can never end a companion
    /// terminal: an interactive shell deliberately IGNORES SIGTERM, so even
    /// with its foreground app dead the shell survived and every shutdown ate
    /// the full force-kill timeout. SIGHUP is the signal that means "your
    /// terminal went away" -- exactly what a real terminal emulator delivers on
    /// close -- and shells answer it by resending HUP to their jobs and
    /// exiting. Apps that handle SIGTERM exit on the first signal and never
    /// observe the second.
    pub fn terminate(&self) {
        let _ = self.signal_process_groups(rustix::process::Signal::TERM);
        let _ = self.signal_process_groups(rustix::process::Signal::HUP);
    }

    /// Hard-kill the child's whole process group (SIGKILL) — the forceful
    /// counterpart to [`terminate`]. `shutdown_ptys` calls this for any child
    /// that has not exited once the grace period elapses, so the "force-closing"
    /// log line is truthful at the instant it prints rather than relying solely
    /// on the SIGKILL in `Drop`. Signals the group (not just the lone PID) for
    /// the same `setsid` reason `terminate` and `Drop` do. `Drop` remains the
    /// backstop for any path that never calls this.
    ///
    /// A genuine failure (anything but `ESRCH`, which just means the group is
    /// already gone) is logged at WARN: this is a deliberate, operator-visible
    /// shutdown action, and a SIGKILL that did not land leaves a child running
    /// while `shutdown_ptys` reports it force-closed, so it must leave a trace
    /// (mirrors the `Drop` breadcrumb, louder because the context is explicit).
    pub fn force_terminate(&self) {
        if let Err(err) = self.signal_process_groups(rustix::process::Signal::KILL)
            && err != rustix::io::Errno::SRCH
        {
            logger::warn(&format!(
                "PtyClient::force_terminate: kill_process_group failed: {err}"
            ));
        }
    }

    /// Send `sig` to the child's own process group AND, when a job-controlled app
    /// is running in a DIFFERENT foreground process group, to that group too.
    ///
    /// An interactive shell (which is what a companion terminal runs) enables job
    /// control and places each foreground command in its own process group, then
    /// hands it the terminal foreground. So a running app (an editor, a dev
    /// server, a REPL) does NOT share the shell's process group, and signaling
    /// only the shell's group -- which is what a plain `kill_process_group(child)`
    /// does -- never reaches the app. On close/shutdown that left the app running
    /// until the grace period's SIGKILL; here we also signal the foreground group
    /// so the app itself is asked to exit (SIGTERM) or reaped (SIGKILL) directly.
    /// When the shell owns the foreground (idle) the foreground pgid equals the
    /// child pid and the extra signal is skipped. Returns the first error (ESRCH,
    /// "group already gone", is benign).
    fn signal_process_groups(&self, sig: rustix::process::Signal) -> Result<(), rustix::io::Errno> {
        let Some(child) = self.child_process_id() else {
            return Ok(());
        };
        let Some(child_group) = rustix::process::Pid::from_raw(child as i32) else {
            return Ok(());
        };
        let child_res = rustix::process::kill_process_group(child_group, sig);
        let fg_res = match self.foreground_pgid() {
            Some(fg) if fg != child => rustix::process::Pid::from_raw(fg as i32)
                .map(|group| rustix::process::kill_process_group(group, sig))
                .unwrap_or(Ok(())),
            _ => Ok(()),
        };
        child_res.and(fg_res)
    }

    /// The PTY's foreground process group id (via `tcgetpgrp` on the master), or
    /// `None` if it cannot be read. Equals the child's own pid when the shell owns
    /// the foreground (an idle prompt); differs when a job-controlled app runs in
    /// its own group.
    fn foreground_pgid(&self) -> Option<u32> {
        let raw_fd = self.master.as_raw_fd()?;
        foreground_pgid_of_fd(raw_fd)
    }

    /// Returns the name of the foreground process running in this PTY, or
    /// `None` if the shell itself is in the foreground (idle).
    ///
    /// Uses `tcgetpgrp()` (see `foreground_pgid_of_fd` for why it is a direct
    /// libc call) to get the foreground process group and compares it to the
    /// shell PID. If they differ, a child command is running and its name is
    /// resolved via platform-specific APIs.
    pub fn foreground_process_name(&self) -> Option<String> {
        let raw_fd = self.master.as_raw_fd()?;
        let fg_pid = foreground_pgid_of_fd(raw_fd)?;

        let shell_pid = self.child.process_id()?;
        if fg_pid == shell_pid {
            // Shell itself is in the foreground — no command running.
            return None;
        }

        process_name(fg_pid)
    }
}

/// The foreground process group of `fd` via a DIRECT `libc::tcgetpgrp` call,
/// validated by `valid_pgid` before use. Deliberately not rustix's wrapper:
/// on macOS a PTY whose foreground process group is gone returns 0, and
/// rustix 1.1.4 guards that case only on Linux (`#[cfg(linux_kernel)]` in its
/// termios syscalls) before feeding the raw value into
/// `Pid::from_raw_unchecked`. That is an assertion failure on debug builds
/// (it killed the engine thread in production the moment the foreground poll
/// touched an orphaned PTY) and an invalid `NonZeroI32` on release builds.
/// Here an invalid id is simply "no foreground process", which is also what
/// it means.
fn foreground_pgid_of_fd(raw_fd: std::os::unix::io::RawFd) -> Option<u32> {
    // SAFETY: `tcgetpgrp` only reads from the fd, which the caller owns for
    // the duration of the call; an invalid fd yields -1, which `valid_pgid`
    // rejects.
    let raw = unsafe { libc::tcgetpgrp(raw_fd) };
    valid_pgid(raw)
}

/// Whether a raw `tcgetpgrp` result is a usable process-group id: strictly
/// positive. Zero (macOS's "no foreground process group" on an orphaned PTY)
/// and negatives (error returns) are not, and must never reach a
/// `Pid`/`NonZero` construction.
fn valid_pgid(raw: libc::pid_t) -> Option<u32> {
    (raw > 0).then_some(raw as u32)
}

/// Whether a batch of bytes written to a PTY should count as the user "typing"
/// for the Typing-state / working-suppression window (see
/// [`crate::engine::Engine::note_pty_input`]). Genuine keystrokes and pastes
/// count; three things do not, because they are the terminal reporting an event
/// to the child rather than the user entering text:
///   - an empty frame (a no-op / keepalive write),
///   - a FOCUS REPORT (`CSI I` focus-in / `CSI O` focus-out), which xterm.js
///     emits when the viewer gains or loses focus while the child app has focus
///     tracking (DECSET 1004) on. Selecting a terminal focuses it, so without
///     this a plain focus change would light "Typing" with nothing typed, and
///   - a MOUSE REPORT (SGR `CSI < ...` or legacy `CSI M ...`), which the viewer
///     sends when the child has mouse reporting on and the user clicks or
///     scrolls. Scrolling and clicking are not typing.
///
/// The bytes are still written to the PTY (the child receives its focus/mouse
/// event); they simply do not stamp the input window. No printable key or cursor
/// key encodes as one of these sequences, so genuine input is never dropped.
pub fn write_counts_as_typing(bytes: &[u8]) -> bool {
    !bytes.is_empty() && !is_focus_report(bytes) && !is_mouse_report(bytes)
}

fn is_focus_report(bytes: &[u8]) -> bool {
    bytes == b"\x1b[I" || bytes == b"\x1b[O"
}

fn is_mouse_report(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x1b[<") || bytes.starts_with(b"\x1b[M")
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
        // reaps those descendants so the slave is released. A job-control
        // FOREGROUND app (in its own group under an interactive shell) is also
        // reached, via the foreground-group signal in `signal_process_groups`.
        // (A descendant that has left both groups — a double-forked daemon, or a
        // job-control BACKGROUND job — is still out of reach here. A well-behaved
        // daemon redirects its inherited
        // terminal fds away before detaching so it will not hold the slave
        // open; a misbehaving one that keeps the slave open could still stall
        // the join, though that has not been observed with the supported
        // providers.)
        // SIGKILL the child's group AND the foreground group when a job-controlled
        // app owns a different one (see `signal_process_groups`). ESRCH just means
        // a group already exited (benign). Anything else (e.g. EPERM) means a kill
        // did not happen, so the reader join below could stall — leave a
        // breadcrumb in the log.
        if let Err(err) = self.signal_process_groups(rustix::process::Signal::KILL)
            && err != rustix::io::Errno::SRCH
        {
            logger::debug(&format!(
                "PtyClient::drop: kill_process_group failed: {err}"
            ));
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
    term: Term<EventProxy>,
    parser: Processor<StdSyncHandler>,
    event_proxy: EventProxy,
    rows: u16,
    cols: u16,
    /// Fingerprint of the ACTIVE AREA's content at the last
    /// `take_content_change` call. Drives the streaming/"working" signal: only a
    /// change in what the child actually rendered counts as it producing output,
    /// so non-rendering bytes (OSC status sequences like OSC 9;4 progress, color
    /// queries) never read as activity. `None` until the first check.
    ///
    /// The active area, deliberately, and not the displayed viewport: "did the
    /// child produce output" is a property of the terminal, not of where the
    /// human is looking. A user scrolled back further than one screen height
    /// sees only immutable history, so a viewport fingerprint would never change
    /// and the agent would read as idle while it was still working.
    last_content_hash: Option<u64>,
    /// The child's scrolling region, tracked by a second parser over the same
    /// bytes. `Term` keeps its own copy in a private field with no accessor, so
    /// this is the only way to read the region back out for a reconnect repaint.
    /// See [`crate::scroll_margins`] for why it is a mirror of the engine's
    /// behaviour rather than an independent reading of the specification.
    scroll_region: ScrollRegionTracker,
}

impl TerminalState {
    fn new(rows: u16, cols: u16, scrollback_lines: usize) -> Self {
        Self::with_scrollback(rows, cols, scrollback_lines)
    }

    fn with_scrollback(rows: u16, cols: u16, scrollback: usize) -> Self {
        let event_proxy = EventProxy::new(rows, cols);
        let dimensions = TerminalDimensions::new(rows, cols);
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &dimensions, event_proxy.clone()),
            parser: Processor::new(),
            event_proxy,
            rows,
            cols,
            last_content_hash: None,
            scroll_region: ScrollRegionTracker::new(rows, cols),
        }
    }

    /// Whether the ACTIVE AREA's content changed since the last call (real
    /// output), as opposed to only non-rendering bytes advancing the parser (OSC
    /// status sequences, color queries, cursor-only moves). Fingerprints the
    /// rendered characters, deliberately cursor-independent, so a bare cursor
    /// blink/move or an OSC 9;4 progress report never reads as activity. Drives
    /// the streaming/"working" signal; the raw snapshot dirty flag is separate.
    ///
    /// It hashes the active area rather than the DISPLAYED viewport on purpose.
    /// The two are the same thing only while the display offset is zero. Scroll
    /// back past one screen height and the viewport is nothing but immutable
    /// history: its fingerprint can never change, so the agent would read as
    /// idle, the spinner and shimmer would stop, the poll rate would drop, and
    /// every browser watching that agent would turn its working badge off, all
    /// while the child was still producing output.
    fn take_content_change(&mut self) -> bool {
        use std::hash::{Hash, Hasher};
        // Walking the active area's lines in a fixed order and hashing each
        // cell's character captures both the content AND its layout: a spinner
        // cycling a glyph, a new line of text, or a scroll all change the
        // sequence, while a cursor-only move or an OSC status write leave it
        // identical.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let screen_lines = self.term.screen_lines() as i32;
        for line in 0..screen_lines {
            for cell in &self.term.grid()[Line(line)] {
                cell.c.hash(&mut hasher);
            }
        }
        let hash = hasher.finish();
        let changed = self.last_content_hash != Some(hash);
        self.last_content_hash = Some(hash);
        changed
    }

    fn process(&mut self, data: &[u8]) -> Vec<u8> {
        self.parser.advance(&mut self.term, data);
        self.clamp_display_offset_to_history();
        // The same bytes, through a second parser that watches only the scrolling
        // region. The two cannot disagree about a synchronized update, even
        // though each keeps its own buffer and its own 150ms timer. The timer is
        // never what releases a batch here: the parser only consults
        // `pending_timeout`, which reports whether a timeout was ever ARMED and
        // not whether it has expired, and the one entry point that acts on expiry
        // (`Processor::stop_sync`) is never called from dux. So a batch is
        // released by exactly two things, the ESU sequence in the stream and the
        // sync buffer filling, and both are pure functions of the bytes. Feed
        // both parsers the same bytes and they buffer and release in lockstep.
        self.scroll_region.advance(data);
        let pending = self.event_proxy.take_pending();
        let mut replies = pending.bytes;

        for request in pending.color_requests {
            let rgb = resolve_color_request_rgb(request.index, self.term.colors());
            replies.extend_from_slice((request.formatter)(rgb).as_bytes());
        }

        replies
    }

    /// Workaround for a bug in `alacritty_terminal` 0.26.0, not for one of ours.
    ///
    /// `Grid::scroll_up` (grid/mod.rs) bumps the display offset for every scroll
    /// while the offset is non-zero, so that a scrolled-back view stays parked
    /// on the same content. But it only pushes the scrolled-out line into
    /// history when the scrolling region starts at row zero, and it clamps the
    /// bumped offset to `max_scroll_limit` (the configured scrollback capacity)
    /// rather than to the current history size. Every other clamp in that crate
    /// uses the history size; that one line is the odd one out. So a child that
    /// sets a scrolling region with a TOP margin and scrolls it, while the user
    /// is scrolled back and the history ring is not yet full, walks the offset
    /// past the history size.
    ///
    /// Measured on the pinned 0.26.0: a 5-row terminal with 3 lines of history,
    /// scrolled to the top, given `CSI 2;5r` and eight line feeds, ends with a
    /// display offset of 11 against a history size of 3, and the next render
    /// panics inside the library:
    ///
    /// ```text
    /// panicked at alacritty_terminal-0.26.0/src/grid/storage.rs:225:9:
    /// assertion failed: positive < self.len
    /// ```
    ///
    /// Debug builds panic, so this is reachable from `cargo test` and
    /// `cargo run`; release builds did not panic in the range tested but render
    /// wrapped ring rows, which is wrong content rather than a crash.
    ///
    /// The fix has to be on the WRITE side. The panic happens inside the library
    /// while it builds its display iterator from its own private offset field,
    /// and dux never indexes the grid by offset itself (every read goes through
    /// `renderable_content`), so clamping a number dux reads back out would
    /// change nothing. `Scroll::Delta(0)` re-clamps the offset to the history
    /// size and is measured to fix it. This also keeps the scrollback badge
    /// honest: the snapshot reads offset and total together, and an unclamped
    /// grid yielded nonsense pairs like "57/16".
    ///
    /// The guard is what keeps the common path cheap: one comparison, and no
    /// touching of damage tracking or the event proxy (both of which
    /// `Term::scroll_display` does unconditionally) on every chunk.
    fn clamp_display_offset_to_history(&mut self) {
        if self.term.grid().display_offset() > self.term.grid().history_size() {
            self.term.scroll_display(Scroll::Delta(0));
        }
    }

    fn has_visible_output(&self) -> bool {
        self.term
            .renderable_content()
            .display_iter
            .any(|indexed| !indexed.cell.c.is_whitespace())
    }

    /// Count the number of distinct viewport rows that contain at least one
    /// non-whitespace character.
    fn visible_line_count(&self) -> usize {
        let mut seen_rows = std::collections::HashSet::new();
        for indexed in self.term.renderable_content().display_iter {
            if !indexed.cell.c.is_whitespace() {
                seen_rows.insert(indexed.point.line.0);
            }
        }
        seen_rows.len()
    }

    /// Returns `true` if the terminal contains only a small amount of output:
    /// no scrollback history AND at most `threshold` visible lines with content.
    /// Used to detect failed `--continue` exits that print a short error message.
    fn has_minimal_output(&self, threshold: usize) -> bool {
        self.term.grid().history_size() == 0 && self.visible_line_count() <= threshold
    }

    fn visible_text_excerpt(&self, max_lines: usize) -> String {
        let mut rows = vec![String::new(); usize::from(self.rows)];
        for indexed in self.term.renderable_content().display_iter {
            let Ok(row) = usize::try_from(indexed.point.line.0) else {
                continue;
            };
            let Some(line) = rows.get_mut(row) else {
                continue;
            };
            while line.chars().count() < indexed.point.column.0 {
                line.push(' ');
            }
            line.push(indexed.cell.c);
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
        self.term.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// Whether the child process has switched to the alternate screen buffer
    /// (e.g. via DECSET 1049). Full-screen TUI apps like opencode use the
    /// alt screen; Claude and shells use the main screen.
    fn is_alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    fn snapshot(&self) -> TerminalSnapshot {
        let mut snap = TerminalSnapshot::empty();
        self.snapshot_into(&mut snap, true);
        snap
    }

    /// Fill `target` with the current terminal viewport, reusing its existing
    /// `cells` allocation to avoid per-frame heap churn. When `collect_links` is
    /// false, OSC 8 hyperlink interning is skipped and every cell's `link` is
    /// `None`.
    fn snapshot_into(&self, target: &mut TerminalSnapshot, collect_links: bool) {
        let renderable = self.term.renderable_content();
        let display_offset = renderable.display_offset;
        let history_size = self.term.grid().history_size();
        let colors = renderable.colors;
        let cursor = if renderable.cursor.shape == CursorShape::Hidden {
            None
        } else {
            term::point_to_viewport(display_offset, renderable.cursor.point).map(|point| {
                SnapshotCursor {
                    row: point.line as u16,
                    col: point.column.0 as u16,
                }
            })
        };

        target.cells.clear();
        target.links.clear();
        // Per-call side index (URI -> index into `target.links`) so interning a
        // link that spans many cells stays O(1) instead of a linear scan of
        // `target.links` per cell (which was quadratic for a wide run of distinct
        // links). Rebuilt every call alongside `target.links`.
        let mut link_index: std::collections::HashMap<String, u16> =
            std::collections::HashMap::new();
        for indexed in renderable.display_iter {
            let cell = indexed.cell;
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }

            let Some(point) = term::point_to_viewport(display_offset, indexed.point) else {
                continue;
            };

            let mut symbol = CompactString::new("");
            symbol.push(cell.c);
            if let Some(zerowidth) = cell.zerowidth() {
                for ch in zerowidth {
                    symbol.push(*ch);
                }
            }

            // Intern this cell's OSC 8 hyperlink URI (if any) into the per-snapshot
            // table, deduplicating via `link_index` so a link spanning many cells
            // stores its URI once. Gated three ways: only when `collect_links` is
            // set (hyperlinks enabled); only for a forwardable scheme (http/https,
            // no control bytes, mirroring the web link handler) so a `file://` or
            // `javascript:` URI never wraps; and only up to `MAX_SNAPSHOT_LINKS`
            // distinct URIs so a hostile flood of unique links cannot grow the
            // table without bound. A cell whose URI fails any gate renders as plain
            // text (`link = None`) and does not consume a table slot.
            let link = if collect_links {
                cell.hyperlink().and_then(|hyperlink| {
                    let uri = hyperlink.uri();
                    if !is_forwardable_link_uri(uri) {
                        return None;
                    }
                    if let Some(idx) = link_index.get(uri) {
                        return Some(*idx);
                    }
                    if target.links.len() >= MAX_SNAPSHOT_LINKS {
                        return None;
                    }
                    let idx = u16::try_from(target.links.len()).ok()?;
                    target.links.push(uri.to_string());
                    link_index.insert(uri.to_string(), idx);
                    Some(idx)
                })
            } else {
                None
            };

            target.cells.push(SnapshotCell {
                row: point.line as u16,
                col: point.column.0 as u16,
                symbol,
                fg: convert_terminal_color(cell.fg, colors),
                bg: convert_terminal_color(cell.bg, colors),
                modifier: cell_modifier(cell),
                link,
            });
        }

        target.rows = self.rows;
        target.cols = self.cols;
        target.scrollback_offset = display_offset;
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
    fn reconnect_repaint(&self) -> Vec<u8> {
        if self.is_alt_screen() {
            let snapshot = self.snapshot();
            let mut out = synthesize_repaint(&snapshot, true, self.scroll_region.region());
            // A HIDDEN cursor is absent from the snapshot, because a snapshot
            // describes what is drawn and a hidden cursor draws nothing. So
            // `synthesize_repaint` emitted no positioning for it, and the region
            // restore it emitted just before that HOMED the cursor. Place it
            // explicitly: a hidden cursor still has a position, and a program that
            // moves or prints relative to it resumes from the wrong cell.
            if snapshot.cursor.is_none() {
                let renderable = self.term.renderable_content();
                if let Some(point) =
                    term::point_to_viewport(renderable.display_offset, renderable.cursor.point)
                {
                    out.extend_from_slice(
                        format!("\x1b[{};{}H", point.line + 1, point.column.0 + 1).as_bytes(),
                    );
                }
            }
            // Cells alone are not the terminal's state: re-assert the child's
            // private modes so a client that reset before applying the replay
            // (every web reconnect does) comes back with mouse tracking,
            // bracketed paste and cursor visibility intact.
            out.extend_from_slice(mode_restore_sequence(*self.term.mode()).as_bytes());
            return out;
        }

        let renderable = self.term.renderable_content();
        let colors = renderable.colors;
        // The replay always rebuilds the buffer ending at the live bottom, so the
        // cursor must be mapped as if the grid were NOT scrolled back (display
        // offset 0). Using the live `display_offset` here would add the scrollback
        // distance and emit an out-of-range cursor position whenever a TUI
        // operator is reading history at the moment a web client connects.
        //
        // Visibility is NOT a reason to skip this. A hidden cursor still has a
        // position, and the region restore emitted just before the positioning
        // homes the cursor, so leaving it out resumes the client at the origin
        // rather than where the program's cursor is. Whether it is DRAWN is
        // `?25`, which `mode_restore_sequence` restores separately.
        let cursor = term::point_to_viewport(0, renderable.cursor.point);

        let grid = self.term.grid();
        let cols = grid.columns();
        let full_top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        // Bound the replay so a very large configured scrollback can't make one
        // connect build an enormous buffer under the terminal lock. The default
        // (10_000) never trips this; when it does, the most recent lines are kept.
        let top = clamp_replay_top(full_top, bottom);
        if top != full_top {
            logger::debug(&format!(
                "reconnect replay truncated scrollback from {} to {} lines (cap {})",
                bottom - full_top + 1,
                bottom - top + 1,
                MAX_RECONNECT_REPLAY_LINES,
            ));
        }

        // Pre-size to history+screen rows so a large scrollback doesn't reallocate
        // repeatedly while we hold the terminal lock.
        let est_rows = (bottom - top + 1).max(0) as usize;
        let mut out = String::with_capacity(est_rows * (cols + 2) + 32);
        // Ensure the primary buffer and autowrap-on (so soft-wrapped rows can be
        // rebuilt by the client), widen the scrolling region back to the whole
        // screen, then clear the screen, clear the client's saved scrollback (3J),
        // and home the cursor. Clearing scrollback makes a reconnect idempotent:
        // we rebuild from the authoritative grid rather than appending a second
        // copy of the history. The widening is load bearing rather than defensive:
        // this replay rebuilds the buffer by printing lines and letting them
        // scroll off the top, and a line only reaches the client's scrollback when
        // the scrolling region starts at the FIRST row. A bottom margin alone is
        // fine (a program pinning a status bar keeps its scrollback, which
        // `scroll_region_with_bottom_margin_still_captures_scrollback` asserts on
        // our own engine); a top margin is what would send every replayed line
        // into a pinned band instead of the history. Widening covers both without
        // having to ask which one the client has.
        //
        // Origin mode goes off in the same breath. The replay addresses the cursor
        // absolutely, and origin mode would make those coordinates relative to the
        // top margin the replay is about to set, landing the cursor one whole
        // margin too low on a client that arrived with the flag already on.
        // Clearing it is not the same as restoring it, which stays out of scope;
        // it just guarantees the frame's own positioning means what it says.
        out.push_str("\x1b[?1049l\x1b[?7h\x1b[?6l\x1b[r\x1b[2J\x1b[3J\x1b[H");

        let mut last_style: Option<(CellColor, CellColor, CellModifier)> = None;
        // A soft-wrapped row carries `WRAPLINE` on its last cell. We replay such a
        // row at full width with NO line break and let the client's autowrap
        // re-create the soft wrap when the next row's first cell overflows — that
        // preserves copy/paste and resize-reflow semantics. A `\r\n` is emitted
        // only for genuine (hard) line breaks.
        let mut prev_wrapped = false;
        for line in top..=bottom {
            let row = &grid[Line(line)];
            let wrapped = cols > 0 && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            if line != top && !prev_wrapped {
                // Reset SGR before a hard line break while a non-default
                // background is still active. A `\r\n` at the bottom of the
                // screen scrolls, and a scroll fills the newly-exposed row with
                // the CURRENT background color (Background-Color-Erase). Without
                // this reset the previous line's background bleeds onto the next
                // line on the client — even though we re-emit SGR for the next
                // line's first cell, the bleed has already happened during the
                // scroll. Soft-wrapped rows intentionally skip this so a colored
                // background continues across the wrap.
                if matches!(last_style, Some((_, bg, _)) if bg != CellColor::Reset) {
                    out.push_str("\x1b[0m");
                    last_style = None;
                }
                out.push_str("\r\n");
            }
            // Right-trim trailing empty default cells so we don't emit a screen's
            // worth of spaces per line. `Cell::is_empty` is false for colored
            // (non-default-background) spaces, so visible trailing blocks survive.
            // A wrapped row keeps its full width — the trailing cells are load
            // bearing for the autowrap to fire at the right column.
            let emit_to = if wrapped {
                cols
            } else {
                let mut last_col = 0usize;
                for c in 0..cols {
                    if !row[Column(c)].is_empty() {
                        last_col = c + 1;
                    }
                }
                last_col
            };
            for c in 0..emit_to {
                let cell = &row[Column(c)];
                // The trailing spacer of a wide char carries no glyph; the wide
                // char itself (at the previous column) holds the symbol.
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let fg = convert_terminal_color(cell.fg, colors);
                let bg = convert_terminal_color(cell.bg, colors);
                let modifier = cell_modifier(cell);
                let style = (fg, bg, modifier);
                if last_style != Some(style) {
                    out.push_str("\x1b[0m");
                    out.push_str(&sgr_sequence(fg, bg, modifier));
                    last_style = Some(style);
                }
                // A tab cell stores a literal '\t' as a one-column anchor (the
                // span's fill spaces follow in later cells); emitting it raw would
                // make the client interpret a tab control and jump columns. Map
                // any C0 control to a space — only '\t' is reachable here, but
                // this is defensively safe for all of them.
                out.push(if cell.c < ' ' { ' ' } else { cell.c });
                if let Some(zerowidth) = cell.zerowidth() {
                    for ch in zerowidth {
                        out.push(*ch);
                    }
                }
            }
            prev_wrapped = wrapped;
        }

        out.push_str("\x1b[0m");
        // The region goes back after the printing that needed the whole screen and
        // before the cursor, because setting a region homes the cursor.
        out.push_str(&self.scroll_region.region().decstbm_sequence());
        if let Some(point) = cursor {
            out.push_str(&format!("\x1b[{};{}H", point.line + 1, point.column.0 + 1));
        }
        // Last, so the `?7h` this replay forced on above (to rebuild soft-wrapped
        // rows through the client's own autowrap) is put back to whatever the
        // child actually has, alongside the rest of its private modes.
        out.push_str(&mode_restore_sequence(*self.term.mode()));
        out.into_bytes()
    }

    fn scrollback_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    fn scroll(&mut self, up: bool, amount: usize) {
        let delta = if up { amount as i32 } else { -(amount as i32) };
        self.term.scroll_display(Scroll::Delta(delta));
    }

    fn set_scrollback(&mut self, rows: usize) {
        let current = self.term.grid().display_offset();
        let target = rows.min(self.term.grid().history_size());
        let delta = target as i32 - current as i32;
        self.term.scroll_display(Scroll::Delta(delta));
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
        self.event_proxy.set_size(rows, cols);
        self.term.resize(TerminalDimensions::new(rows, cols));
        // A resize widens the engine's region back to the whole screen at the new
        // height. Follow it, or the tracker keeps reporting margins the child no
        // longer has. Both dimensions go across because the engine skips its own
        // region reset when neither changed, and the tracker mirrors that skip.
        self.scroll_region.resize(rows, cols);
    }
}

#[derive(Clone)]
struct EventProxy {
    pending: Arc<Mutex<PendingEvents>>,
    size: Arc<Mutex<(u16, u16)>>,
}

impl EventProxy {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            pending: Arc::new(Mutex::new(PendingEvents::default())),
            size: Arc::new(Mutex::new((rows, cols))),
        }
    }

    fn push_bytes(&self, bytes: &[u8]) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.bytes.extend_from_slice(bytes);
        }
    }

    fn push_color_request(&self, index: usize, formatter: ColorRequestFormatter) {
        if let Ok(mut pending) = self.pending.lock() {
            pending
                .color_requests
                .push(PendingColorRequest { index, formatter });
        }
    }

    fn take_pending(&self) -> PendingEvents {
        self.pending
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }

    fn set_size(&self, rows: u16, cols: u16) {
        if let Ok(mut size) = self.size.lock() {
            *size = (rows, cols);
        }
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.push_bytes(text.as_bytes()),
            // The terminal ding (`Event::Bell`) is deliberately NOT handled here.
            // The raw-byte `AttentionScanner` in the reader loop is the single
            // bell-detection path, because it is already the ONLY path for the
            // notification and progress sequences this enum has no variant for.
            // Taking the bell from both places would arm the flag twice for one
            // ding and split one signal set across two mechanisms.
            Event::ColorRequest(index, formatter) => self.push_color_request(index, formatter),
            Event::TextAreaSizeRequest(formatter) => {
                let (rows, cols) = self.size.lock().map(|size| *size).unwrap_or((24, 80));
                let response = formatter(WindowSize {
                    num_lines: rows,
                    num_cols: cols,
                    cell_width: 0,
                    cell_height: 0,
                });
                self.push_bytes(response.as_bytes());
            }
            _ => {}
        }
    }
}

type ColorRequestFormatter = Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>;

#[derive(Default)]
struct PendingEvents {
    bytes: Vec<u8>,
    color_requests: Vec<PendingColorRequest>,
}

struct PendingColorRequest {
    index: usize,
    formatter: ColorRequestFormatter,
}

struct TerminalDimensions {
    rows: usize,
    cols: usize,
}

impl TerminalDimensions {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows: usize::from(rows),
            cols: usize::from(cols),
        }
    }
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// Translate an alacritty cell's style flags into our serializable
/// `CellModifier`. Shared by the per-frame snapshot and the reconnect repaint.
fn cell_modifier(cell: &Cell) -> CellModifier {
    let mut modifier = CellModifier::default();
    if cell.flags.contains(Flags::BOLD) {
        modifier.bold = true;
    }
    if cell.flags.contains(Flags::ITALIC) {
        modifier.italic = true;
    }
    if cell.flags.intersects(Flags::ALL_UNDERLINES) {
        modifier.underlined = true;
    }
    if cell.flags.contains(Flags::INVERSE) {
        modifier.reversed = true;
    }
    if cell.flags.contains(Flags::DIM) {
        modifier.dim = true;
    }
    if cell.flags.contains(Flags::STRIKEOUT) {
        modifier.crossed_out = true;
    }
    modifier
}

fn convert_terminal_color(
    color: TermColor,
    palette: &alacritty_terminal::term::color::Colors,
) -> CellColor {
    match color {
        TermColor::Spec(Rgb { r, g, b }) => CellColor::Rgb(r, g, b),
        TermColor::Indexed(index) => palette[index as usize]
            .map(|rgb| CellColor::Rgb(rgb.r, rgb.g, rgb.b))
            .unwrap_or(CellColor::Indexed(index)),
        TermColor::Named(named) => palette[named]
            .map(|rgb| CellColor::Rgb(rgb.r, rgb.g, rgb.b))
            .unwrap_or_else(|| named_color_to_tui(named)),
    }
}

fn named_color_to_tui(color: NamedColor) -> CellColor {
    match color {
        NamedColor::Black => CellColor::Indexed(0),
        NamedColor::Red => CellColor::Indexed(1),
        NamedColor::Green => CellColor::Indexed(2),
        NamedColor::Yellow => CellColor::Indexed(3),
        NamedColor::Blue => CellColor::Indexed(4),
        NamedColor::Magenta => CellColor::Indexed(5),
        NamedColor::Cyan => CellColor::Indexed(6),
        NamedColor::White => CellColor::Indexed(7),
        NamedColor::BrightBlack => CellColor::Indexed(8),
        NamedColor::BrightRed => CellColor::Indexed(9),
        NamedColor::BrightGreen => CellColor::Indexed(10),
        NamedColor::BrightYellow => CellColor::Indexed(11),
        NamedColor::BrightBlue => CellColor::Indexed(12),
        NamedColor::BrightMagenta => CellColor::Indexed(13),
        NamedColor::BrightCyan => CellColor::Indexed(14),
        NamedColor::BrightWhite => CellColor::Indexed(15),
        NamedColor::DimBlack => CellColor::Indexed(0),
        NamedColor::DimRed => CellColor::Indexed(1),
        NamedColor::DimGreen => CellColor::Indexed(2),
        NamedColor::DimYellow => CellColor::Indexed(3),
        NamedColor::DimBlue => CellColor::Indexed(4),
        NamedColor::DimMagenta => CellColor::Indexed(5),
        NamedColor::DimCyan => CellColor::Indexed(6),
        NamedColor::DimWhite => CellColor::Indexed(7),
        NamedColor::Foreground
        | NamedColor::Background
        | NamedColor::Cursor
        | NamedColor::BrightForeground
        | NamedColor::DimForeground => CellColor::Reset,
    }
}

/// The bundle of optional spawn inputs for [`PtyClient::spawn_with_env_opts`],
/// folded into a struct so the argument list stays readable as it grows.
/// Borrows so the common `spawn_with_env` path passes a temporary empty identity
/// with no allocation.
pub struct PtySpawnOptions<'a> {
    /// The caller's `[env]` overrides, applied last so a user value always wins.
    pub env: &'a [(String, String)],
    /// Whether the reader loop runs the attention/passthrough scanner (agents:
    /// true; companion shells: false).
    pub track_agent_signals: bool,
    /// The terminal-identity env mutation to apply before the caller env.
    pub identity: &'a crate::term_identity::TerminalIdentity,
}

/// Apply a resolved terminal identity to a spawn command: remove first, then set.
/// A `remove` entry ending in `*` scrubs every inherited variable whose name
/// starts with the prefix, expanded against the real process environment (the one
/// the child would otherwise inherit) since `env_remove` takes a concrete name.
fn apply_identity_env(cmd: &mut CommandBuilder, identity: &crate::term_identity::TerminalIdentity) {
    // Snapshot the ambient environment once per spawn and delegate to the pure
    // helper. Passing the ambient set as a parameter keeps the prefix-scrub logic
    // testable without mutating (and racing on) the process-wide environment.
    let ambient: Vec<(std::ffi::OsString, std::ffi::OsString)> = env::vars_os().collect();
    apply_identity_env_with(cmd, identity, &ambient);
}

/// The pure core of [`apply_identity_env`]: apply `identity` against an explicit
/// `ambient` environment snapshot rather than reading `std::env` directly, so a
/// unit test can supply a fabricated ambient set and never touch the process env.
fn apply_identity_env_with(
    cmd: &mut CommandBuilder,
    identity: &crate::term_identity::TerminalIdentity,
    ambient: &[(std::ffi::OsString, std::ffi::OsString)],
) {
    for name in &identity.remove {
        if let Some(prefix) = name.strip_suffix('*') {
            for (key, _) in ambient {
                if let Some(key) = key.to_str()
                    && key.starts_with(prefix)
                {
                    cmd.env_remove(key);
                }
            }
        } else {
            cmd.env_remove(name);
        }
    }
    for (name, value) in &identity.set {
        cmd.env(name, value);
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

fn resolve_color_request_rgb(
    index: usize,
    palette: &alacritty_terminal::term::color::Colors,
) -> Rgb {
    (index < alacritty_terminal::term::color::COUNT)
        .then(|| palette[index])
        .flatten()
        .or_else(|| default_palette_rgb(index))
        .unwrap_or(Rgb {
            r: 0x00,
            g: 0x00,
            b: 0x00,
        })
}

fn default_palette_rgb(index: usize) -> Option<Rgb> {
    match index {
        0 => Some(rgb(0x00, 0x00, 0x00)),
        1 => Some(rgb(0xcd, 0x00, 0x00)),
        2 => Some(rgb(0x00, 0xcd, 0x00)),
        3 => Some(rgb(0xcd, 0xcd, 0x00)),
        4 => Some(rgb(0x00, 0x00, 0xee)),
        5 => Some(rgb(0xcd, 0x00, 0xcd)),
        6 => Some(rgb(0x00, 0xcd, 0xcd)),
        7 => Some(rgb(0xe5, 0xe5, 0xe5)),
        8 => Some(rgb(0x7f, 0x7f, 0x7f)),
        9 => Some(rgb(0xff, 0x00, 0x00)),
        10 => Some(rgb(0x00, 0xff, 0x00)),
        11 => Some(rgb(0xff, 0xff, 0x00)),
        12 => Some(rgb(0x5c, 0x5c, 0xff)),
        13 => Some(rgb(0xff, 0x00, 0xff)),
        14 => Some(rgb(0x00, 0xff, 0xff)),
        15 => Some(rgb(0xff, 0xff, 0xff)),
        16..=231 => Some(xterm_color_cube(index)),
        232..=255 => Some(xterm_grayscale(index)),
        x if x == NamedColor::Foreground as usize => Some(rgb(0xff, 0xff, 0xff)),
        x if x == NamedColor::Background as usize => Some(rgb(0x00, 0x00, 0x00)),
        x if x == NamedColor::Cursor as usize => Some(rgb(0xff, 0xff, 0xff)),
        x if x == NamedColor::DimBlack as usize => Some(rgb(0x00, 0x00, 0x00)),
        x if x == NamedColor::DimRed as usize => Some(rgb(0x80, 0x00, 0x00)),
        x if x == NamedColor::DimGreen as usize => Some(rgb(0x00, 0x80, 0x00)),
        x if x == NamedColor::DimYellow as usize => Some(rgb(0x80, 0x80, 0x00)),
        x if x == NamedColor::DimBlue as usize => Some(rgb(0x00, 0x00, 0x80)),
        x if x == NamedColor::DimMagenta as usize => Some(rgb(0x80, 0x00, 0x80)),
        x if x == NamedColor::DimCyan as usize => Some(rgb(0x00, 0x80, 0x80)),
        x if x == NamedColor::DimWhite as usize => Some(rgb(0x80, 0x80, 0x80)),
        x if x == NamedColor::BrightForeground as usize => Some(rgb(0xff, 0xff, 0xff)),
        x if x == NamedColor::DimForeground as usize => Some(rgb(0x80, 0x80, 0x80)),
        _ => None,
    }
}

fn xterm_color_cube(index: usize) -> Rgb {
    const STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

    let idx = index - 16;
    let r = STEPS[idx / 36];
    let g = STEPS[(idx / 6) % 6];
    let b = STEPS[idx % 6];
    rgb(r, g, b)
}

fn xterm_grayscale(index: usize) -> Rgb {
    let level = 8 + ((index - 232) as u8 * 10);
    rgb(level, level, level)
}

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use compact_str::CompactString;
    use portable_pty::CommandBuilder;

    /// The pgid guard behind `tcgetpgrp`: zero and negative raw values are
    /// invalid process-group ids and must map to `None`, never reach a
    /// `Pid`/`NonZero` construction. This is the exact hole that killed the
    /// engine thread in production: on macOS `tcgetpgrp` returns 0 for a PTY
    /// whose foreground process group is gone, and rustix 1.1.4 only guards
    /// that case on Linux before feeding the value to `from_raw_unchecked`
    /// (a debug-build assertion failure, an invalid NonZeroI32 in release).
    #[test]
    fn valid_pgid_rejects_zero_and_negative_ids() {
        assert_eq!(valid_pgid(0), None);
        assert_eq!(valid_pgid(-1), None);
        assert_eq!(valid_pgid(i32::MIN), None);
        assert_eq!(valid_pgid(1), Some(1));
        assert_eq!(valid_pgid(43210), Some(43210));
    }

    #[test]
    fn write_counts_as_typing_excludes_empty_focus_and_mouse_reports() {
        // Genuine input counts.
        assert!(write_counts_as_typing(b"a"));
        assert!(write_counts_as_typing(b"hello"));
        assert!(write_counts_as_typing(b"\r")); // Enter
        assert!(write_counts_as_typing(b"\x7f")); // Backspace
        assert!(write_counts_as_typing(b"\x1b[A")); // Up arrow is real input
        assert!(write_counts_as_typing(b"\x1b[Z")); // Shift-Tab is real input

        // Non-typing writes do not.
        assert!(!write_counts_as_typing(b"")); // empty frame
        assert!(!write_counts_as_typing(b"\x1b[I")); // focus in
        assert!(!write_counts_as_typing(b"\x1b[O")); // focus out
        assert!(!write_counts_as_typing(b"\x1b[<0;10;5M")); // SGR mouse press
        assert!(!write_counts_as_typing(b"\x1b[<64;10;5M")); // SGR wheel scroll
        assert!(!write_counts_as_typing(b"\x1b[M !!")); // legacy mouse
    }

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

    /// Wait until the PTY viewport shows `needle` (proving the reader thread has
    /// consumed the child's output), or panic after a bounded wait.
    fn wait_for_viewport(client: &PtyClient, needle: &str) {
        for _ in 0..50 {
            let snapshot = client.snapshot();
            if viewport_lines(&snapshot)
                .iter()
                .any(|line| line.contains(needle))
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("timed out waiting for {needle:?} in the PTY viewport");
    }

    // A one-shot child that emits, in order: an OSC 9;4 working progress report, an
    // OSC 9 notification, a bare bell, then a visible "X" sync marker.
    const SIGNAL_EMITTER: &str = "printf '\\033]9;4;1;50\\007\\033]9;hi\\007\\007X'";

    #[test]
    fn agent_signals_are_scanned_on_the_tracked_path() {
        // The raw-byte scanner (not the emulator) is the single detection path for
        // the bell, OSC notifications, and progress. On the agent (tracked) spawn
        // path all three land: a bare bell/notification arms `take_attention` and
        // a progress report populates `progress_report`.
        let args = vec!["-c".to_string(), SIGNAL_EMITTER.to_string()];
        let client = PtyClient::spawn_with_env_opts(
            "/bin/sh",
            &args,
            Path::new("."),
            5,
            40,
            100,
            PtySpawnOptions {
                env: &[],
                track_agent_signals: true,
                identity: &crate::term_identity::TerminalIdentity::default(),
            },
        )
        .expect("spawn pty");
        wait_for_viewport(&client, "X");

        let report = client.progress_report().expect("a progress report");
        assert!(report.working, "9;4;1 is a working progress state");
        assert!(
            client.take_attention(true),
            "the notification and bell must arm attention"
        );
    }

    #[test]
    fn agent_captures_passthrough_sequences() {
        // A clipboard SET plus a notification, then a sync marker.
        let emit = "printf '\\033]52;c;aGk=\\007\\033]9;hi\\007X'";
        let args = vec!["-c".to_string(), emit.to_string()];
        let client = PtyClient::spawn_with_env_opts(
            "/bin/sh",
            &args,
            Path::new("."),
            5,
            40,
            100,
            PtySpawnOptions {
                env: &[],
                track_agent_signals: true,
                identity: &crate::term_identity::TerminalIdentity::default(),
            },
        )
        .expect("spawn pty");
        wait_for_viewport(&client, "X");

        let caps = client.take_passthrough();
        let kinds: Vec<_> = caps.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&crate::attention::CapturedKind::ClipboardSet));
        assert!(kinds.contains(&crate::attention::CapturedKind::Notify));
        // Draining consumed the ring.
        assert!(client.take_passthrough().is_empty());
    }

    #[test]
    fn companion_captures_no_passthrough() {
        // With signal tracking off (a companion shell), nothing is captured.
        let emit = "printf '\\033]9;hi\\007\\033]52;c;aGk=\\007X'";
        let args = vec!["-c".to_string(), emit.to_string()];
        let client = PtyClient::spawn_with_env_opts(
            "/bin/sh",
            &args,
            Path::new("."),
            5,
            40,
            100,
            PtySpawnOptions {
                env: &[],
                track_agent_signals: false,
                identity: &crate::term_identity::TerminalIdentity::default(),
            },
        )
        .expect("spawn pty");
        wait_for_viewport(&client, "X");
        assert!(client.take_passthrough().is_empty());
    }

    #[test]
    fn passthrough_ring_is_capped_drop_oldest() {
        // Emit far more NUMBERED sequences than the ring holds, then a trailing X.
        // The ring must cap at PASSTHROUGH_RING_CAP and keep exactly the LAST
        // cap-worth (the oldest dropped), not merely stay under the cap.
        const TOTAL: usize = 200;
        let emit = format!(
            "for i in $(seq 1 {TOTAL}); do printf '\\033]9;n%d\\007' \"$i\"; done; printf X"
        );
        let args = vec!["-c".to_string(), emit];
        let client = PtyClient::spawn_with_env_opts(
            "/bin/sh",
            &args,
            Path::new("."),
            5,
            40,
            100,
            PtySpawnOptions {
                env: &[],
                track_agent_signals: true,
                identity: &crate::term_identity::TerminalIdentity::default(),
            },
        )
        .expect("spawn pty");
        // X is printed AFTER every sequence, so its visibility proves the reader
        // thread processed all TOTAL captures (the ring push precedes the terminal
        // write within each scan pass).
        wait_for_viewport(&client, "X");
        let drained = client.take_passthrough();
        assert_eq!(
            drained.len(),
            PASSTHROUGH_RING_CAP,
            "the ring must retain exactly its cap once flooded"
        );
        assert!(!drained.is_empty(), "the ring must not be vacuously empty");
        // The retained sequences are the LAST cap-worth: n(TOTAL-cap+1)..=n(TOTAL).
        let first_kept = TOTAL - PASSTHROUGH_RING_CAP + 1;
        assert_eq!(
            drained.first().map(|s| s.bytes.clone()),
            Some(format!("\x1b]9;n{first_kept}\x1b\\").into_bytes()),
            "oldest retained sequence must be n{first_kept} (older ones dropped)"
        );
        assert_eq!(
            drained.last().map(|s| s.bytes.clone()),
            Some(format!("\x1b]9;n{TOTAL}\x1b\\").into_bytes()),
            "newest retained sequence must be n{TOTAL}"
        );
    }

    #[test]
    fn bell_alone_is_gated_by_count_bell() {
        // A bare bell (no notification) must only arm attention when the caller
        // opts to count bells, mirroring the `attention_on_bell` preference.
        let args = vec!["-c".to_string(), "printf '\\007X'".to_string()];
        let client = PtyClient::spawn_with_env_opts(
            "/bin/sh",
            &args,
            Path::new("."),
            5,
            40,
            100,
            PtySpawnOptions {
                env: &[],
                track_agent_signals: true,
                identity: &crate::term_identity::TerminalIdentity::default(),
            },
        )
        .expect("spawn pty");
        wait_for_viewport(&client, "X");

        // count_bell = false: the bell does not arm attention (and it is drained).
        assert!(
            !client.take_attention(false),
            "a bell must be ignored when bells are not counted"
        );
    }

    #[test]
    fn companion_terminals_never_scan_agent_signals() {
        // Companion terminals spawn with tracking OFF: the very same bytes must
        // leave `take_attention` and `progress_report` inert, so a plain shell can
        // never raise a spurious attention flag or working override.
        let args = vec!["-c".to_string(), SIGNAL_EMITTER.to_string()];
        let client = PtyClient::spawn_with_env_opts(
            "/bin/sh",
            &args,
            Path::new("."),
            5,
            40,
            100,
            PtySpawnOptions {
                env: &[],
                track_agent_signals: false,
                identity: &crate::term_identity::TerminalIdentity::default(),
            },
        )
        .expect("spawn pty");
        wait_for_viewport(&client, "X");

        assert!(
            client.progress_report().is_none(),
            "an untracked companion must record no progress"
        );
        assert!(
            !client.take_attention(true),
            "an untracked companion must never arm attention"
        );
    }

    #[test]
    fn take_content_change_fires_on_text_not_on_osc_or_cursor() {
        let mut terminal = TerminalState::with_scrollback(4, 20, 100);

        // Real printed text is a visible change.
        terminal.process(b"hello");
        assert!(
            terminal.take_content_change(),
            "printing text must register a visible change"
        );

        // Processing bytes that don't alter the grid content is NOT a change:
        // an OSC 9;4 progress report (the agent's own status signal)...
        terminal.process(b"\x1b]9;4;1;50\x1b\\");
        assert!(
            !terminal.take_content_change(),
            "an OSC 9;4 progress report changes no visible content"
        );

        // ...and a bare cursor move (no cell content written).
        terminal.process(b"\x1b[2;3H");
        assert!(
            !terminal.take_content_change(),
            "a cursor move writes no visible content"
        );

        // Printing more text is a change again.
        terminal.process(b" world");
        assert!(
            terminal.take_content_change(),
            "printing more text must register a visible change"
        );
    }

    #[test]
    fn scrollback_reaches_beyond_visible_rows() {
        let mut terminal = TerminalState::with_scrollback(3, 12, 100);
        terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");

        assert_eq!(terminal.term.grid().history_size(), 3);

        terminal.set_scrollback(terminal.term.grid().history_size());
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
        terminal.set_scrollback(terminal.term.grid().history_size());

        terminal.process(b"six\r\nseven\r\n");
        let snapshot = terminal.snapshot();

        assert!(snapshot.scrollback_offset <= terminal.term.grid().history_size());
        assert_eq!(
            snapshot.scrollback_total,
            terminal.term.grid().history_size()
        );
        assert!(terminal.term.grid().history_size() >= 5);
    }

    #[test]
    fn visible_change_fires_while_scrolled_past_the_viewport() {
        // The question `take_content_change` answers is "did the child produce
        // visible output", which is a property of the terminal and not of where
        // the human is looking. This is exactly the state the live path is in
        // whenever the operator reads history while the child keeps talking,
        // which the reader no longer stops parsing for; driving `TerminalState`
        // directly just pins the property without spawning a PTY.
        let mut terminal = TerminalState::with_scrollback(3, 16, 100);
        terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\n");

        // Scroll back FURTHER than one screen height, so the displayed viewport
        // is entirely immutable history.
        let history = terminal.term.grid().history_size();
        assert!(
            history > terminal.term.screen_lines(),
            "precondition: more history than one screen ({history} vs {})",
            terminal.term.screen_lines()
        );
        terminal.set_scrollback(history);
        // Prime the fingerprint so the assertion below is about the new output.
        terminal.take_content_change();

        terminal.process(b"eight\r\n");

        assert!(
            terminal.take_content_change(),
            "output printed while the user reads history is still the agent working"
        );
    }

    #[test]
    fn region_scroll_while_scrolled_back_keeps_offset_renderable() {
        // Regression for a panic inside alacritty_terminal 0.26.0: its
        // `Grid::scroll_up` bumps the display offset for every scroll while the
        // offset is non-zero, but only pushes to history when the region starts
        // at row zero, and it clamps to `max_scroll_limit` rather than to the
        // current history size. A top-margin region scrolled while the user is
        // scrolled back and the history ring is unsaturated therefore drives the
        // offset past the history size, and the next render panics in
        // `grid/storage.rs`.
        let mut terminal = TerminalState::with_scrollback(5, 16, 1000);
        terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\n");
        let history = terminal.term.grid().history_size();
        assert!(history > 0, "precondition: some history exists");
        assert!(
            history < 1000,
            "precondition: the history ring is unsaturated ({history})"
        );
        terminal.set_scrollback(history);

        // A scrolling region with a TOP margin (rows 2..5), then scroll it.
        terminal.process(b"\x1b[2;5r");
        terminal.process(b"\x1b[5;1H");
        for _ in 0..8 {
            terminal.process(b"\n");
        }

        let history = terminal.term.grid().history_size();
        let offset = terminal.term.grid().display_offset();
        assert!(
            offset <= history,
            "display offset {offset} must stay within history size {history}"
        );
        // The panic lives in the render path, not in the number, so prove the
        // render completes too.
        let snapshot = terminal.snapshot();
        assert!(snapshot.scrollback_offset <= snapshot.scrollback_total);
    }

    #[test]
    fn reconnect_repaint_includes_scrollback_history() {
        let mut terminal = TerminalState::with_scrollback(4, 20, 1000);
        for i in 0..12 {
            terminal.process(format!("line{i}\r\n").as_bytes());
        }
        assert!(
            terminal.term.grid().history_size() > 0,
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
        let viewport_only = String::from_utf8(synthesize_repaint(
            &terminal.snapshot(),
            false,
            ScrollRegion::full(4),
        ))
        .unwrap();
        assert!(
            !viewport_only.contains("line0"),
            "sanity: viewport-only repaint omits scrolled-off history"
        );
    }

    /// Read the scrolling region a live terminal is ACTUALLY using, without an
    /// accessor for it.
    ///
    /// Origin mode is the lever: while it is on, the engine resolves a row
    /// coordinate against the region instead of the screen, clamping it into
    /// `start ..= end - 1`. So homing the cursor lands it on the region's first
    /// row, and asking for a row far past the bottom lands it on the region's
    /// last. Reading the cursor back after each gives both margins exactly, with
    /// no scrolling and without disturbing a single cell. Origin mode is turned
    /// back off afterwards so the probe leaves no state behind.
    ///
    /// This is deliberately a behavioural reading rather than a second copy of the
    /// tracker's arithmetic: if it were the same arithmetic it could agree with
    /// the tracker while both disagreed with the terminal, which is the failure
    /// the test exists to catch.
    ///
    /// It resolves every region a program can actually end up with, but not an
    /// EMPTY one. The engine clamps a top margin to the screen height rather than
    /// to the last row, so a region asked for entirely below the screen collapses
    /// to `start == end`; the same clamp then pins any probe row to `end - 1`, so
    /// an empty region reads back exactly like the one-row region above it. That
    /// is a property of the engine and not of this probe, and it is covered
    /// separately by `an_empty_region_is_reported_and_written_as_the_whole_screen`.
    fn probe_live_scroll_region(terminal: &mut TerminalState) -> (i32, i32) {
        let cursor_line = |terminal: &TerminalState| terminal.term.grid().cursor.point.line.0;

        // `?6h` homes the cursor as it engages, which under origin mode is the top
        // margin.
        terminal.process(b"\x1b[?6h");
        let start = cursor_line(terminal);
        // Far past any plausible screen height, so it clamps to the bottom margin.
        terminal.process(b"\x1b[9999;1H");
        let last = cursor_line(terminal);
        terminal.process(b"\x1b[?6l");

        (start, last + 1)
    }

    /// Drive one byte sequence through a real terminal and through the tracker,
    /// and assert they end up with the same scrolling region.
    fn assert_region_agrees(label: &str, rows: u16, steps: &[&[u8]]) {
        let mut terminal = TerminalState::with_scrollback(rows, 20, 100);
        for step in steps {
            terminal.process(step);
        }

        let tracked = terminal.scroll_region.region();
        let (live_start, live_end) = probe_live_scroll_region(&mut terminal);

        assert_eq!(
            (tracked.start, tracked.end),
            (live_start, live_end),
            "{label}: the tracked scrolling region must match the one the terminal is really using"
        );
        assert_eq!(
            tracked.screen_lines,
            i32::from(rows),
            "{label}: the tracked region must be measured against the current screen height"
        );
    }

    #[test]
    fn tracked_scroll_region_agrees_with_the_terminal() {
        // Nothing written at all: both sides start at the whole screen.
        assert_region_agrees("construction", 24, &[]);
        // Ordinary output moves nothing.
        assert_region_agrees("plain output", 24, &[b"hello\r\nworld\r\n"]);
        // A program pinning a header and a footer.
        assert_region_agrees("explicit set", 24, &[b"\x1b[3;20r"]);
        // The bottom margin omitted means the last row.
        assert_region_agrees("open bottom margin", 24, &[b"\x1b[5r"]);
        // A region set, then narrowed again.
        assert_region_agrees("set twice", 24, &[b"\x1b[3;20r", b"\x1b[8;12r"]);
        // A bottom margin past the last row is clamped to the screen height.
        assert_region_agrees("bottom margin past the screen", 24, &[b"\x1b[3;40r"]);
        // An inverted pair is refused outright and leaves the region alone.
        assert_region_agrees(
            "inverted pair ignored",
            24,
            &[b"\x1b[10;12r", b"\x1b[20;5r"],
        );
        // The whole screen written out explicitly.
        assert_region_agrees("explicit whole screen", 24, &[b"\x1b[1;24r"]);
        // A full reset (RIS) widens the region back to the whole screen. An
        // observer watching only the explicit set reports the stale margins here.
        assert_region_agrees("full reset after a set", 24, &[b"\x1b[3;20r", b"\x1bc"]);
        // Split across chunk boundaries, mid-escape, which is the shape real PTY
        // reads arrive in.
        assert_region_agrees("split across reads", 24, &[b"\x1b[3", b";2", b"0r"]);
        // Bracketed by a synchronized update, so the bytes travel through both
        // parsers' sync buffering.
        assert_region_agrees(
            "inside a synchronized update",
            24,
            &[b"\x1b[?2026h\x1b[4;18rtext\x1b[?2026l"],
        );
        // Column mode (DECCOLM). The engine widens the region back to the whole
        // screen as one of the sequence's side effects, and it does so by calling
        // its own handler method directly rather than through the parser, so no
        // callback carries it. Both polarities run the same side effects.
        assert_region_agrees(
            "column mode set after a set",
            24,
            &[b"\x1b[3;20r", b"\x1b[?3h"],
        );
        assert_region_agrees(
            "column mode unset after a set",
            24,
            &[b"\x1b[3;20r", b"\x1b[?3l"],
        );
    }

    #[test]
    fn an_empty_region_is_reported_and_written_as_the_whole_screen() {
        // Both margins below the last row. The engine clamps each to the screen
        // height (not to the last row), so the region collapses to `24..24` and
        // scrolls nothing at all. The tracker reproduces that exactly, and the
        // repaint writes it as the whole-screen reset, because there is no DECSTBM
        // spelling for an empty region and the inverted pair that would describe
        // it is one a client throws away.
        let mut terminal = TerminalState::with_scrollback(24, 20, 100);
        terminal.process(b"\x1b[30;40r");

        let tracked = terminal.scroll_region.region();
        assert_eq!((tracked.start, tracked.end), (24, 24));
        assert!(!tracked.is_full_screen());
        assert_eq!(tracked.decstbm_sequence(), "\x1b[r");
    }

    #[test]
    fn tracked_scroll_region_agrees_with_the_terminal_across_a_resize() {
        // A resize widens the engine's region back to the whole screen at the new
        // height, and nothing in the byte stream says so. An observer that watches
        // only the bytes reports the pre-resize margins forever after.
        let mut terminal = TerminalState::with_scrollback(24, 20, 100);
        terminal.process(b"\x1b[3;20r");
        terminal.resize(10, 20);

        let tracked = terminal.scroll_region.region();
        let (live_start, live_end) = probe_live_scroll_region(&mut terminal);

        assert_eq!(
            (tracked.start, tracked.end),
            (live_start, live_end),
            "a resize must move the tracked region exactly as it moves the terminal's"
        );
        assert!(
            tracked.is_full_screen(),
            "a resize widens the region back to the whole screen, got {tracked:?}"
        );

        // And a region set after the resize is measured against the new height.
        terminal.process(b"\x1b[2;9r");
        let tracked = terminal.scroll_region.region();
        let (live_start, live_end) = probe_live_scroll_region(&mut terminal);
        assert_eq!((tracked.start, tracked.end), (live_start, live_end));
    }

    #[test]
    fn a_resize_to_the_size_already_in_effect_leaves_the_region_alone() {
        // The engine returns from a resize before touching its region when both
        // dimensions already match, so a same-size resize moves nothing there and
        // must move nothing here either. This is not a corner case: a browser
        // client sends its size on every reconnect, every tab focus, every
        // visibility change and every input claim, and almost all of those are
        // the size already in effect. A tracker that widened on each of them
        // would report the whole screen while the child still had its margins,
        // and the next reconnect would then clobber a region the program still
        // has, which is worse than not restoring one at all.
        let mut terminal = TerminalState::with_scrollback(24, 20, 100);
        terminal.process(b"\x1b[3;20r");
        terminal.resize(24, 20);

        let tracked = terminal.scroll_region.region();
        let (live_start, live_end) = probe_live_scroll_region(&mut terminal);
        assert_eq!(
            (tracked.start, tracked.end),
            (live_start, live_end),
            "a same-size resize must leave the tracked region where the terminal still has it"
        );
        assert_eq!(
            (tracked.start, tracked.end),
            (2, 20),
            "the region the program set must survive a same-size resize, got {tracked:?}"
        );

        // A change in COLUMNS alone still resets the engine's region, so a
        // rows-only comparison is not enough to decide this.
        terminal.resize(24, 40);
        let tracked = terminal.scroll_region.region();
        let (live_start, live_end) = probe_live_scroll_region(&mut terminal);
        assert_eq!(
            (tracked.start, tracked.end),
            (live_start, live_end),
            "a columns-only resize must widen the tracked region exactly as it widens the terminal's"
        );
        assert!(
            tracked.is_full_screen(),
            "a columns-only resize widens the region back to the whole screen, got {tracked:?}"
        );
    }

    /// What the engine does to its scrolling region at one of its region-moving
    /// sites.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RegionMove {
        /// Widens back to every row of the current screen.
        WidensToWholeScreen,
        /// Narrows to this half-open row range.
        NarrowsTo(i32, i32),
        /// Leaves the region exactly where it already was.
        LeavesItAlone,
    }

    /// Every place the terminal engine moves its scrolling region, with the
    /// trigger that reaches it and what it does when it gets there.
    ///
    /// This list, not the byte sequences in
    /// `tracked_scroll_region_agrees_with_the_terminal`, is what gives the mirror
    /// its coverage. Those cases all drive sequences the observer already
    /// implements a callback for, so they can only ever confirm that a door the
    /// observer is watching is still watched. The failure this design is actually
    /// exposed to is the opposite one: the engine moving its region through a
    /// door the observer is NOT watching, which is exactly how the column-mode
    /// site was missed. Enumerating the sites is what opens those doors.
    ///
    /// The sites were read out of the dependency by finding every assignment to
    /// the engine's private region field and every caller that reaches one:
    /// construction, resize, full reset (RIS), the explicit DECSTBM set, and
    /// column mode (DECCOLM), which reaches the explicit set by an internal call
    /// that bypasses the parser. Be honest about what this test can do: it cannot
    /// discover a SIXTH site on its own, because a site nobody has named has no
    /// trigger to drive. What it does is make the set explicit and checkable, so
    /// a dependency bump is a matter of re-reading the field's assignments
    /// against this list rather than guessing.
    fn engine_region_moving_sites() -> Vec<(&'static str, &'static [u8], RegionMove)> {
        vec![
            // The explicit DECSTBM set, the one site a program drives on purpose.
            (
                "explicit set (DECSTBM)",
                b"\x1b[8;12r",
                RegionMove::NarrowsTo(7, 12),
            ),
            // A full reset. Widens, and says nothing else about it.
            (
                "full reset (RIS)",
                b"\x1bc",
                RegionMove::WidensToWholeScreen,
            ),
            // Column mode, both polarities. Widens as a side effect, through an
            // internal call the parser never sees.
            (
                "column mode set (DECCOLM)",
                b"\x1b[?3h",
                RegionMove::WidensToWholeScreen,
            ),
            (
                "column mode unset (DECCOLM)",
                b"\x1b[?3l",
                RegionMove::WidensToWholeScreen,
            ),
            // A sequence that moves no region at all, so a case that passed by
            // doing nothing would be caught by the assertion that the engine
            // really moved.
            ("ordinary output", b"hello\r\n", RegionMove::LeavesItAlone),
        ]
    }

    #[test]
    fn the_tracker_follows_the_engine_at_every_site_that_moves_the_region() {
        // Construction, the one site with no trigger to drive: a terminal that has
        // been written to not at all starts at the whole screen on both sides.
        let mut terminal = TerminalState::with_scrollback(24, 20, 100);
        let tracked = terminal.scroll_region.region();
        assert_eq!(tracked, ScrollRegion::full(24));
        assert_eq!(
            (tracked.start, tracked.end),
            probe_live_scroll_region(&mut terminal),
            "construction: the tracker and the terminal must start on the same region"
        );

        // Resize, the other site with no byte sequence behind it, in both of its
        // outcomes. Covered in full by its own two tests, named here so this list
        // is not read as the complete set.
        //
        // - `tracked_scroll_region_agrees_with_the_terminal_across_a_resize`
        // - `a_resize_to_the_size_already_in_effect_leaves_the_region_alone`

        for (label, trigger, expected) in engine_region_moving_sites() {
            let mut terminal = TerminalState::with_scrollback(24, 20, 100);
            // Narrow first, so a site that widens has something to widen from and
            // a site that changes nothing has something to preserve.
            terminal.process(b"\x1b[3;20r");
            assert_eq!(
                probe_live_scroll_region(&mut terminal),
                (2, 20),
                "{label}: sanity, the setup must actually narrow the terminal's region"
            );

            terminal.process(trigger);

            let live = probe_live_scroll_region(&mut terminal);
            let expected_live = match expected {
                RegionMove::WidensToWholeScreen => (0, 24),
                RegionMove::NarrowsTo(start, end) => (start, end),
                RegionMove::LeavesItAlone => (2, 20),
            };
            // Assert what the ENGINE did first. If a dependency bump changes a
            // site's behaviour, this is what says so, rather than the two sides
            // quietly agreeing on something new.
            assert_eq!(
                live, expected_live,
                "{label}: the terminal did not move its region the way this site says it does"
            );

            let tracked = terminal.scroll_region.region();
            assert_eq!(
                (tracked.start, tracked.end),
                live,
                "{label}: the tracker must follow the engine through this site"
            );
        }
    }

    #[test]
    fn reconnect_repaint_restores_the_scroll_region_on_the_alt_screen() {
        let mut terminal = TerminalState::with_scrollback(6, 20, 100);
        terminal.process(b"\x1b[?1049h");
        terminal.process(b"\x1b[2;5r");
        terminal.process(b"\x1b[1;1Hheader");
        // A cursor position no painting step would emit on its own, so the
        // ordering assertions below cannot latch onto the wrong sequence.
        terminal.process(b"\x1b[4;3H");

        let replay = String::from_utf8(terminal.reconnect_repaint()).unwrap();
        let region_at = replay.find("\x1b[2;5r").unwrap_or_else(|| {
            panic!("the replay must re-assert the scroll region, got:\n{replay:?}")
        });
        let cells_at = replay
            .find("header")
            .expect("the replay must paint the cells");
        let cursor_at = replay
            .find("\x1b[4;3H")
            .expect("the replay must place the cursor");

        assert!(
            cells_at < region_at,
            "the cells are painted with absolute addressing, so the region must come after them: {replay:?}"
        );
        assert!(
            region_at < cursor_at,
            "setting the region homes the cursor, so the cursor must come after it: {replay:?}"
        );
        assert!(
            replay[..cells_at].contains("\x1b[r"),
            "the replay must widen to the whole screen before painting: {replay:?}"
        );
    }

    #[test]
    fn reconnect_repaint_restores_the_scroll_region_on_the_main_screen() {
        let mut terminal = TerminalState::with_scrollback(6, 20, 100);
        terminal.process(b"one\r\ntwo\r\n");
        terminal.process(b"\x1b[2;5r");
        terminal.process(b"\x1b[4;3H");

        let replay = String::from_utf8(terminal.reconnect_repaint()).unwrap();
        let region_at = replay.find("\x1b[2;5r").unwrap_or_else(|| {
            panic!("the replay must re-assert the scroll region, got:\n{replay:?}")
        });
        let text_at = replay
            .find("two")
            .expect("the replay must print the history");
        let cursor_at = replay
            .find("\x1b[4;3H")
            .expect("the replay must place the cursor");

        assert!(
            text_at < region_at,
            "the history is printed and allowed to scroll, so the region must come after it: {replay:?}"
        );
        assert!(
            region_at < cursor_at,
            "setting the region homes the cursor, so the cursor must come after it: {replay:?}"
        );
        assert!(
            replay[..text_at].contains("\x1b[r"),
            "the replay must widen to the whole screen before printing: {replay:?}"
        );
    }

    #[test]
    fn reconnect_repaint_widens_to_the_whole_screen_when_the_child_has_no_region() {
        let mut terminal = TerminalState::with_scrollback(6, 20, 100);
        terminal.process(b"\x1b[?1049h");
        terminal.process(b"plain");

        let replay = String::from_utf8(terminal.reconnect_repaint()).unwrap();
        // Exactly two: the widening before the painting and the restore after it,
        // both of which are the whole screen because that is what the child has.
        // A client arriving from another PTY with a narrow region gets widened
        // rather than left with someone else's margins.
        assert_eq!(
            replay.matches("\x1b[r").count(),
            2,
            "the whole screen must be asserted before and after the painting: {replay:?}"
        );
    }

    #[test]
    fn reconnect_repaint_clears_origin_mode_before_it_positions_anything() {
        // The repaint sets a scrolling region and then places the cursor
        // absolutely. Under origin mode that final row would be read relative to
        // the region's top margin, so a client that arrives with the flag already
        // on (one switching over from another PTY that had it) would put the
        // cursor a whole margin too low. Clearing the flag up front makes the
        // frame's own coordinates mean what they say. It is not a restore: the
        // child's own origin mode is still not carried across, which the
        // `mode_restore_sequence` docs spell out.
        for alt_screen in [false, true] {
            let mut terminal = TerminalState::with_scrollback(6, 20, 100);
            if alt_screen {
                terminal.process(b"\x1b[?1049h");
            }
            terminal.process(b"one\r\ntwo\r\n");
            terminal.process(b"\x1b[2;5r");
            terminal.process(b"\x1b[4;3H");

            let replay = String::from_utf8(terminal.reconnect_repaint()).unwrap();
            let origin_off_at = replay.find("\x1b[?6l").unwrap_or_else(|| {
                panic!(
                    "the replay must clear origin mode, got (alt_screen={alt_screen}):\n{replay:?}"
                )
            });
            let region_at = replay
                .find("\x1b[2;5r")
                .expect("the replay must re-assert the scroll region");
            let cursor_at = replay
                .find("\x1b[4;3H")
                .expect("the replay must place the cursor");
            assert!(
                origin_off_at < region_at && origin_off_at < cursor_at,
                "origin mode must be cleared before any region or cursor is set (alt_screen={alt_screen}): {replay:?}"
            );
            assert!(
                !replay.contains("\x1b[?6h"),
                "the replay must never turn origin mode ON (alt_screen={alt_screen}): {replay:?}"
            );
        }
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
    fn snapshot_interns_osc8_hyperlinks() {
        let mut terminal = TerminalState::with_scrollback(3, 40, 100);
        // An OSC 8 hyperlink wrapping the letter "X", then a plain "Y".
        terminal.process(b"\x1b]8;;https://example.com\x1b\\X\x1b]8;;\x1b\\Y");
        let snapshot = terminal.snapshot();

        assert_eq!(snapshot.links, vec!["https://example.com".to_string()]);
        let x = snapshot
            .cells
            .iter()
            .find(|c| c.symbol == "X")
            .expect("cell X");
        assert_eq!(x.link, Some(0));
        let y = snapshot
            .cells
            .iter()
            .find(|c| c.symbol == "Y")
            .expect("cell Y");
        assert_eq!(y.link, None);

        // empty() clears the links table.
        assert!(TerminalSnapshot::empty().links.is_empty());
    }

    #[test]
    fn snapshot_skips_links_when_collect_links_false() {
        let mut terminal = TerminalState::with_scrollback(3, 40, 100);
        terminal.process(b"\x1b]8;;https://example.com\x1b\\X\x1b]8;;\x1b\\Y");
        let mut snap = TerminalSnapshot::empty();
        terminal.snapshot_into(&mut snap, false);
        assert!(
            snap.links.is_empty(),
            "no interning when collect_links=false"
        );
        assert!(
            snap.cells.iter().all(|c| c.link.is_none()),
            "no cell carries a link when collect_links=false"
        );
    }

    #[test]
    fn snapshot_ignores_non_http_link_schemes() {
        let mut terminal = TerminalState::with_scrollback(3, 40, 100);
        // A file:// scheme is not forwardable: the cell renders as plain text and
        // no slot is consumed in the links table.
        terminal.process(b"\x1b]8;;file:///etc/passwd\x1b\\X\x1b]8;;\x1b\\Y");
        let snapshot = terminal.snapshot();
        assert!(
            snapshot.links.is_empty(),
            "file:// must not be interned as a forwardable link"
        );
        let x = snapshot
            .cells
            .iter()
            .find(|c| c.symbol == "X")
            .expect("cell X");
        assert_eq!(x.link, None);
    }

    #[test]
    fn reconnect_repaint_alt_screen_matches_viewport_repaint() {
        let mut terminal = TerminalState::with_scrollback(5, 20, 100);
        terminal.process(b"main screen line\r\n");
        terminal.process(b"\x1b[?1049h"); // enter the alternate screen
        terminal.process(b"alt content");
        assert!(terminal.is_alt_screen());

        // On the alt screen there is no scrollback to replay, so the reconnect
        // repaint is the viewport-only repaint plus the private-mode restore a
        // reconnecting client needs (`synthesize_repaint` takes a snapshot, which
        // carries cells and no modes, so it cannot emit that block itself).
        let mut expected =
            synthesize_repaint(&terminal.snapshot(), true, terminal.scroll_region.region());
        expected.extend_from_slice(mode_restore_sequence(*terminal.term.mode()).as_bytes());
        assert_eq!(terminal.reconnect_repaint(), expected);
    }

    // The two ANSI (non-private) modes the engine tracks. Insert mode is the one
    // that visibly corrupts a reconnect: a program sitting in it comes back with
    // the client OVERWRITING at the cursor where the program expects each
    // character to push the rest of the line right.
    #[test]
    fn reconnect_repaint_restores_the_ansi_modes() {
        for alt in [false, true] {
            let mut src = TerminalState::with_scrollback(6, 20, 100);
            if alt {
                src.process(b"\x1b[?1049h");
            }
            // IRM (`CSI 4 h`) and LNM (`CSI 20 h`). Neither is a private mode:
            // the private `?4` is DECSCLM smooth scrolling, which the engine does
            // not track at all.
            src.process(b"\x1b[4h\x1b[20h");
            src.process(b"content");
            assert!(src.term.mode().contains(TermMode::INSERT));
            assert!(src.term.mode().contains(TermMode::LINE_FEED_NEW_LINE));

            let mut dst = TerminalState::with_scrollback(6, 20, 100);
            dst.process(&src.reconnect_repaint());

            assert!(
                dst.term.mode().contains(TermMode::INSERT),
                "replay must re-assert insert mode (alt screen: {alt})",
            );
            assert!(
                dst.term.mode().contains(TermMode::LINE_FEED_NEW_LINE),
                "replay must re-assert line-feed/new-line mode (alt screen: {alt})",
            );
        }
    }

    #[test]
    fn reconnect_repaint_clears_stale_ansi_modes() {
        let mut src = TerminalState::with_scrollback(6, 20, 100);
        src.process(b"plain output\r\n");

        // A client left over from a different app carries both; the child has
        // neither, so a full assignment has to put them back off.
        let mut dst = TerminalState::with_scrollback(6, 20, 100);
        dst.process(b"\x1b[4h\x1b[20h");
        dst.process(&src.reconnect_repaint());

        assert!(
            !dst.term.mode().contains(TermMode::INSERT),
            "replay must clear stale insert mode",
        );
        assert!(
            !dst.term.mode().contains(TermMode::LINE_FEED_NEW_LINE),
            "replay must clear stale line-feed/new-line mode",
        );
    }

    // A hidden cursor still HAS a position, and the replay sets a scrolling
    // region, which homes the cursor. Emitting no positioning for a hidden cursor
    // therefore leaves the client's cursor at the origin instead of where the
    // program's is: harmless while the program addresses absolutely, wrong the
    // moment it moves relatively or prints.
    #[test]
    fn reconnect_repaint_places_a_hidden_cursor_where_the_program_left_it() {
        for alt in [false, true] {
            let mut src = TerminalState::with_scrollback(6, 20, 100);
            if alt {
                src.process(b"\x1b[?1049h");
            }
            src.process(b"\x1b[2;5Habc\x1b[?25l");
            let want = {
                let renderable = src.term.renderable_content();
                term::point_to_viewport(renderable.display_offset, renderable.cursor.point)
            };
            assert!(want.is_some());

            let mut dst = TerminalState::with_scrollback(6, 20, 100);
            dst.process(&src.reconnect_repaint());

            let got = {
                let renderable = dst.term.renderable_content();
                term::point_to_viewport(renderable.display_offset, renderable.cursor.point)
            };
            assert_eq!(
                got, want,
                "a hidden cursor must come back where the program left it, not \
                 homed by the scrolling-region restore (alt screen: {alt})",
            );
            assert!(
                !dst.term.mode().contains(TermMode::SHOW_CURSOR),
                "and it must still be hidden (alt screen: {alt})",
            );
        }
    }

    // A reconnecting web client resets its terminal before applying the replay,
    // so every private MODE the child enabled at its own startup is gone by the
    // time the replay lands. Modes are terminal state, not cell content, so a
    // repaint that only redraws cells leaves the client silently mismatched:
    // with mouse tracking lost, the web pane's touch-scroll forward path (which
    // is gated on `mouseTrackingMode !== "none"`) has nothing to forward to and
    // a finger drag over a full-screen agent does nothing at all.
    #[test]
    fn reconnect_repaint_restores_private_modes_on_the_alt_screen() {
        let mut src = TerminalState::with_scrollback(6, 20, 100);
        // A full-screen agent's startup: alt screen, button+drag mouse tracking
        // in SGR encoding, bracketed paste, application cursor keys, and a
        // hidden cursor.
        src.process(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?1h\x1b[?25l");
        src.process(b"alt content");
        assert!(src.is_alt_screen());
        assert!(src.has_mouse_mode());

        // The client resets, then applies the replay: a FRESH terminal is
        // exactly the state the replay has to rebuild from.
        let mut dst = TerminalState::with_scrollback(6, 20, 100);
        dst.process(&src.reconnect_repaint());

        assert!(dst.is_alt_screen(), "replay must restore the alt screen");
        assert!(
            dst.has_mouse_mode(),
            "replay must re-assert the child's mouse tracking mode",
        );
        assert!(
            dst.term.mode().contains(TermMode::MOUSE_DRAG),
            "replay must re-assert button-event (1002) tracking specifically",
        );
        assert!(
            dst.term.mode().contains(TermMode::SGR_MOUSE),
            "replay must re-assert SGR (1006) mouse encoding",
        );
        assert!(
            dst.term.mode().contains(TermMode::BRACKETED_PASTE),
            "replay must re-assert bracketed paste",
        );
        assert!(
            dst.term.mode().contains(TermMode::APP_CURSOR),
            "replay must re-assert application cursor keys",
        );
        assert!(
            !dst.term.mode().contains(TermMode::SHOW_CURSOR),
            "replay must re-assert a hidden cursor",
        );
    }

    // Same contract on the main screen, where the replay is a line stream rather
    // than a positioned repaint. A shell that turned autowrap OFF or enabled
    // bracketed paste must come back that way too.
    #[test]
    fn reconnect_repaint_restores_private_modes_on_the_main_screen() {
        let mut src = TerminalState::with_scrollback(6, 20, 100);
        src.process(b"\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?7l");
        src.process(b"prompt$ ");
        assert!(!src.is_alt_screen());

        let mut dst = TerminalState::with_scrollback(6, 20, 100);
        dst.process(&src.reconnect_repaint());

        assert!(
            dst.term.mode().contains(TermMode::BRACKETED_PASTE),
            "replay must re-assert bracketed paste on the main screen",
        );
        assert!(
            dst.term.mode().contains(TermMode::MOUSE_REPORT_CLICK),
            "replay must re-assert click (1000) tracking on the main screen",
        );
        assert!(
            dst.term.mode().contains(TermMode::SGR_MOUSE),
            "replay must re-assert SGR mouse encoding on the main screen",
        );
        assert!(
            !dst.term.mode().contains(TermMode::LINE_WRAP),
            "replay must restore autowrap-off after using autowrap to rebuild \
             soft-wrapped rows",
        );
    }

    // Modes are restored from the emulator's tracked flags, never guessed, and a
    // child that set nothing must get the default terminal back rather than a
    // block of stale mode sets.
    #[test]
    fn reconnect_repaint_restores_default_modes_when_the_child_set_none() {
        let mut src = TerminalState::with_scrollback(6, 20, 100);
        src.process(b"plain output\r\n");

        // A client left over from a DIFFERENT app (mouse tracking and bracketed
        // paste on) must be put back to the defaults this child actually has.
        let mut dst = TerminalState::with_scrollback(6, 20, 100);
        dst.process(b"\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?25l");
        dst.process(&src.reconnect_repaint());

        assert!(
            !dst.has_mouse_mode(),
            "replay must clear stale mouse tracking"
        );
        assert!(
            !dst.term.mode().contains(TermMode::SGR_MOUSE),
            "replay must clear stale SGR mouse encoding",
        );
        assert!(
            !dst.term.mode().contains(TermMode::BRACKETED_PASTE),
            "replay must clear stale bracketed paste",
        );
        assert!(
            dst.term.mode().contains(TermMode::SHOW_CURSOR),
            "replay must restore a visible cursor",
        );
        assert!(
            dst.term.mode().contains(TermMode::LINE_WRAP),
            "replay must restore autowrap-on",
        );
    }

    #[test]
    fn mode_restore_sequence_emits_both_polarities() {
        let all_off = mode_restore_sequence(TermMode::empty());
        assert!(all_off.contains("\x1b[?1000l"), "{all_off:?}");
        assert!(all_off.contains("\x1b[?1006l"), "{all_off:?}");
        assert!(all_off.contains("\x1b[?2004l"), "{all_off:?}");
        assert!(all_off.contains("\x1b[?25l"), "{all_off:?}");
        assert!(all_off.contains("\x1b[?7l"), "{all_off:?}");
        assert!(all_off.contains("\x1b>"), "{all_off:?}");

        let on = mode_restore_sequence(
            TermMode::MOUSE_MOTION
                | TermMode::SGR_MOUSE
                | TermMode::BRACKETED_PASTE
                | TermMode::SHOW_CURSOR
                | TermMode::LINE_WRAP
                | TermMode::APP_KEYPAD
                | TermMode::FOCUS_IN_OUT
                | TermMode::ALTERNATE_SCROLL,
        );
        assert!(on.contains("\x1b[?1003h"), "{on:?}");
        assert!(on.contains("\x1b[?1006h"), "{on:?}");
        assert!(on.contains("\x1b[?2004h"), "{on:?}");
        assert!(on.contains("\x1b[?25h"), "{on:?}");
        assert!(on.contains("\x1b[?7h"), "{on:?}");
        assert!(on.contains("\x1b[?1004h"), "{on:?}");
        assert!(on.contains("\x1b[?1007h"), "{on:?}");
        assert!(on.contains("\x1b="), "{on:?}");

        // Cursor addressing modes are deliberately NOT restored: the repaint
        // paints with absolute positions and never re-asserts a scroll region,
        // so re-enabling origin mode would misplace every later write.
        assert!(!on.contains("\x1b[?6"), "{on:?}");
        assert!(!all_off.contains("\x1b[?6"), "{all_off:?}");
    }

    // The receiving terminal folds 1000/1002/1003 into ONE active mouse protocol
    // and a disable of any of them clears it, so a disable emitted after the
    // enable silently undoes it. Pin the ordering: every disable in the tracking
    // family must precede every enable.
    #[test]
    fn mode_restore_sequence_enables_mouse_tracking_after_its_disables() {
        for (flag, enable) in [
            (TermMode::MOUSE_REPORT_CLICK, "\x1b[?1000h"),
            (TermMode::MOUSE_DRAG, "\x1b[?1002h"),
            (TermMode::MOUSE_MOTION, "\x1b[?1003h"),
        ] {
            let seq = mode_restore_sequence(flag);
            let enable_at = seq.find(enable).unwrap_or_else(|| {
                panic!("{enable:?} missing from {seq:?}");
            });
            for disable in ["\x1b[?1000l", "\x1b[?1002l", "\x1b[?1003l"] {
                if let Some(at) = seq.find(disable) {
                    assert!(
                        at < enable_at,
                        "{disable:?} must precede {enable:?} in {seq:?}",
                    );
                }
            }
            // Exactly one tracking mode is enabled, so nothing can re-escalate
            // or downgrade it after the fact.
            let enables = ["\x1b[?1000h", "\x1b[?1002h", "\x1b[?1003h"]
                .iter()
                .filter(|e| seq.contains(**e))
                .count();
            assert_eq!(enables, 1, "{seq:?}");
        }
    }

    #[test]
    fn scroll_region_with_bottom_margin_still_captures_scrollback() {
        // Agent CLIs pin a status bar by setting a DECSTBM scroll region with a
        // bottom margin (e.g. ESC[1;4r on a 5-row screen). A top-anchored region
        // (scroll_top == 0) must still feed scrollback so PgUp/PgDn/mouse-wheel
        // scrollback keeps working, matching xterm/alacritty/wezterm behavior.
        let mut terminal = TerminalState::with_scrollback(5, 20, 100);
        // Set a top-anchored scroll region with a bottom margin: rows 1..4,
        // leaving row 5 pinned as a status bar.
        terminal.process(b"\x1b[1;4r");
        // Home the cursor inside the region.
        terminal.process(b"\x1b[H");
        for i in 0..12 {
            terminal.process(format!("e{i}\r\n").as_bytes());
        }

        // Lines that scroll off the top of the region must be captured.
        assert!(
            terminal.term.grid().history_size() > 0,
            "scroll region with a bottom margin must still feed scrollback, got {}",
            terminal.term.grid().history_size()
        );

        let history = terminal.term.grid().history_size();
        terminal.set_scrollback(history);
        let snapshot = terminal.snapshot();
        let lines = viewport_lines(&snapshot);
        assert!(
            lines.iter().any(|line| line.contains("e0")),
            "an early line should be visible after scrolling back, got:\n{lines:#?}"
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
            dst.term.grid().history_size() > 0,
            "replay must rebuild scrollback in a fresh terminal"
        );
        assert_eq!(
            src.reconnect_repaint(),
            dst.reconnect_repaint(),
            "reconnect repaint is stable across a round-trip",
        );
    }

    #[test]
    fn reconnect_repaint_round_trips_tall_buffer_with_bottom_cursor() {
        // Reproduces the web re-attach symptom: an agent with lots of scrollback
        // and a bottom-anchored input prompt. The replay must reproduce the
        // prompt on the bottom row with the cursor inside it, not shifted up into
        // the conversation history. A non-multiple-of-rows history (29 lines into
        // a 6-row screen) also stresses the scrollback window-walk boundary.
        let mut src = TerminalState::with_scrollback(6, 20, 100);
        for i in 0..29 {
            src.process(format!("hist{i}\r\n").as_bytes());
        }
        // Draw a bottom-row prompt and leave the cursor just after it.
        src.process(b"\x1b[6;1H\x1b[2K> ");

        let src_snap = src.snapshot();
        assert_eq!(
            src_snap.cursor,
            Some(SnapshotCursor { row: 5, col: 2 }),
            "precondition: cursor sits just after the bottom-row prompt"
        );

        let replay = src.reconnect_repaint();
        let mut dst = TerminalState::with_scrollback(6, 20, 100);
        dst.process(&replay);
        let dst_snap = dst.snapshot();

        assert_eq!(
            dst_snap.cursor, src_snap.cursor,
            "cursor must round-trip to the same viewport row/col"
        );

        // Every visible row must round-trip identically (the symptom is the
        // bottom-anchored prompt shifting up into the conversation history).
        let row_text = |snap: &TerminalSnapshot, row: u16| -> String {
            let mut cells: Vec<_> = snap.cells.iter().filter(|c| c.row == row).collect();
            cells.sort_by_key(|c| c.col);
            cells
                .iter()
                .map(|c| c.symbol.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        for row in 0..src_snap.rows {
            assert_eq!(
                row_text(&dst_snap, row),
                row_text(&src_snap, row),
                "visible row {row} must round-trip identically"
            );
        }
    }

    #[test]
    fn reconnect_repaint_cursor_in_range_when_scrolled_back() {
        let mut terminal = TerminalState::with_scrollback(4, 20, 1000);
        for i in 0..12 {
            terminal.process(format!("line{i}\r\n").as_bytes());
        }
        // The operator scrolls into history; a web client connecting now must
        // still place the cursor within the live screen, not at an offset row.
        terminal.set_scrollback(terminal.term.grid().history_size());
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
        let has_wrap = |t: &TerminalState| {
            (t.term.grid().topmost_line().0..=t.term.grid().bottommost_line().0).any(|l| {
                t.term.grid()[Line(l)][Column(7)]
                    .flags
                    .contains(Flags::WRAPLINE)
            })
        };
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
        // The cursor position is the last thing painted; only the mode-restore
        // block (which never moves the cursor) follows it.
        let modes = mode_restore_sequence(*terminal.term.mode());
        assert!(
            replay.ends_with(&format!("\x1b[3;5H{modes}")),
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
    fn a_scroll_step_clamps_to_the_available_history() {
        // Both `set_scrollback` and `scroll` clamp to however much history the
        // grid actually holds, so a seeded offset is NOT evidence that a
        // further step of a given size is reachable. This is stated here
        // because a TUI test relied on the opposite: it seeded 10 rows of
        // scrollback while `seq` was still streaming (line buffered, so the
        // lines land as many small writes), and when the poll happened to land
        // with only 12 rows of history the seed succeeded at 10 while the
        // 3-line wheel step could only reach 12, not 13. Roughly 1 run in 40.
        //
        // 35 lines into a 24-row grid leaves 12 rows above the screen, which is
        // exactly the band that reproduced it, so the numbers below are the
        // measured shape of that failure rather than an invented one.
        //
        // Be honest about what this pins: it characterises the DEPENDENCY, not
        // dux logic. Deleting dux's own redundant clamp leaves this test still
        // passing (measured), because the terminal library clamps first; it is
        // kept because the scrolling helper depends on that behaviour and a
        // dependency bump that loosened it would be caught here. It also drives
        // `TerminalState` directly, so it skips the PtyClient/reader layer where
        // the original flake actually lived.
        let mut terminal = TerminalState::with_scrollback(24, 80, 12);
        for n in 1..=60 {
            terminal.process(format!("{n}\r\n").as_bytes());
        }

        terminal.set_scrollback(10);
        assert_eq!(
            terminal.scrollback_offset(),
            10,
            "12 rows of history must accept a seed of 10"
        );

        terminal.scroll(true, 3);
        assert_eq!(
            terminal.scrollback_offset(),
            12,
            "a 3-line step from 10 must CLAMP at the 12 rows of history \
             available, not reach 13"
        );

        // And the clamp is the history depth, not the step size: from the top
        // there is nowhere further to go.
        terminal.scroll(true, 3);
        assert_eq!(terminal.scrollback_offset(), 12);
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
    fn apply_identity_env_sets_and_removes() {
        use std::ffi::OsString;

        let mut cmd = CommandBuilder::new("printf");
        // Seed the two variables the identity should scrub directly on the builder,
        // so the test never mutates (or races on) the process-wide environment.
        cmd.env("KITTY_TEST_MARKER", "1");
        cmd.env("TERM_PROGRAM", "iTerm.app");
        // A fabricated ambient snapshot for the prefix-expansion path (`KITTY_*`).
        let ambient: Vec<(OsString, OsString)> = vec![
            (OsString::from("KITTY_TEST_MARKER"), OsString::from("1")),
            (OsString::from("TERM_PROGRAM"), OsString::from("iTerm.app")),
        ];
        // TERM_PROGRAM is BOTH removed (an exact scrub entry) and set: remove runs
        // first, then set, so the forced value wins.
        let identity = crate::term_identity::TerminalIdentity {
            set: vec![("TERM_PROGRAM".to_string(), "ghostty".to_string())],
            remove: vec!["TERM_PROGRAM".to_string(), "KITTY_*".to_string()],
        };
        assert!(cmd.get_env("KITTY_TEST_MARKER").is_some());
        apply_identity_env_with(&mut cmd, &identity, &ambient);
        assert_eq!(
            cmd.get_env("TERM_PROGRAM").and_then(|v| v.to_str()),
            Some("ghostty"),
            "an overlapping remove+set resolves to the set value (set runs last)"
        );
        // The prefix `KITTY_*` scrubbed the concrete inherited variable.
        assert!(cmd.get_env("KITTY_TEST_MARKER").is_none());
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
    fn terminate_reaches_a_job_controlled_foreground_app() {
        // A companion terminal runs an interactive shell, which enables job
        // control and places each foreground command in its OWN process group.
        // `terminate()` must signal that foreground group, not only the shell's,
        // or a running app never receives SIGTERM on close/shutdown (the user's
        // "waiting the 30s and the app never got asked to exit" report).
        //
        // Proof: run a foreground `sleep` (which lands in its own group),
        // `terminate()`, and watch the terminal foreground return to the shell.
        // That return happens ONLY if sleep's own group was signaled: an
        // interactive shell ignores SIGTERM, so signaling just the shell's group
        // (the pre-fix behavior) would leave sleep running with the foreground
        // pinned to it, and the assertion below would time out.
        let args = vec![
            "--norc".to_string(),
            "--noprofile".to_string(),
            "-i".to_string(),
        ];
        let client = match PtyClient::spawn("bash", &args, Path::new("."), 24, 80, 100) {
            Ok(c) => c,
            Err(_) => return, // bash unavailable on this host; skip.
        };
        let Some(child) = client.child_process_id() else {
            return;
        };

        client
            .write_bytes(b"sleep 30\n")
            .expect("write to the shell");

        // Wait until job control moves the foreground onto sleep's own group.
        let deadline = Instant::now() + std::time::Duration::from_secs(8);
        loop {
            match client.foreground_pgid() {
                Some(fg) if fg != child => break, // sleep owns the foreground group
                _ if Instant::now() >= deadline => return, // job control never engaged; skip
                _ => thread::sleep(std::time::Duration::from_millis(20)),
            }
        }

        client.terminate();

        // sleep dies from the foreground-group SIGTERM and the shell (which ignores
        // SIGTERM) reclaims the terminal foreground, so the foreground pgid returns
        // to the shell's own pid. `is_exited` is a safety valve so the test never
        // hangs if the whole thing tore down instead.
        let deadline = Instant::now() + std::time::Duration::from_secs(8);
        loop {
            if client.is_exited() || client.foreground_pgid() == Some(child) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "terminate() did not reach the foreground app's process group: the \
                 foreground sleep is still running after SIGTERM"
            );
            thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn terminate_ends_an_interactive_shell() {
        // An interactive shell deliberately ignores SIGTERM, so a graceful
        // shutdown that only sends SIGTERM can NEVER end a companion
        // terminal: the foreground app may die, but the shell survives and
        // the terminal always eats the full force-kill timeout ("0 terminals
        // exited successfully" on every shutdown). The signal that means
        // "your terminal is going away" is SIGHUP — what a real terminal
        // emulator delivers on close, and which shells answer by resending
        // HUP to their jobs and exiting. `terminate()` must send it too.
        let args = vec![
            "--norc".to_string(),
            "--noprofile".to_string(),
            "-i".to_string(),
        ];
        let client = match PtyClient::spawn("bash", &args, Path::new("."), 24, 80, 100) {
            Ok(c) => c,
            Err(_) => return, // bash unavailable on this host; skip.
        };

        // Make sure the shell is up and responsive before signaling it.
        client
            .write_bytes(b"echo shell-ready\n")
            .expect("write to the shell");
        wait_for_viewport(&client, "shell-ready");

        client.terminate();

        let deadline = Instant::now() + std::time::Duration::from_secs(8);
        while !client.is_exited() {
            assert!(
                Instant::now() < deadline,
                "terminate() did not end the interactive shell: it ignores \
                 SIGTERM, so the graceful path must also deliver SIGHUP"
            );
            thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn try_wait_memoizes_the_reap_so_a_second_caller_still_sees_the_status() {
        // The raw `Child::try_wait` reaps the zombie and yields the status
        // EXACTLY ONCE; every later call returns `None`. Several engine call
        // sites poll the same client each tick (the exit prune, the shutdown
        // sweep, the terminating-PTY reaper), so without memoization whichever
        // one polls first silently steals the exit code from the rest, and the
        // exit message that needs it reports "unknown". `reaped_at` must be
        // stamped by that first observation and stay put.
        let args = vec!["-c".to_string(), "exit 7".to_string()];
        let mut client =
            PtyClient::spawn("/bin/sh", &args, Path::new("."), 5, 40, 100).expect("spawn pty");

        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        while client.try_wait().is_none() {
            assert!(Instant::now() < deadline, "child did not exit in time");
            thread::sleep(std::time::Duration::from_millis(5));
        }

        let first = client
            .reaped_at()
            .expect("the reap instant must be stamped");
        for _ in 0..3 {
            let status = client.try_wait().expect("the status must be replayed");
            assert_eq!(status.exit_code(), 7, "the replayed status must be exact");
            assert!(!status.success());
            assert_eq!(
                client.reaped_at(),
                Some(first),
                "the reap instant must be the FIRST observation, not the latest poll"
            );
        }
    }

    #[test]
    fn reaped_at_is_none_while_the_child_is_alive() {
        // `reaped_at` is the clock the prune's drain grace runs off, so it must
        // stay unset until a `try_wait` actually observes an exit.
        let mut client = PtyClient::spawn("cat", &[], Path::new("."), 5, 40, 100).expect("spawn");
        assert_eq!(client.reaped_at(), None, "never polled: no reap yet");
        assert!(client.try_wait().is_none(), "cat is still running");
        assert_eq!(
            client.reaped_at(),
            None,
            "a poll that found no exit must not stamp a reap instant"
        );
    }

    #[test]
    fn subscribe_after_exit_does_not_leak_a_dead_subscriber() {
        // G5 regression: a subscribe landing after the reader thread's
        // one-shot `subs.clear()` (on EOF) used to attach a live subscriber to
        // a PTY that will never clear its subscriber list again — the
        // receiver would see only `Timeout`, never `Disconnected`, so a web
        // forwarder blocked on it would never complete and its socket/permit/
        // sub-quota slot would leak until the client disconnected. `subscribe`
        // now checks `is_exited()` (monotonic, set once by the reader thread)
        // before registering, so a post-exit subscribe never joins the list
        // and its receiver observes `Disconnected` immediately.
        let args = vec!["-c".to_string(), "exit 0".to_string()];
        let client =
            PtyClient::spawn("/bin/sh", &args, Path::new("."), 5, 40, 100).expect("spawn pty");

        // Wait for the reader thread to observe EOF and set `exited`.
        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        while !client.is_exited() {
            assert!(Instant::now() < deadline, "child did not exit in time");
            thread::sleep(std::time::Duration::from_millis(5));
        }
        // Give the reader thread a moment to also finish its post-loop
        // `subs.clear()` so this genuinely exercises the post-clear window,
        // not just a race with it.
        thread::sleep(std::time::Duration::from_millis(50));

        let (_guard, rx) = client.subscribe();

        // The subscriber must never have been registered: the shared list
        // stays empty (nothing for a later prune to find or leak).
        assert!(
            client
                .subscribers
                .lock()
                .expect("subscribers mutex poisoned")
                .is_empty(),
            "a post-exit subscribe must not be added to the subscriber list"
        );
        // The receiver must observe disconnection immediately (its sender was
        // never stored), not block waiting for a `subs.clear()` that will
        // never come again.
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
            other => panic!(
                "expected an immediately-disconnected receiver for a post-exit \
                 subscribe, got {other:?}"
            ),
        }
    }

    // ---------------------------------------------------------------------
    // Replay handoff: a freshly connected client must see every byte of the
    // child's output EXACTLY ONCE, in order, with nothing lost.
    //
    // The failure these pin is not cosmetic. `subscribe_with_repaint` hands a
    // client a snapshot of the grid plus a live byte stream, and if a chunk can
    // land in BOTH the client renders the snapshot's tail and then a replay of
    // bytes already inside it: for line-oriented output that appends, so the log
    // reads as a jump forward followed by a jump back. If a chunk can land in
    // NEITHER, the agent's output is silently lost, which is worse.
    // ---------------------------------------------------------------------

    /// Width of the numeric part of the `L<n>` marker lines these tests stream.
    ///
    /// It is FIXED, and that is load bearing. A PTY read can cut a line anywhere,
    /// so the grid routinely ends mid-number and the next chunk carries the rest;
    /// the checks below therefore join the repaint and the live stream before
    /// parsing, and the join is what puts such a number back together. With
    /// variable-width ids a join of two halves that do NOT belong together also
    /// yields a well-formed number, so a stream that had genuinely lost bytes was
    /// reported with a FABRICATED id (measured: a repaint ending `L5` and a chunk
    /// opening `4465` were reported as one line `L54465`, which describes no line
    /// either side ever produced). With a fixed width the arithmetic gives it
    /// away: the repaint keeps the first `k` digits of the line it was cut in and
    /// the chunk carries the last `width - m` digits of the line IT was cut in, so
    /// the join is `width` digits long only when `k == m`, which is exactly the
    /// case where the two halves are the same line. Every other join comes out the
    /// wrong length and [`scan_line_ids`] names it as itself instead of printing a
    /// number nobody can act on. (A loss that happens to cut both lines at the
    /// same digit still yields a well-formed id, but it is then a real id from the
    /// halves that were joined, and it is off by more than one, so the ordinary
    /// contiguity check still reports it.)
    const LINE_ID_DIGITS: usize = 8;

    /// A shell that emits one `L<n>` line per line we write to it, so a test
    /// decides exactly when output is produced and never sleeps on a guess.
    /// `stty -echo` keeps the line discipline from echoing our own writes back,
    /// so the child's output is only the lines it prints.
    const LINE_ECHOER: &str =
        "stty -echo; while IFS= read -r n; do printf 'L%08d\\r\\n' \"$n\"; done";

    fn spawn_line_echoer(scrollback: usize) -> PtyClient {
        let args = vec!["-c".to_string(), LINE_ECHOER.to_string()];
        PtyClient::spawn("/bin/sh", &args, Path::new("."), 6, 40, scrollback).expect("spawn pty")
    }

    /// [`LINE_ECHOER`] with each line padded out to about 4 KiB by a long run of
    /// no-op SGR resets. The padding is what lets a test push megabytes of child
    /// output through the reader without paying for megabytes of grid: `ESC[0m`
    /// advances the parser and occupies the byte stream but writes no cell, so
    /// 1500 echoed lines are 6 MB on the wire and still only 1500 rows of
    /// history. Measured with `/bin/sh`: 4107 bytes per echoed line.
    const PADDED_LINE_ECHOER: &str = "stty -echo; pad=$(printf '\\033[0m'); i=0; \
         while [ $i -lt 10 ]; do pad=\"$pad$pad\"; i=$((i+1)); done; \
         while IFS= read -r n; do printf 'L%08d%s\\r\\n' \"$n\" \"$pad\"; done";

    fn spawn_padded_line_echoer(scrollback: usize) -> PtyClient {
        let args = vec!["-c".to_string(), PADDED_LINE_ECHOER.to_string()];
        PtyClient::spawn("/bin/sh", &args, Path::new("."), 6, 40, scrollback).expect("spawn pty")
    }

    /// Every row the grid holds, scrollback history first and then the viewport,
    /// right-trimmed. The snapshot only ever exposes the visible rows, so a test
    /// that needs to prove nothing fell out of history has to read the grid.
    fn all_grid_lines(client: &PtyClient) -> Vec<String> {
        let terminal = client.terminal.lock().expect("terminal mutex poisoned");
        let history = terminal.term.grid().history_size() as i32;
        let rows = i32::from(terminal.rows);
        let cols = usize::from(terminal.cols);
        let mut out = Vec::with_capacity((history + rows) as usize);
        for line in -history..rows {
            let row = &terminal.term.grid()[Line(line)];
            let mut text = String::with_capacity(cols);
            for c in 0..cols {
                text.push(row[Column(c)].c);
            }
            out.push(text.trim_end().to_string());
        }
        out
    }

    /// The `L<n>` ids carried by `lines`, in order.
    fn grid_line_ids(lines: &[String]) -> Vec<u64> {
        lines
            .iter()
            .filter_map(|line| {
                let rest = line.strip_prefix('L')?;
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                (digits.len() == LINE_ID_DIGITS).then(|| digits.parse().ok())?
            })
            .collect()
    }

    /// Drop ANSI escape sequences so the remaining bytes are the text a client
    /// would end up displaying.
    fn strip_ansi(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] == 0x1b && i + 1 < bytes.len() {
                match bytes[i + 1] {
                    // CSI: parameters then a final byte in 0x40..=0x7e.
                    b'[' => {
                        let mut j = i + 2;
                        while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                            j += 1;
                        }
                        i = j.saturating_add(1);
                    }
                    // OSC: terminated by BEL or ST.
                    b']' => {
                        let mut j = i + 2;
                        while j < bytes.len() && bytes[j] != 0x07 {
                            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                                j += 1;
                                break;
                            }
                            j += 1;
                        }
                        i = j.saturating_add(1);
                    }
                    _ => i += 2,
                }
                continue;
            }
            out.push(bytes[i]);
            i += 1;
        }
        out
    }

    /// What a scan of a byte stream for `L<n>` markers found.
    struct LineIdScan {
        /// Every complete id, in the order it appears.
        ids: Vec<u64>,
        /// Digit runs whose length was not [`LINE_ID_DIGITS`], as
        /// `(byte offset, the run)`. A run that is too SHORT at the very END of
        /// the stream is simply a chunk cut in half and is not recorded. Every
        /// other wrong-length run is, including one that is too LONG at the end
        /// (a run cut short cannot have grown past the width) and a bare marker
        /// letter with no digits behind it at a junction: those mean the two
        /// halves that were joined did not belong together, which is bytes lost
        /// or bytes replayed.
        malformed: Vec<(usize, String)>,
    }

    /// Scan `bytes` for `L<n>` markers. Callers pass the repaint and the live
    /// stream CONCATENATED, so a line split across the junction (the grid ends
    /// mid-number and the stream carries the rest) reads back as the one id it
    /// really is.
    fn scan_line_ids(bytes: &[u8]) -> LineIdScan {
        let text = String::from_utf8_lossy(&strip_ansi(bytes)).to_string();
        let raw = text.as_bytes();
        let mut scan = LineIdScan {
            ids: Vec::new(),
            malformed: Vec::new(),
        };
        let mut i = 0usize;
        while i < raw.len() {
            if raw[i] == b'L' {
                let mut j = i + 1;
                while j < raw.len() && raw[j].is_ascii_digit() {
                    j += 1;
                }
                let run = &text[i + 1..j];
                // A run is only excusable as a chunk boundary when it could
                // still have been GROWING when the stream stopped: too few
                // digits, and nothing after it. A run that is already too LONG
                // cannot have been cut short, and a marker with no digits at all
                // is not a partial number as long as some other byte follows it,
                // so both are faults wherever they sit.
                let at_end_of_stream = j >= raw.len();
                let excusable_boundary = at_end_of_stream && run.len() < LINE_ID_DIGITS;
                if run.len() == LINE_ID_DIGITS {
                    scan.ids.push(run.parse::<u64>().expect("digits"));
                } else if !excusable_boundary {
                    scan.malformed.push((i, run.to_string()));
                }
                if j > i + 1 {
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        scan
    }

    /// Every complete `L<n>` id in `bytes`, in the order it appears.
    fn line_ids(bytes: &[u8]) -> Vec<u64> {
        scan_line_ids(bytes).ids
    }

    /// Poll `cond` until it holds, returning whether it ever did.
    fn wait_until(timeout: std::time::Duration, mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if cond() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Drain everything currently queued on a subscriber's receiver.
    fn drain(rx: &std::sync::mpsc::Receiver<Vec<u8>>, settle: std::time::Duration) -> Vec<u8> {
        let mut out = Vec::new();
        while let Ok(chunk) = rx.recv_timeout(settle) {
            out.extend_from_slice(&chunk);
        }
        out
    }

    #[test]
    fn scan_line_ids_names_a_join_of_two_different_lines_instead_of_inventing_one() {
        // The exact shape the busy-PTY check parses: a repaint whose grid ends
        // mid-number, immediately followed by the live chunk carrying the rest.
        let repaint_tail = b"L00000454\r\nL00000455\r\nL0000";

        // The halves belong together, so the join is the line they came from.
        let ok = scan_line_ids(&[repaint_tail.as_slice(), b"0456\r\n"].concat());
        assert_eq!(ok.ids, vec![454, 455, 456]);
        assert!(ok.malformed.is_empty());

        // They do not, because bytes went missing between them. Under a
        // variable-width format this join reads as a perfectly well-formed line
        // that neither side ever printed; the fixed width makes it the wrong
        // length, so it is reported as what it is.
        let lost = scan_line_ids(&[repaint_tail.as_slice(), b"04465\r\n"].concat());
        assert_eq!(lost.ids, vec![454, 455]);
        assert_eq!(
            lost.malformed,
            vec![(22usize, "000004465".to_string())],
            "a wrong-width marker must be reported as itself, not turned into an id"
        );

        // A run cut short by the END of the stream is just a chunk boundary and
        // is not a fault.
        let cut = scan_line_ids(b"L00000454\r\nL0000");
        assert_eq!(cut.ids, vec![454]);
        assert!(cut.malformed.is_empty());

        // A run LONGER than the fixed width is never a chunk cut short, so it is
        // a fault wherever it sits, end of stream included.
        let too_long = scan_line_ids(b"L00000454\r\nL0000045566");
        assert_eq!(too_long.ids, vec![454]);
        assert_eq!(
            too_long.malformed,
            vec![(11usize, "0000045566".to_string())],
            "an over-length run cannot be a chunk cut short",
        );

        // A bare marker letter with no digits after it is likewise not a partial
        // number, so long as there is a following byte to prove the stream did
        // not simply stop on the letter.
        let bare = scan_line_ids(b"L00000454\r\nL\r\n");
        assert_eq!(bare.ids, vec![454]);
        assert_eq!(
            bare.malformed,
            vec![(11usize, String::new())],
            "a marker with no digits at a junction is a fault, not a boundary",
        );

        // A stream that ends ON the marker letter really could be a chunk cut in
        // half, so it stays unrecorded.
        let trailing = scan_line_ids(b"L00000454\r\nL");
        assert_eq!(trailing.ids, vec![454]);
        assert!(trailing.malformed.is_empty());
    }

    #[test]
    fn a_client_does_not_join_the_fan_out_list_before_it_can_reach_its_snapshot() {
        // The SUBSCRIBE half of the exactly-once handoff, guarded by the one
        // thing about it that is directly observable: a client must not appear in
        // the fan-out list while it is still blocked from taking its snapshot.
        //
        // Be honest about the gap between the name and the contract. The contract
        // (see `subscribe_with_repaint`) is that registration and the snapshot
        // happen under ONE hold of the terminal lock. This test does not pin
        // that, and cannot cheaply: an implementation that took the lock,
        // registered, released it, then re-took it to snapshot would satisfy
        // every assertion below and still have the original bug. What it does
        // pin, deterministically, is that registration is behind the terminal
        // lock at all, which is the half that was actually missing. It
        // constructs no duplicated byte and observes none.
        //
        // Registering first and snapshotting second, with the lock taken only for
        // the snapshot, opens a window in which a chunk is fanned out to a client
        // that is still parked waiting for its grid. That chunk then lands in the
        // channel AND in the snapshot the client is about to be handed, so the
        // client renders the snapshot's tail and then a replay of bytes already
        // inside it. `Mutex` is not fair, so the window is not "a few bytes": the
        // reader barges and the overlap is bounded only by how long the client is
        // starved (measured at thousands of lines on a busy PTY).
        //
        // The window is CONSTRUCTED rather than raced for. A helper thread holds
        // the terminal lock, so a `subscribe_with_repaint` on this thread cannot
        // reach its snapshot, and the helper then watches the subscriber list. If
        // the call registered anyway, the window exists. It must not.
        //
        // `PtyClient` is not `Sync`, so the orchestration runs on the helper with
        // only the shared handles while THIS thread makes the real call.
        let client = spawn_line_echoer(500);
        client.write_bytes(b"1\r").expect("write");
        wait_for_viewport(&client, "L00000001");

        let terminal = Arc::clone(&client.terminal);
        let subscribers = Arc::clone(&client.subscribers);
        let (holding_tx, holding_rx) = std::sync::mpsc::channel::<()>();

        let helper = thread::spawn(move || {
            let held = terminal.lock().expect("terminal mutex poisoned");
            // Only now may the subscriber try to snapshot, so it is guaranteed to
            // park on this lock rather than sail through.
            holding_tx.send(()).expect("main thread alive");
            // A one-sided wait: it can only give a client that registers early
            // more time to have done so, and a client that cannot register until
            // it holds this lock cannot register at all while the helper holds it,
            // however long the wait runs.
            let registered = wait_until(std::time::Duration::from_millis(300), || {
                !subscribers
                    .lock()
                    .expect("subscribers mutex poisoned")
                    .is_empty()
            });
            drop(held);
            registered
        });

        holding_rx.recv().expect("helper thread alive");
        let (_guard, repaint, rx) = client.subscribe_with_repaint();
        let registered_while_parked = helper.join().expect("helper thread");

        assert!(
            !registered_while_parked,
            "a client appeared in the fan-out list while it was still parked \
             waiting for the grid it is about to be handed; every chunk read in \
             that window would reach the client twice"
        );

        // And the ordinary invariant the client cares about, end to end.
        for n in 2..=6 {
            client
                .write_bytes(format!("{n}\r").as_bytes())
                .expect("write");
        }
        wait_for_viewport(&client, "L00000006");

        let mut combined = strip_ansi(&repaint);
        combined.extend_from_slice(&drain(&rx, std::time::Duration::from_millis(200)));
        assert_eq!(
            line_ids(&combined),
            vec![1, 2, 3, 4, 5, 6],
            "a freshly connected client must see each line exactly once and in \
             order (its whole byte stream reads {:?})",
            String::from_utf8_lossy(&combined),
        );
    }

    /// A line echoer that also rings the terminal bell on every line it prints.
    ///
    /// The bell is an ANCHOR, not decoration. The reader loop scans every chunk
    /// for attention signals BEFORE it takes the terminal lock and before it fans
    /// anything out, in either arrangement of those two steps, so a raised bell
    /// proves the reader is already holding that chunk. Without it a test that
    /// asserts "the reader has not fanned this out yet" cannot tell a correctly
    /// parked reader from one that simply has not read the bytes yet.
    const BELLING_LINE_ECHOER: &str =
        "stty -echo; while IFS= read -r n; do printf '\\aL%08d\\r\\n' \"$n\"; done";

    #[test]
    fn a_chunk_the_reader_already_holds_still_reaches_a_client_that_registers_first() {
        // The READER half of the exactly-once handoff, on its own and without
        // racing for it. The fan-out has to happen UNDER the terminal lock; with
        // it outside, this ordering loses a chunk outright:
        //
        //   reader:     fans chunk C out; nobody is subscribed, so C goes nowhere
        //   subscriber: takes the terminal lock, registers, snapshots a grid
        //               that does not contain C yet
        //   reader:     finally gets the lock and parses C
        //
        // C is in neither the repaint nor the channel and the client never sees
        // it, which is the missing-lines half of the reported corruption. Taking
        // the lock first makes the ordering impossible: a reader parked on the
        // lock has fanned nothing out, so a subscriber that registers while it is
        // parked is guaranteed to be sent the chunk when the lock is released.
        //
        // The window is CONSTRUCTED. This thread holds the terminal lock and then
        // performs, by hand, exactly the two steps `subscribe_with_repaint` takes
        // once it owns that lock (register, then snapshot). It cannot call the
        // real method, which would deadlock on the lock it is already holding.
        //
        // One acknowledged hole in the revert detection: it assumes the child's
        // single small write arrives at the reader as ONE read, so that a
        // fan-out placed before the lock would have carried the whole line
        // before this thread subscribes. A short read splitting that line would
        // let a reverted implementation still deliver the tail and pass. In
        // practice a write this small is never split, so this is recorded rather
        // than defended against.
        let args = vec!["-c".to_string(), BELLING_LINE_ECHOER.to_string()];
        let client =
            PtyClient::spawn("/bin/sh", &args, Path::new("."), 6, 40, 500).expect("spawn pty");
        client.write_bytes(b"1\r").expect("write");
        wait_for_viewport(&client, "L00000001");

        client.attention_bell.store(false, Ordering::Release);
        let held = client.terminal.lock().expect("terminal mutex poisoned");
        client.write_bytes(b"2\r").expect("write");
        assert!(
            wait_until(std::time::Duration::from_secs(5), || client
                .attention_bell
                .load(Ordering::Acquire)),
            "the reader never read the chunk carrying the second line"
        );
        // Give a fan-out placed BEFORE the lock every chance to have happened.
        // The wait is one-sided: it can only make an early fan-out more likely to
        // have completed, and a reader parked on the terminal lock cannot make
        // progress no matter how long it lasts, so it cannot turn a correct
        // reader into a failure.
        thread::sleep(std::time::Duration::from_millis(250));

        let (_guard, rx) = client.subscribe();
        let repaint = held.reconnect_repaint();
        drop(held);

        let mut combined = strip_ansi(&repaint);
        combined.extend_from_slice(&drain(&rx, std::time::Duration::from_millis(300)));
        assert_eq!(
            line_ids(&combined),
            vec![1, 2],
            "a chunk the reader was already holding must still reach a client \
             that registered before the chunk was parsed (its whole byte stream \
             reads {:?})",
            String::from_utf8_lossy(&combined),
        );
    }

    #[test]
    fn a_client_connecting_while_the_operator_reads_history_still_gets_every_line() {
        // A browser attaching is rebuilt from the GRID. While the reader held
        // bytes back, the grid a scrolled-back operator was looking at was stale
        // by however much the child had produced since, so a reconnect landing in
        // that window rebuilt from old content and the missing bytes had to be
        // stapled onto the repaint as a raw tail. Parsing unconditionally means
        // the grid is always current, so the repaint alone is exact and the
        // stapling is gone. Prove the property that mattered: connect mid
        // scrollback and lose nothing.
        let client = spawn_line_echoer(500);

        for n in 1..=8 {
            client
                .write_bytes(format!("{n}\r").as_bytes())
                .expect("write");
        }
        wait_for_viewport(&client, "L00000008");

        client.set_scrollback(3);
        assert_eq!(
            client.scrollback_offset(),
            3,
            "the operator is reading back"
        );

        for n in 9..=12 {
            client
                .write_bytes(format!("{n}\r").as_bytes())
                .expect("write");
        }
        assert!(
            wait_until(std::time::Duration::from_secs(5), || {
                grid_line_ids(&all_grid_lines(&client)).last() == Some(&12)
            }),
            "output produced while scrolled back must still reach the grid"
        );

        let (_guard, repaint, rx) = client.subscribe_with_repaint();
        client.write_bytes(b"13\r").expect("write");
        assert!(
            wait_until(std::time::Duration::from_secs(5), || {
                grid_line_ids(&all_grid_lines(&client)).last() == Some(&13)
            }),
            "the child never echoed the last line"
        );

        let mut combined = strip_ansi(&repaint);
        combined.extend_from_slice(&drain(&rx, std::time::Duration::from_millis(200)));
        assert_eq!(
            line_ids(&combined),
            (1..=13).collect::<Vec<u64>>(),
            "a client connecting while the operator reads history must be handed \
             every line exactly once (its whole byte stream reads {:?})",
            String::from_utf8_lossy(&combined),
        );
    }

    #[test]
    fn scrolling_back_never_drops_the_output_that_arrives_while_you_read() {
        // The whole point of parsing unconditionally. Reading history is a VIEW
        // operation: it must not change what the terminal records. dux used to
        // stop feeding the parser while the operator was scrolled back and hold
        // the child's bytes in a side buffer capped at 4 MiB, dropping the OLDEST
        // bytes on overflow, so a scrollback session across a busy build simply
        // lost the middle of it, permanently and silently. No real terminal does
        // this: tmux's pane reader parses regardless of copy mode, and alacritty
        // compensates the view rather than the stream.
        //
        // So: scroll back, produce well past that old cap, come back to the
        // bottom, and require every line to be in history, in order, with no gap.
        const LINES: u64 = 1500; // about 6.2 MB of child output, comfortably past 4 MiB.
        let client = spawn_padded_line_echoer(5000);

        // A subscriber sees the raw byte stream regardless of what the grid is
        // doing, which is how this test knows the child has finished echoing
        // without asking the grid a question the grid used to refuse to answer.
        let (_guard, rx) = client.subscribe();

        client.write_bytes(b"1\r").expect("write the first line");
        wait_for_viewport(&client, "L00000001");

        // Build a little history, then scroll into it.
        for n in 2..=10 {
            client
                .write_bytes(format!("{n}\r").as_bytes())
                .expect("write");
        }
        wait_for_viewport(&client, "L00000010");
        client.set_scrollback(4);
        assert_eq!(
            client.scrollback_offset(),
            4,
            "the test has to actually be scrolled back to mean anything"
        );

        let mut input = String::new();
        for n in 11..=LINES {
            input.push_str(&format!("{n}\r"));
        }
        client
            .write_bytes(input.as_bytes())
            .expect("write the bulk lines");

        // Wait for the last line on the wire, carrying a few bytes between chunks
        // so a marker split across a chunk boundary is still found.
        let marker = format!("L{LINES:08}");
        let deadline = Instant::now() + std::time::Duration::from_secs(120);
        let mut carry = String::new();
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(chunk) => {
                    carry.push_str(&String::from_utf8_lossy(&chunk));
                    if carry.contains(&marker) {
                        break;
                    }
                    let keep = carry.len().saturating_sub(marker.len());
                    carry.drain(..keep);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("the echoer died before it echoed {marker}")
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            assert!(
                Instant::now() < deadline,
                "the echoer never reached {marker} on the wire"
            );
        }

        // Back to the live bottom, and give the grid a moment to hold every line.
        client.set_scrollback(0);
        assert!(
            wait_until(std::time::Duration::from_secs(30), || {
                grid_line_ids(&all_grid_lines(&client)).last() == Some(&LINES)
            }),
            "the last echoed line never reached the grid"
        );

        let ids = grid_line_ids(&all_grid_lines(&client));
        let expected: Vec<u64> = (1..=LINES).collect();
        if ids != expected {
            let missing: Vec<u64> = expected
                .iter()
                .copied()
                .filter(|n| !ids.contains(n))
                .collect();
            panic!(
                "scrolling back lost child output: the grid holds {} of {LINES} lines, \
                 first {:?}, last {:?}, {} missing (first few: {:?}). Reading history \
                 must never change what the terminal records.",
                ids.len(),
                ids.first(),
                ids.last(),
                missing.len(),
                missing.iter().take(5).collect::<Vec<_>>(),
            );
        }
    }

    #[test]
    fn many_clients_attaching_to_a_busy_pty_each_get_a_seamless_stream() {
        // The user-visible reproduction: a real, continuously streaming,
        // line-oriented PTY with clients attaching while it runs. For every
        // attach, the repaint and the first live chunk must join seamlessly, with
        // no id repeated and none skipped. Before the fix this failed within the
        // first few dozen attaches (measured: 5 of 78 attaches overlapped, one by
        // more than three thousand lines).
        //
        // This is the SYSTEM-level check and it is inherently sampled, so the two
        // halves of the fix each also carry a constructed, deterministic test of
        // their own (`a_client_does_not_join_the_fan_out_list_before_it_can_reach_its_snapshot`
        // for the subscribe side, which pins that registration happens behind the
        // terminal lock rather than the no-duplicate outcome itself, and
        // `a_chunk_the_reader_already_holds_still_reaches_a_client_that_registers_first`
        // for the reader side). Do not rely on this one to catch a revert.
        //
        // Enough lines that the child cannot possibly finish before the checks
        // below do, even on a loaded machine; the loop leaves as soon as it has
        // seen enough attaches and dropping the client kills the child.
        let total = 400_000u64;
        let script = format!(
            "i=1; while [ $i -le {total} ]; do printf 'L%08d\\n' \"$i\"; i=$((i+1)); done; echo END"
        );
        let args = vec!["-c".to_string(), script];
        let client =
            PtyClient::spawn("/bin/sh", &args, Path::new("."), 6, 40, 500_000).expect("spawn pty");
        assert!(
            wait_until(std::time::Duration::from_secs(10), || client.has_output()),
            "the child never produced any output"
        );

        let wanted = 25usize;
        let mut attaches = 0usize;
        let mut checked = 0usize;
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        while checked < wanted && Instant::now() < deadline {
            attaches += 1;
            let (guard, repaint, rx) = client.subscribe_with_repaint();
            if line_ids(&repaint).is_empty() {
                drop(guard);
                continue;
            }
            // The next live chunk, if the child is still running.
            let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_millis(500)) else {
                drop(guard);
                break;
            };
            drop(guard);

            let mut joined = strip_ansi(&repaint);
            joined.extend_from_slice(&chunk);
            let scan = scan_line_ids(&joined);
            let context = || {
                format!(
                    "repaint tail {:?}, first live chunk {:?}",
                    String::from_utf8_lossy(&strip_ansi(
                        &repaint[repaint.len().saturating_sub(60)..]
                    )),
                    String::from_utf8_lossy(&chunk[..chunk.len().min(60)]),
                )
            };
            // A marker of the wrong width can only come from joining two halves
            // that did not belong together, so it is a failure in its own right
            // and is reported as itself rather than as some invented line number.
            assert!(
                scan.malformed.is_empty(),
                "attach {attaches} handed a client a marker of the wrong width at \
                 {:?}: every id is {LINE_ID_DIGITS} digits, so this is two halves \
                 of different lines joined together ({})",
                scan.malformed,
                context(),
            );
            if scan.ids.len() < 3 {
                continue;
            }
            for (i, pair) in scan.ids.windows(2).enumerate() {
                assert_eq!(
                    pair[1],
                    pair[0] + 1,
                    "attach {attaches} handed a client a discontinuity at index {i}: \
                     L{} is followed by L{} ({})",
                    pair[0],
                    pair[1],
                    context(),
                );
            }
            checked += 1;
        }
        assert_eq!(
            checked, wanted,
            "only {checked} of {wanted} attaches could be checked (attempted \
             {attaches}), which is too few to mean anything"
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
        let history = terminal.term.grid().history_size();
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
        terminal.snapshot_into(&mut buf, true);
        let cap_after_first = buf.cells.capacity();
        assert!(!buf.cells.is_empty(), "first snapshot should have cells");

        // Second call should reuse the Vec capacity.
        terminal.process(b"more\r\n");
        terminal.snapshot_into(&mut buf, true);
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
        terminal.snapshot_into(&mut buf, true);

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
        terminal.snapshot_into(&mut buf, true);
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
            link: None,
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
            links: Vec::new(),
        };
        let bytes = synthesize_repaint(&snapshot, true, ScrollRegion::full(1));
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
            links: Vec::new(),
        };
        let bytes = synthesize_repaint(&snapshot, false, ScrollRegion::full(1));
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

    /// Dropping the guard removes the subscriber immediately, before any PTY
    /// output arrives. The receiver must observe disconnection via `Err`.
    #[test]
    fn dropping_the_guard_removes_subscriber_without_output() {
        let args = vec!["-c".to_string(), "cat".to_string()];
        let client =
            PtyClient::spawn("/bin/sh", &args, Path::new("."), 5, 40, 100).expect("spawn pty");
        let (guard, rx) = client.subscribe();
        drop(guard);
        // No PTY output is produced. The sender was removed by the guard's Drop,
        // so the channel is now disconnected and recv() must return Err.
        assert!(
            rx.recv().is_err(),
            "receiver should observe disconnection after guard is dropped"
        );
    }

    /// Dropping one guard removes only its subscriber; the other remains live.
    #[test]
    fn dropping_one_guard_keeps_the_other_subscriber() {
        let args = vec!["-c".to_string(), "cat".to_string()];
        let client =
            PtyClient::spawn("/bin/sh", &args, Path::new("."), 5, 40, 100).expect("spawn pty");
        let (g1, rx1) = client.subscribe();
        let (_g2, rx2) = client.subscribe();
        drop(g1);
        // The first subscriber's channel is disconnected.
        assert!(
            rx1.recv().is_err(),
            "rx1 should observe disconnection after g1 is dropped"
        );
        // The second subscriber is still alive: no output, so Empty.
        assert!(
            matches!(rx2.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
            "rx2 should still be live (Empty), not disconnected"
        );
    }
}
