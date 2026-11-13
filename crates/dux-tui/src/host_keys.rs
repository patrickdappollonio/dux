//! Asking the host terminal for extended keys, and handing it back clean.
//!
//! Outside tmux dux asks the host whether it speaks the Kitty keyboard
//! protocol, with its own bounded query, and pushes the protocol's first level
//! (disambiguate) on the alternate screen. Inside tmux it never asks for Kitty:
//! it reads tmux's own settings with the tmux command line and asks tmux for its
//! extended keys (modifyOtherKeys level 2) only when tmux will send them in the
//! `csi-u` format dux reads.
//!
//! What dux switched on is kept in a [`HostKeysRecord`], two bits in one
//! atomic, so the panic hook can read it without waiting on a lock the
//! panicking thread may hold. Every switch-on sets its bit only after its bytes
//! were written and flushed; every teardown swaps the record to empty and
//! writes the switch-off for each bit it took, so a second teardown writes
//! nothing.

use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStringExt;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use dux_core::config::ExtendedKeysMode;

/// Pushes the Kitty protocol's disambiguate level (crossterm's
/// `PushKeyboardEnhancementFlags` with `DISAMBIGUATE_ESCAPE_CODES`).
pub(crate) const KITTY_PUSH: &[u8] = b"\x1b[>1u";
/// Sets the disambiguate level on the current stack entry, without pushing a
/// new one, so a force redraw can re-assert it without growing the stack.
pub(crate) const KITTY_REASSERT: &[u8] = b"\x1b[=1;1u";
/// Pops the entry [`KITTY_PUSH`] pushed (crossterm's
/// `PopKeyboardEnhancementFlags`).
pub(crate) const KITTY_POP: &[u8] = b"\x1b[<1u";
/// Asks tmux for its extended keys, modifyOtherKeys level 2.
pub(crate) const TMUX_REQUEST: &[u8] = b"\x1b[>4;2m";
/// xterm's documented modifyOtherKeys reset, which tmux treats as "back to
/// what the server option says".
pub(crate) const TMUX_RESET: &[u8] = b"\x1b[>4;0m";

/// How long dux waits for the host terminal to answer its Kitty query.
pub(crate) const KITTY_QUERY_DEADLINE: Duration = Duration::from_secs(1);
/// How long one `tmux show-options` may run before it is killed.
pub(crate) const TMUX_COMMAND_CEILING: Duration = Duration::from_secs(2);

const KITTY_BIT: u8 = 1;
const TMUX_BIT: u8 = 2;

/// What dux has switched on in the host terminal or tmux.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HostKeys {
    /// The Kitty disambiguate level was pushed on the host's alternate screen.
    pub(crate) kitty_pushed: bool,
    /// tmux was asked for its extended keys.
    pub(crate) tmux_extended: bool,
}

impl HostKeys {
    fn from_bits(bits: u8) -> Self {
        Self {
            kitty_pushed: bits & KITTY_BIT != 0,
            tmux_extended: bits & TMUX_BIT != 0,
        }
    }
}

/// A lock-free record of [`HostKeys`].
#[derive(Debug, Default)]
pub(crate) struct HostKeysRecord(AtomicU8);

impl HostKeysRecord {
    /// An empty record: nothing switched on.
    pub(crate) const fn new() -> Self {
        Self(AtomicU8::new(0))
    }

    /// What is switched on now.
    pub(crate) fn current(&self) -> HostKeys {
        HostKeys::from_bits(self.0.load(Ordering::SeqCst))
    }

    fn set(&self, bit: u8) {
        self.0.fetch_or(bit, Ordering::SeqCst);
    }

    /// Clear `bit`, returning whether it was set.
    fn clear(&self, bit: u8) -> bool {
        self.0.fetch_and(!bit, Ordering::SeqCst) & bit != 0
    }

    fn take(&self) -> HostKeys {
        HostKeys::from_bits(self.0.swap(0, Ordering::SeqCst))
    }
}

/// The process's record of what dux switched on in the host terminal.
pub(crate) static HOST_KEYS: HostKeysRecord = HostKeysRecord::new();

fn write_all_and_flush(out: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    out.write_all(bytes)?;
    out.flush()
}

/// Push the Kitty disambiguate level, recording it only when the bytes were
/// written and flushed. A failure is returned for the caller to log; the
/// record stays clear, so dux carries on with classic keys.
pub(crate) fn switch_on_kitty(out: &mut impl Write, record: &HostKeysRecord) -> io::Result<()> {
    write_all_and_flush(out, KITTY_PUSH)?;
    record.set(KITTY_BIT);
    Ok(())
}

/// Re-assert the Kitty level on the current stack entry when dux pushed it.
/// Write errors are ignored, like the rest of a force redraw's re-sends.
pub(crate) fn reassert_kitty(out: &mut impl Write, record: &HostKeysRecord) {
    if record.current().kitty_pushed {
        let _ = write_all_and_flush(out, KITTY_REASSERT);
    }
}

/// Ask tmux for its extended keys, recording it only when the bytes were
/// written and flushed.
pub(crate) fn switch_on_tmux(out: &mut impl Write, record: &HostKeysRecord) -> io::Result<()> {
    write_all_and_flush(out, TMUX_REQUEST)?;
    record.set(TMUX_BIT);
    Ok(())
}

/// Apply the tmux half of a plan: ask tmux for its extended keys whenever the
/// plan says so, even when the record already holds the bit, because a pane
/// reset since (a `reset`, a program that switched modifyOtherKeys off) is
/// exactly what a force redraw exists to repair; otherwise hand them back.
pub(crate) fn apply_tmux_plan(
    out: &mut impl Write,
    record: &HostKeysRecord,
    plan: &Plan,
) -> io::Result<()> {
    if plan.request_tmux {
        switch_on_tmux(out, record)
    } else {
        switch_off_tmux(out, record);
        Ok(())
    }
}

/// Hand tmux's extended keys back, when dux asked for them. Write errors are
/// ignored: the bit is cleared first, so nothing is reset twice.
pub(crate) fn switch_off_tmux(out: &mut impl Write, record: &HostKeysRecord) {
    if record.clear(TMUX_BIT) {
        let _ = write_all_and_flush(out, TMUX_RESET);
    }
}

/// Write the switch-off for everything the record holds, emptying it first, so
/// a second call writes nothing. Write errors are ignored: this runs while the
/// terminal is being handed back, a panic included, and there is nobody left
/// to tell.
pub(crate) fn write_switch_off(out: &mut impl Write, record: &HostKeysRecord) {
    let taken = record.take();
    if taken.kitty_pushed {
        let _ = out.write_all(KITTY_POP);
    }
    if taken.tmux_extended {
        let _ = out.write_all(TMUX_RESET);
    }
    let _ = out.flush();
}

/// The host terminal's answer to dux's Kitty query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KittyAnswer {
    /// It answered the Kitty query: it speaks the protocol.
    Yes,
    /// It answered only the device-attributes query sent after it.
    No,
    /// It answered nothing before the deadline.
    NoAnswer,
}

/// What dux learned from tmux's own settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TmuxReport {
    /// `tmux show-options` failed or ran past its ceiling, with the reason.
    CommandFailed(String),
    /// tmux does not know `extended-keys`: older than 3.2, which has no
    /// extended keys at all.
    OlderThan32,
    /// `extended-keys` is `off`: tmux ignores every request. `needs_format`
    /// is whether this tmux has `extended-keys-format` (3.5 and newer, where
    /// the default is the xterm format), so the advice names that line too.
    ExtendedKeysOff { needs_format: bool },
    /// `extended-keys` is on but the format is `xterm`, which dux cannot read.
    XtermFormat,
    /// `extended-keys` is `on` or `always` and the format is `csi-u`, or the
    /// tmux is 3.2 to 3.4, which has no format option and sends only `csi-u`.
    Ready,
}

impl TmuxReport {
    /// Why tmux's extended keys stay off, for `dux.log`. `None` for
    /// [`Self::Ready`].
    pub(crate) fn reason(&self) -> Option<String> {
        match self {
            Self::Ready => None,
            Self::CommandFailed(reason) => Some(format!(
                "dux left tmux's extended keys off: it could not read tmux's settings ({reason})"
            )),
            Self::OlderThan32 => Some(
                "dux left tmux's extended keys off: this tmux has none, because extended keys \
                 need tmux 3.2 or newer"
                    .to_string(),
            ),
            Self::ExtendedKeysOff { needs_format: true } => Some(
                "dux left tmux's extended keys off: tmux's extended-keys option is off \
                 (set -s extended-keys on, and set -s extended-keys-format csi-u)"
                    .to_string(),
            ),
            Self::ExtendedKeysOff {
                needs_format: false,
            } => Some(
                "dux left tmux's extended keys off: tmux's extended-keys option is off \
                 (set -s extended-keys on)"
                    .to_string(),
            ),
            Self::XtermFormat => Some(
                "dux left tmux's extended keys off: tmux sends them in its xterm format, \
                 which dux cannot read (set -s extended-keys-format csi-u)"
                    .to_string(),
            ),
        }
    }

    /// The status-line warning raised when `extended_keys` is `on` and tmux is
    /// not set up. `None` for [`Self::Ready`]. The tmux.conf lines are names,
    /// so the plain spelling quotes them.
    pub(crate) fn warning(&self) -> Option<String> {
        use dux_core::status_text;
        let sentence = match self {
            Self::Ready => return None,
            Self::XtermFormat => status_text![
                "Extended keys are on, but tmux sends them in its xterm format, which dux \
                 cannot read, so dux left tmux's extended keys off. Add ",
                q("set -s extended-keys-format csi-u"),
                " to your tmux.conf and reload it."
            ],
            Self::ExtendedKeysOff { needs_format: true } => status_text![
                "Extended keys are on, but tmux's extended-keys option is off. Add ",
                q("set -s extended-keys on"),
                " and ",
                q("set -s extended-keys-format csi-u"),
                " to your tmux.conf and reload it."
            ],
            Self::ExtendedKeysOff {
                needs_format: false,
            } => status_text![
                "Extended keys are on, but tmux's extended-keys option is off. Add ",
                q("set -s extended-keys on"),
                " to your tmux.conf and reload it."
            ],
            Self::OlderThan32 => status_text![
                "Extended keys are on, but this tmux is older than 3.2 and has no extended \
                 keys for dux to turn on. Upgrade tmux to 3.2 or newer."
            ],
            Self::CommandFailed(reason) => status_text![format!(
                "Extended keys are on, but dux could not read tmux's settings ({reason}); \
                     dux.log has the details."
            )],
        };
        Some(String::from(status_text![
            sentence,
            " Then run the ",
            q("force-redraw"),
            " palette command."
        ]))
    }
}

/// How loudly a [`Plan`]'s log line is written to `dux.log`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogLevel {
    Info,
    Warn,
}

/// Where the terminal UI runs: straight in a host terminal, or inside tmux.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Host {
    #[default]
    Terminal,
    Tmux,
}

/// What to switch on, and what to say about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) push_kitty: bool,
    pub(crate) request_tmux: bool,
    pub(crate) log: Option<(LogLevel, String)>,
    pub(crate) warning: Option<String>,
}

/// Decide what to switch on. Outside tmux, `kitty` is the host's answer (asked
/// only in `auto`); under tmux, `tmux` is tmux's report, and Kitty is never
/// pushed: tmux answers the Kitty query "no" and keeps no Kitty stack for the
/// pane, so a push there would record a mode tmux never entered.
pub(crate) fn plan(
    mode: ExtendedKeysMode,
    host: Host,
    kitty: Option<KittyAnswer>,
    tmux: Option<&TmuxReport>,
) -> Plan {
    match (mode, host) {
        (ExtendedKeysMode::Off, _) => Plan::default(),
        (ExtendedKeysMode::On, Host::Terminal) => Plan {
            push_kitty: true,
            ..Plan::default()
        },
        (ExtendedKeysMode::Auto, Host::Terminal) => match kitty {
            Some(KittyAnswer::Yes) => Plan {
                push_kitty: true,
                ..Plan::default()
            },
            Some(KittyAnswer::NoAnswer) => Plan {
                log: Some((
                    LogLevel::Warn,
                    format!(
                        "the terminal did not answer dux's Kitty keyboard query within {} ms; \
                         extended keys stay off",
                        KITTY_QUERY_DEADLINE.as_millis()
                    ),
                )),
                ..Plan::default()
            },
            Some(KittyAnswer::No) | None => Plan::default(),
        },
        (ExtendedKeysMode::Auto | ExtendedKeysMode::On, Host::Tmux) => match tmux {
            None => Plan::default(),
            Some(TmuxReport::Ready) => Plan {
                request_tmux: true,
                ..Plan::default()
            },
            Some(report) => Plan {
                log: report.reason().map(|reason| (LogLevel::Info, reason)),
                warning: if mode == ExtendedKeysMode::On {
                    report.warning()
                } else {
                    None
                },
                ..Plan::default()
            },
        },
    }
}

/// How long dux waits, before the first frame, for tmux to name the tty of the
/// pane the environment points at.
pub(crate) const TMUX_PANE_CHECK_CEILING: Duration = Duration::from_secs(1);

/// Where this run really is, given where the environment says it is. `TMUX`
/// and `TERM_PROGRAM` are inherited by everything started from a tmux pane,
/// a terminal opened from it included, so they are only a hint: dux is under
/// tmux when the tmux pane `TMUX_PANE` names has dux's own stdin as its tty.
/// Runs `tmux` once, bounded by [`TMUX_PANE_CHECK_CEILING`], and only when the
/// environment says tmux.
pub(crate) fn verified_host(env_says: Host) -> Host {
    if env_says == Host::Terminal {
        return Host::Terminal;
    }
    let own_tty = rustix::termios::ttyname(std::io::stdin(), Vec::new())
        .map(|name| std::ffi::OsString::from_vec(name.into_bytes()))
        .map_err(|err| err.to_string());
    match tmux_pane_is_ours(
        std::ffi::OsStr::new("tmux"),
        std::env::var_os("TMUX_PANE").as_deref(),
        own_tty,
        TMUX_PANE_CHECK_CEILING,
    ) {
        Ok(()) => Host::Tmux,
        Err(reason) => {
            dux_core::logger::info(&format!(
                "the environment says tmux, but dux treats this terminal as outside tmux: \
                 {reason}"
            ));
            Host::Terminal
        }
    }
}

/// Whether the tmux pane `pane` names has `own_tty` as its tty, asked of
/// `program`; the reason when it does not or cannot be told.
pub(crate) fn tmux_pane_is_ours(
    program: &std::ffi::OsStr,
    pane: Option<&std::ffi::OsStr>,
    own_tty: Result<std::ffi::OsString, String>,
    ceiling: Duration,
) -> Result<(), String> {
    let Some(pane) = pane.filter(|pane| !pane.is_empty()) else {
        return Err("TMUX_PANE is not set".to_string());
    };
    let own_tty = own_tty.map_err(|err| format!("dux could not name its own tty ({err})"))?;
    let Some(pane) = pane.to_str() else {
        return Err(format!("TMUX_PANE {} is not text", pane.display()));
    };
    let pane_tty = run_tmux(
        program,
        &["display-message", "-p", "-t", pane, "#{pane_tty}"],
        "tmux display-message",
        ceiling,
    )?;
    if pane_tty.is_empty() {
        return Err(format!("tmux knows no pane {pane}"));
    }
    if std::ffi::OsStr::new(&pane_tty) == own_tty {
        Ok(())
    } else {
        Err(format!(
            "tmux pane {pane} is on {pane_tty}, and dux is on {}",
            own_tty.to_string_lossy()
        ))
    }
}

/// Read tmux's extended-keys settings with the `tmux` on `PATH`.
pub(crate) fn tmux_keys() -> TmuxReport {
    tmux_keys_with(std::ffi::OsStr::new("tmux"), TMUX_COMMAND_CEILING)
}

/// [`tmux_keys`] with the program and the per-command ceiling given.
///
/// tmux 3.2 added `extended-keys`; 3.5 added `extended-keys-format`, with
/// `xterm` as its default. A tmux without the format option (3.2 to 3.4)
/// sends extended keys only in the `csi-u` format, so its missing format reads
/// as `csi-u`.
pub(crate) fn tmux_keys_with(program: &std::ffi::OsStr, ceiling: Duration) -> TmuxReport {
    let keys = match show_server_option(program, "extended-keys", ceiling) {
        Ok(value) => value,
        Err(reason) if reason.contains("invalid option") => return TmuxReport::OlderThan32,
        Err(reason) => return TmuxReport::CommandFailed(reason),
    };
    let (format, has_format) = match show_server_option(program, "extended-keys-format", ceiling) {
        Ok(value) => (value, true),
        Err(reason) if reason.contains("invalid option") => ("csi-u".to_string(), false),
        Err(reason) => return TmuxReport::CommandFailed(reason),
    };
    match (keys.as_str(), format.as_str()) {
        ("off", _) => TmuxReport::ExtendedKeysOff {
            needs_format: has_format,
        },
        ("on" | "always", "csi-u") => TmuxReport::Ready,
        ("on" | "always", "xterm") => TmuxReport::XtermFormat,
        (keys, format) => TmuxReport::CommandFailed(format!(
            "unexpected extended-keys {keys:?} with extended-keys-format {format:?}"
        )),
    }
}

/// `tmux show-options -s -v <option>`, trimmed, or the reason it failed. The
/// child is killed when it runs past `ceiling`.
fn show_server_option(
    program: &std::ffi::OsStr,
    option: &str,
    ceiling: Duration,
) -> Result<String, String> {
    run_tmux(
        program,
        &["show-options", "-s", "-v", option],
        &format!("tmux show-options {option}"),
        ceiling,
    )
}

/// Run `program` with `args`, returning its trimmed output or the reason it
/// failed, each reason starting with `what`. The child is killed when it runs
/// past `ceiling`.
fn run_tmux(
    program: &std::ffi::OsStr,
    args: &[&str],
    what: &str,
    ceiling: Duration,
) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("could not run tmux: {err}"))?;
    let deadline = Instant::now() + ceiling;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{what} ran past {} ms", ceiling.as_millis()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("waiting for tmux failed: {err}"));
            }
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    if status.success() {
        Ok(stdout.trim().to_string())
    } else {
        let said = stderr.trim();
        Err(if said.is_empty() {
            format!("{what} exited with {status}")
        } else {
            format!("{what}: {said}")
        })
    }
}

/// The host's answer to the Kitty query, and the keys typed while dux waited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KittyQuery {
    pub(crate) answer: KittyAnswer,
    pub(crate) keys: Vec<KeyEvent>,
}

/// Ask the terminal on `input`/`output` whether it speaks the Kitty keyboard
/// protocol: write `CSI ? u` and then the primary device-attributes query, and
/// read until the attributes reply or `deadline`. A Kitty reply before the
/// attributes reply is a yes; the attributes reply alone is a no; silence is
/// no answer. Every other sequence read meanwhile is a key typed before the
/// first frame: decoded and kept, or dropped with a debug line.
pub(crate) fn query_kitty(
    input: impl AsFd,
    output: &mut impl Write,
    deadline: Duration,
) -> KittyQuery {
    use rustix::event::{PollFd, PollFlags, poll};

    let mut query = KittyQuery {
        answer: KittyAnswer::NoAnswer,
        keys: Vec::new(),
    };
    if write_all_and_flush(output, b"\x1b[?u\x1b[c").is_err() {
        return query;
    }
    let mut parser = crate::raw_input::RawInputParser::default();
    let mut kitty_seen = false;
    let mut answered = false;
    let ends_at = Instant::now() + deadline;
    let mut buf = [0u8; 1024];
    while !answered {
        let left = ends_at.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let timeout = rustix::time::Timespec {
            tv_sec: i64::try_from(left.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: left.subsec_nanos().into(),
        };
        let mut fds = [PollFd::new(&input, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => break,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => break,
        }
        let n = match rustix::io::read(&input, &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for parsed in parser.feed_sequences(&buf[..n]) {
            let seq = parsed.bytes.as_slice();
            if is_kitty_reply(seq) {
                kitty_seen = true;
            } else if !answered && is_device_attributes_reply(seq) {
                // The rest of this read is still walked: a key typed right
                // after the reply arrives in the same read.
                query.answer = if kitty_seen {
                    KittyAnswer::Yes
                } else {
                    KittyAnswer::No
                };
                answered = true;
            } else {
                keep_typed_key(seq, &mut query.keys);
            }
        }
    }
    for parsed in parser
        .feed_sequences(&[])
        .into_iter()
        .chain(parser.resolve_pending_esc())
    {
        keep_typed_key(&parsed.bytes, &mut query.keys);
    }
    if query.answer == KittyAnswer::NoAnswer && kitty_seen {
        query.answer = KittyAnswer::Yes;
    }
    query
}

fn keep_typed_key(seq: &[u8], keys: &mut Vec<KeyEvent>) {
    match crate::key_decode::decode_key(seq) {
        Some(key) => keys.push(key),
        None => dux_core::logger::debug(&format!(
            "dropped a key typed during the Kitty keyboard query that dux cannot read: {:?}",
            String::from_utf8_lossy(seq)
        )),
    }
}

/// `CSI ? flags u`, the Kitty query's reply.
fn is_kitty_reply(seq: &[u8]) -> bool {
    seq.strip_prefix(b"\x1b[?")
        .and_then(|rest| rest.strip_suffix(b"u"))
        .is_some_and(|flags| flags.iter().all(u8::is_ascii_digit))
}

/// `CSI ? … c`, the primary device-attributes reply.
fn is_device_attributes_reply(seq: &[u8]) -> bool {
    seq.strip_prefix(b"\x1b[?")
        .and_then(|rest| rest.strip_suffix(b"c"))
        .is_some_and(|params| params.iter().all(|b| b.is_ascii_digit() || *b == b';'))
}

/// The host's Kitty answer, asked once per process: the flip back from the web
/// server reuses it, and only the first call can carry typed keys.
pub(crate) fn kitty_answer_once(
    cell: &OnceLock<KittyAnswer>,
    ask: impl FnOnce() -> KittyQuery,
) -> KittyQuery {
    let mut asked = None;
    let answer = *cell.get_or_init(|| {
        let query = ask();
        let answer = query.answer;
        asked = Some(query);
        answer
    });
    asked.unwrap_or(KittyQuery {
        answer,
        keys: Vec::new(),
    })
}

/// The process's cached Kitty answer.
pub(crate) static KITTY_ANSWER: OnceLock<KittyAnswer> = OnceLock::new();

/// The App's part in the negotiation: the mode this run started with, its
/// tmux probes, the host's Kitty answer and the last tmux report, and the keys
/// typed during the Kitty query, which the first loop iteration handles.
#[derive(Debug)]
pub(crate) struct HostKeysState {
    pub(crate) mode: ExtendedKeysMode,
    /// Where this run is: `Tmux` only when [`verified_host`] confirmed it at
    /// start; `Terminal` otherwise, which includes a run with
    /// `[ui] extended_keys` off, where nothing was checked.
    pub(crate) host: Host,
    pub(crate) probes: TmuxProbes,
    /// The host's answer to the Kitty query, when this run asked it.
    pub(crate) kitty_answer: Option<KittyAnswer>,
    pub(crate) last_tmux_report: Option<TmuxReport>,
    pub(crate) startup_keys: Vec<KeyEvent>,
    /// The record this run switches extended keys on in: the process's own
    /// [`HOST_KEYS`], which the panic hook and the teardown also read. A test
    /// gives a run a record of its own.
    pub(crate) record: &'static HostKeysRecord,
}

impl Default for HostKeysState {
    fn default() -> Self {
        Self {
            mode: ExtendedKeysMode::default(),
            host: Host::default(),
            probes: TmuxProbes::default(),
            kitty_answer: None,
            last_tmux_report: None,
            startup_keys: Vec::new(),
            record: &HOST_KEYS,
        }
    }
}

/// The input debugger's header when dux reads classic keys because the setting
/// says so.
const ASKED_NOTHING_OFF: &str = "Extended keys are off ([ui] extended_keys): Shift-Enter, Ctrl-i \
     and Ctrl-Shift chords look like Enter, Tab and Ctrl chords.";
/// The header when the host terminal did not say it speaks Kitty keys.
const ASKED_NO_KITTY: &str = "Your terminal did not say it speaks the Kitty keyboard protocol: \
     Shift-Enter, Ctrl-i and Ctrl-Shift chords look like Enter, Tab and Ctrl chords.";

impl HostKeysState {
    /// What dux asked the host for in this run, for the input debugger's
    /// header: the setting, what the record says was switched on, the host's
    /// Kitty answer and tmux's last report. The entries under it say what
    /// arrived.
    pub(crate) fn asked_for(&self) -> String {
        if self.mode == ExtendedKeysMode::Off {
            return ASKED_NOTHING_OFF.to_string();
        }
        let switched_on = self.record.current();
        match self.host {
            Host::Terminal => {
                let answered_yes = self.kitty_answer == Some(KittyAnswer::Yes);
                if switched_on.kitty_pushed && answered_yes {
                    "dux asked your terminal for Kitty keys, and it said yes.".to_string()
                } else if switched_on.kitty_pushed {
                    "dux turned on Kitty keys without asking your terminal first \
                     ([ui] extended_keys is on)."
                        .to_string()
                } else if answered_yes || self.mode == ExtendedKeysMode::On {
                    "dux could not switch on Kitty keys in your terminal; dux.log has the \
                     details."
                        .to_string()
                } else {
                    ASKED_NO_KITTY.to_string()
                }
            }
            Host::Tmux => {
                if switched_on.tmux_extended {
                    return "dux asked tmux for extended keys in its csi-u format.".to_string();
                }
                match &self.last_tmux_report {
                    None => "dux is still reading tmux's settings.".to_string(),
                    Some(report) => report.reason().map_or_else(
                        || {
                            "tmux is set up for extended keys, but dux could not ask it for \
                             them; dux.log has the details."
                                .to_string()
                        },
                        |reason| format!("{reason}."),
                    ),
                }
            }
        }
    }
}

/// Tags each tmux probe so only the newest answer is applied: every force
/// redraw starts an independent probe, and an older answer arriving last would
/// switch tmux's mode back.
#[derive(Debug, Default)]
pub(crate) struct TmuxProbes {
    latest: u64,
}

impl TmuxProbes {
    /// Start a probe, returning its number.
    pub(crate) fn start(&mut self) -> u64 {
        self.latest += 1;
        self.latest
    }

    /// Whether `probe` is the newest one started.
    pub(crate) fn is_latest(&self, probe: u64) -> bool {
        probe == self.latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// Teardown pops what it pushed first, and only once. The bytes that leave
    /// the alternate screen are not among them: `ratatui::restore` writes those
    /// after this, on the normal exit and the panic path alike.
    #[test]
    fn a_kitty_switch_on_is_popped_first_at_teardown_and_only_once() {
        let record = HostKeysRecord::new();
        let mut on = Vec::new();
        switch_on_kitty(&mut on, &record).expect("write");
        assert_eq!(on, KITTY_PUSH);
        let mut teardown = Vec::new();
        crate::app::write_disable_input_modes(&mut teardown, &record);
        assert!(teardown.starts_with(KITTY_POP), "{teardown:?}");
        let leave = b"\x1b[?1049l";
        assert!(
            !teardown.windows(leave.len()).any(|w| w == leave),
            "the pop is written while still on the alternate screen: {teardown:?}"
        );
        let mut again = Vec::new();
        crate::app::write_disable_input_modes(&mut again, &record);
        assert!(
            !again.windows(KITTY_POP.len()).any(|w| w == KITTY_POP),
            "a second teardown pops nothing: {again:?}"
        );
    }

    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("closed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("closed"))
        }
    }

    #[test]
    fn both_bits_are_switched_off_once_and_a_failed_switch_on_leaves_its_bit_clear() {
        let record = HostKeysRecord::new();
        assert!(switch_on_kitty(&mut FailingWriter, &record).is_err());
        assert!(switch_on_tmux(&mut FailingWriter, &record).is_err());
        assert_eq!(record.current(), HostKeys::default());

        switch_on_kitty(&mut Vec::new(), &record).expect("write");
        switch_on_tmux(&mut Vec::new(), &record).expect("write");
        let mut out = Vec::new();
        write_switch_off(&mut out, &record);
        assert_eq!(out, [KITTY_POP, TMUX_RESET].concat());
        let mut again = Vec::new();
        write_switch_off(&mut again, &record);
        assert_eq!(again, Vec::<u8>::new());
    }

    #[test]
    fn with_nothing_switched_on_nothing_is_written() {
        let record = HostKeysRecord::new();
        let mut out = Vec::new();
        write_switch_off(&mut out, &record);
        reassert_kitty(&mut out, &record);
        switch_off_tmux(&mut out, &record);
        assert_eq!(out, Vec::<u8>::new());
        assert_eq!(
            plan(ExtendedKeysMode::Off, Host::Terminal, None, None),
            Plan::default()
        );
        assert_eq!(
            plan(
                ExtendedKeysMode::Off,
                Host::Tmux,
                Some(KittyAnswer::Yes),
                Some(&TmuxReport::Ready)
            ),
            Plan::default()
        );
    }

    #[test]
    fn a_force_redraw_reasserts_without_pushing_and_tmux_can_be_handed_back() {
        let record = HostKeysRecord::new();
        switch_on_kitty(&mut Vec::new(), &record).expect("write");
        switch_on_tmux(&mut Vec::new(), &record).expect("write");
        let mut out = Vec::new();
        reassert_kitty(&mut out, &record);
        assert_eq!(out, KITTY_REASSERT);
        let mut off = Vec::new();
        switch_off_tmux(&mut off, &record);
        switch_off_tmux(&mut off, &record);
        assert_eq!(off, TMUX_RESET, "reset once");
        assert_eq!(
            record.current(),
            HostKeys {
                kitty_pushed: true,
                tmux_extended: false
            }
        );
    }

    fn push() -> Plan {
        Plan {
            push_kitty: true,
            ..Plan::default()
        }
    }

    fn request() -> Plan {
        Plan {
            request_tmux: true,
            ..Plan::default()
        }
    }

    #[test]
    fn the_planner_gives_each_row_of_the_table() {
        use ExtendedKeysMode::{Auto, Off, On};
        use KittyAnswer::{No, NoAnswer, Yes};
        let reports = [
            TmuxReport::Ready,
            TmuxReport::XtermFormat,
            TmuxReport::ExtendedKeysOff { needs_format: true },
            TmuxReport::ExtendedKeysOff {
                needs_format: false,
            },
            TmuxReport::OlderThan32,
            TmuxReport::CommandFailed("no server running".to_string()),
        ];
        for host in [Host::Terminal, Host::Tmux] {
            for kitty in [None, Some(Yes), Some(No), Some(NoAnswer)] {
                for report in reports.iter().map(Some).chain([None]) {
                    assert_eq!(plan(Off, host, kitty, report), Plan::default());
                }
            }
        }
        // Outside tmux.
        assert_eq!(plan(Auto, Host::Terminal, Some(Yes), None), push());
        assert_eq!(plan(Auto, Host::Terminal, Some(No), None), Plan::default());
        let silent = plan(Auto, Host::Terminal, Some(NoAnswer), None);
        assert!(!silent.push_kitty && !silent.request_tmux && silent.warning.is_none());
        assert_eq!(silent.log.map(|(level, _)| level), Some(LogLevel::Warn));
        assert_eq!(plan(On, Host::Terminal, None, None), push());
        // Under tmux: never Kitty, whatever the host would answer.
        for mode in [Auto, On] {
            for kitty in [None, Some(Yes)] {
                assert_eq!(
                    plan(mode, Host::Tmux, kitty, Some(&TmuxReport::Ready)),
                    request()
                );
                assert_eq!(plan(mode, Host::Tmux, kitty, None), Plan::default());
            }
        }
        for report in &reports[1..] {
            let auto = plan(Auto, Host::Tmux, None, Some(report));
            assert!(!auto.push_kitty && !auto.request_tmux && auto.warning.is_none());
            assert_eq!(
                auto.log.as_ref().map(|(level, _)| *level),
                Some(LogLevel::Info)
            );
            let on = plan(On, Host::Tmux, None, Some(report));
            assert!(!on.push_kitty && !on.request_tmux);
            assert_eq!(on.warning, report.warning());
            assert!(on.log.is_some());
        }
    }

    #[test]
    fn each_tmux_report_warns_in_its_own_words() {
        use ExtendedKeysMode::On;
        assert_eq!(
            plan(On, Host::Tmux, None, Some(&TmuxReport::XtermFormat))
                .warning
                .as_deref(),
            Some(
                "Extended keys are on, but tmux sends them in its xterm format, which dux \
                 cannot read, so dux left tmux's extended keys off. Add \"set -s \
                 extended-keys-format csi-u\" to your tmux.conf and reload it. Then run the \
                 \"force-redraw\" palette command."
            )
        );
        assert_eq!(
            plan(
                On,
                Host::Tmux,
                None,
                Some(&TmuxReport::ExtendedKeysOff { needs_format: true })
            )
            .warning
            .as_deref(),
            Some(
                "Extended keys are on, but tmux's extended-keys option is off. Add \"set -s \
                 extended-keys on\" and \"set -s extended-keys-format csi-u\" to your \
                 tmux.conf and reload it. Then run the \"force-redraw\" palette command."
            )
        );
        assert_eq!(
            plan(
                On,
                Host::Tmux,
                None,
                Some(&TmuxReport::ExtendedKeysOff {
                    needs_format: false
                })
            )
            .warning
            .as_deref(),
            Some(
                "Extended keys are on, but tmux's extended-keys option is off. Add \"set -s \
                 extended-keys on\" to your tmux.conf and reload it. Then run the \
                 \"force-redraw\" palette command."
            )
        );
        assert_eq!(
            plan(On, Host::Tmux, None, Some(&TmuxReport::OlderThan32))
                .warning
                .as_deref(),
            Some(
                "Extended keys are on, but this tmux is older than 3.2 and has no extended \
                 keys for dux to turn on. Upgrade tmux to 3.2 or newer. Then run the \
                 \"force-redraw\" palette command."
            )
        );
        assert_eq!(
            plan(
                On,
                Host::Tmux,
                None,
                Some(&TmuxReport::CommandFailed("no server running".to_string()))
            )
            .warning
            .as_deref(),
            Some(
                "Extended keys are on, but dux could not read tmux's settings (no server \
                 running); dux.log has the details. Then run the \"force-redraw\" palette \
                 command."
            )
        );
    }

    /// A stand-in `tmux` answering `extended-keys` and `extended-keys-format`
    /// from the script's own text.
    fn stand_in(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("tmux");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    #[test]
    fn tmux_keys_maps_each_answer_to_its_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        let answers = |keys: &str, format: &str| {
            format!(
                "case \"$4\" in\n  extended-keys) {keys} ;;\n  extended-keys-format) {format} ;;\nesac"
            )
        };
        let cases = [
            (answers("echo on", "echo csi-u"), TmuxReport::Ready),
            (answers("echo always", "echo csi-u"), TmuxReport::Ready),
            (answers("echo on", "echo xterm"), TmuxReport::XtermFormat),
            (
                answers("echo off", "echo csi-u"),
                TmuxReport::ExtendedKeysOff { needs_format: true },
            ),
            // tmux 3.2 to 3.4: no format option, and only csi-u output.
            (
                answers(
                    "echo on",
                    "echo 'invalid option: extended-keys-format' >&2; exit 1",
                ),
                TmuxReport::Ready,
            ),
            (
                answers(
                    "echo always",
                    "echo 'invalid option: extended-keys-format' >&2; exit 1",
                ),
                TmuxReport::Ready,
            ),
            (
                answers(
                    "echo off",
                    "echo 'invalid option: extended-keys-format' >&2; exit 1",
                ),
                TmuxReport::ExtendedKeysOff {
                    needs_format: false,
                },
            ),
            // Older than 3.2: no extended-keys option at all.
            (
                answers(
                    "echo 'invalid option: extended-keys' >&2; exit 1",
                    "echo 'invalid option: extended-keys-format' >&2; exit 1",
                ),
                TmuxReport::OlderThan32,
            ),
        ];
        for (body, expected) in cases {
            let program = stand_in(dir.path(), &body);
            assert_eq!(
                tmux_keys_with(program.as_os_str(), TMUX_COMMAND_CEILING),
                expected,
                "{body}"
            );
        }
        let failing = stand_in(dir.path(), "echo 'no server running' >&2; exit 1");
        assert_eq!(
            tmux_keys_with(failing.as_os_str(), TMUX_COMMAND_CEILING),
            TmuxReport::CommandFailed(
                "tmux show-options extended-keys: no server running".to_string()
            )
        );
        let hanging = stand_in(dir.path(), "exec sleep 30");
        let started = Instant::now();
        let report = tmux_keys_with(hanging.as_os_str(), Duration::from_millis(200));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the hang was cut short"
        );
        assert!(
            matches!(&report, TmuxReport::CommandFailed(reason) if reason.contains("ran past")),
            "{report:?}"
        );
        assert!(matches!(
            tmux_keys_with(dir.path().join("missing").as_os_str(), TMUX_COMMAND_CEILING),
            TmuxReport::CommandFailed(_)
        ));
    }

    fn answered(replies: &[u8], deadline: Duration) -> (KittyQuery, Vec<u8>) {
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let mut host = theirs.try_clone().expect("clone");
        host.write_all(replies).expect("reply");
        let mut written = Vec::new();
        let query = query_kitty(&ours, &mut written, deadline);
        drop(theirs);
        (query, written)
    }

    #[test]
    fn query_kitty_reads_the_answer_and_keeps_keys_typed_before_it() {
        let (yes, written) = answered(b"\x1b[?1u\x1b[?62c", KITTY_QUERY_DEADLINE);
        assert_eq!(written, b"\x1b[?u\x1b[c");
        assert_eq!(yes.answer, KittyAnswer::Yes);
        assert_eq!(yes.keys, Vec::<KeyEvent>::new());

        let (no, _) = answered(b"\x1b[?62;22c", KITTY_QUERY_DEADLINE);
        assert_eq!(no.answer, KittyAnswer::No);

        let (typed, _) = answered(b"x\x1b[?1u\x1b[?62c", KITTY_QUERY_DEADLINE);
        assert_eq!(typed.answer, KittyAnswer::Yes);
        assert_eq!(typed.keys.len(), 1);
        assert_eq!(typed.keys[0].code, crossterm::event::KeyCode::Char('x'));

        let (after, _) = answered(b"\x1b[?1u\x1b[?62cx", KITTY_QUERY_DEADLINE);
        assert_eq!(after.answer, KittyAnswer::Yes);
        assert_eq!(
            after.keys.iter().map(|key| key.code).collect::<Vec<_>>(),
            [crossterm::event::KeyCode::Char('x')],
            "a key in the same read as the reply is kept"
        );

        let started = Instant::now();
        let (silent, _) = answered(b"", Duration::from_millis(150));
        assert_eq!(silent.answer, KittyAnswer::NoAnswer);
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn query_kitty_reads_a_reply_split_across_two_reads() {
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let mut host = theirs.try_clone().expect("clone");
        host.write_all(b"\x1b[?1u\x1b[?6").expect("first half");
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            host.write_all(b"2c").expect("second half");
        });
        let query = query_kitty(&ours, &mut Vec::new(), KITTY_QUERY_DEADLINE);
        late.join().expect("writer");
        drop(theirs);
        assert_eq!(query.answer, KittyAnswer::Yes);
        assert_eq!(query.keys, Vec::<KeyEvent>::new());
    }

    #[test]
    fn a_tmux_request_is_written_every_time_the_plan_asks() {
        let record = HostKeysRecord::new();
        let mut out = Vec::new();
        apply_tmux_plan(&mut out, &record, &request()).expect("write");
        apply_tmux_plan(&mut out, &record, &request()).expect("write");
        assert_eq!(out, [TMUX_REQUEST, TMUX_REQUEST].concat());
        assert!(record.current().tmux_extended);
        let mut off = Vec::new();
        apply_tmux_plan(&mut off, &record, &Plan::default()).expect("write");
        apply_tmux_plan(&mut off, &record, &Plan::default()).expect("write");
        assert_eq!(off, TMUX_RESET, "reset once");
        assert!(!record.current().tmux_extended);
    }

    /// A stand-in `tmux` that names `/dev/pts/7` as pane `%3`'s tty only when
    /// asked exactly that question, and leaves a mark whenever it runs.
    fn pane_stand_in(dir: &std::path::Path, answer: &str) -> std::path::PathBuf {
        let mark = dir.join("ran");
        stand_in(
            dir,
            &format!(
                "touch '{}'\nif [ \"$1 $2 $3 $4 $5\" = 'display-message -p -t %3 #{{pane_tty}}' ]; \
                 then {answer}; else exit 9; fi",
                mark.display()
            ),
        )
    }

    #[test]
    fn tmux_is_the_host_only_when_the_named_pane_is_on_dux_own_tty() {
        let pts7 = || Ok(std::ffi::OsString::from("/dev/pts/7"));
        let pane = Some(std::ffi::OsStr::new("%3"));
        let ceiling = TMUX_PANE_CHECK_CEILING;
        let dir = tempfile::tempdir().expect("tempdir");

        let matching = pane_stand_in(dir.path(), "echo /dev/pts/7");
        assert_eq!(
            tmux_pane_is_ours(matching.as_os_str(), pane, pts7(), ceiling),
            Ok(())
        );

        let other = pane_stand_in(dir.path(), "echo /dev/pts/8");
        let mismatch = tmux_pane_is_ours(other.as_os_str(), pane, pts7(), ceiling);
        assert!(
            matches!(&mismatch, Err(reason) if reason.contains("/dev/pts/8")),
            "{mismatch:?}"
        );

        // A pane id tmux does not know prints nothing and exits 0.
        let empty = pane_stand_in(dir.path(), "echo");
        assert!(tmux_pane_is_ours(empty.as_os_str(), pane, pts7(), ceiling).is_err());

        let failing = pane_stand_in(dir.path(), "echo 'no server running' >&2; exit 1");
        assert!(tmux_pane_is_ours(failing.as_os_str(), pane, pts7(), ceiling).is_err());

        let hanging = pane_stand_in(dir.path(), "exec sleep 30");
        let started = Instant::now();
        assert!(
            tmux_pane_is_ours(
                hanging.as_os_str(),
                pane,
                pts7(),
                Duration::from_millis(200)
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(5));

        let no_tty = tmux_pane_is_ours(
            matching.as_os_str(),
            pane,
            Err("not a tty".to_string()),
            ceiling,
        );
        assert!(no_tty.is_err(), "{no_tty:?}");

        let mark = dir.path().join("ran");
        let _ = std::fs::remove_file(&mark);
        for unset in [None, Some(std::ffi::OsStr::new(""))] {
            assert!(tmux_pane_is_ours(matching.as_os_str(), unset, pts7(), ceiling).is_err());
        }
        assert!(!mark.exists(), "no TMUX_PANE, no tmux run");
        assert_eq!(verified_host(Host::Terminal), Host::Terminal);
    }

    #[test]
    fn the_kitty_answer_is_asked_at_most_once_per_process() {
        let cell = OnceLock::new();
        let mut asks = 0;
        let first = kitty_answer_once(&cell, || {
            asks += 1;
            KittyQuery {
                answer: KittyAnswer::Yes,
                keys: vec![KeyEvent::from(crossterm::event::KeyCode::Char('x'))],
            }
        });
        assert_eq!(first.keys.len(), 1);
        let second = kitty_answer_once(&cell, || {
            asks += 1;
            KittyQuery {
                answer: KittyAnswer::No,
                keys: Vec::new(),
            }
        });
        assert_eq!(asks, 1);
        assert_eq!(second.answer, KittyAnswer::Yes);
        assert_eq!(second.keys, Vec::<KeyEvent>::new());
    }

    #[test]
    fn only_the_newest_tmux_probe_is_applied() {
        let mut probes = TmuxProbes::default();
        let older = probes.start();
        let newer = probes.start();
        assert!(!probes.is_latest(older));
        assert!(probes.is_latest(newer));
    }
}
