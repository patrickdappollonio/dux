//! Client-side remote-share session driver.
//!
//! Decode the pairing code, dial the host, complete the handshake, then
//! render incoming `RemoteMessage` frames to the local terminal while
//! forwarding local keyboard events back as `RemoteMessage::InputKey`.
//! The client is deliberately minimal — its job is to mirror the host's
//! composited output, not to reimplement dux.

use std::io::{Stdout, Write, stdout};

use anyhow::{Context, Result};
use crossterm::cursor::MoveTo;
use crossterm::event::{EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::style::{
    Attribute as CtAttribute, Color as CtColor, Print, ResetColor, SetAttribute,
    SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use ratatui::style::Modifier;
use tokio::sync::mpsc;

use super::PROTOCOL_VERSION;
use super::codec::{decode, encode};
use super::endpoint::{bind_client_endpoint, decode_endpoint_addr};
use super::handshake::client_handshake;
use super::iroh_transport::{ALPN, IrohTransport};
use super::messages::{RemoteMessage, WireCell, WireColor};
use super::pairing::decode_pairing_code;
use super::tee_backend::{from_wire_color, modifier_from_bits};
use super::transport::Transport;

/// Run the client end-to-end: dial, handshake, render frames, forward
/// keystrokes until either the host closes the connection or the user
/// aborts (Ctrl-C or the host signalling `Bye`).
///
/// `relay_url` lets the caller point the client at a custom iroh relay
/// (matching the host's `[remote].relay_url` config). Pass `None` to
/// use iroh's public relay mesh.
pub async fn run_client(code: &str, client_label: &str, relay_url: Option<String>) -> Result<()> {
    let payload = decode_pairing_code(code).context("pairing code is invalid")?;
    let addr = decode_endpoint_addr(&payload.endpoint_addr)
        .context("pairing code contained a malformed endpoint address")?;

    let endpoint = bind_client_endpoint(relay_url.as_deref()).await?;
    let conn = endpoint
        .connect(addr, ALPN)
        .await
        .context("iroh: failed to connect to host")?;
    let mut transport = IrohTransport::dial(conn).await?;

    let host_label = client_handshake(&mut transport, &payload.pin, client_label)
        .await
        .context("remote: handshake rejected by host")?;
    eprintln!(
        "connected to '{host_label}' (protocol v{PROTOCOL_VERSION}); press Ctrl-C to disconnect"
    );

    enable_raw_mode().context("enable raw mode")?;
    execute!(
        stdout(),
        crossterm::terminal::EnterAlternateScreen,
        EnableMouseCapture
    )
    .context("enter alternate screen")?;

    let result = render_and_input_loop(&mut transport).await;

    let _ = execute!(
        stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen,
        ResetColor
    );
    let _ = disable_raw_mode();
    result
}

/// Concurrently drive two streams:
///
/// 1. `transport.recv()` — incoming frame diffs → local repaint.
/// 2. Local keyboard events (read via `spawn_blocking`) → outbound
///    `RemoteMessage::InputKey`.
async fn render_and_input_loop<T: Transport>(transport: &mut T) -> Result<()> {
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();

    // Spawn a blocking task that reads crossterm events and forwards
    // KeyEvents onto a channel. Blocking because `crossterm::event::read`
    // is a sync call; the event poll loop lives on a tokio blocking
    // worker thread.
    tokio::task::spawn_blocking(move || {
        loop {
            match crossterm::event::read() {
                Ok(Event::Key(key)) => {
                    if key_tx.send(key).is_err() {
                        break;
                    }
                }
                // Non-key events are ignored for MVP. Mouse + paste can be
                // added later through a richer wire type.
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    let mut out = stdout();
    loop {
        tokio::select! {
            msg = transport.recv() => {
                let bytes = match msg {
                    Ok(b) => b,
                    Err(err) => {
                        eprintln!("\nremote: host closed connection: {err:#}");
                        return Ok(());
                    }
                };
                let decoded: RemoteMessage = decode(&bytes)?;
                if handle_incoming(&mut out, transport, decoded).await? {
                    return Ok(());
                }
            }
            Some(key) = key_rx.recv() => {
                if is_quit_shortcut(&key) {
                    eprintln!("\nremote: disconnecting (Ctrl-Q)");
                    return Ok(());
                }
                let msg = RemoteMessage::InputKey {
                    key: dux_remote::key_translate::from_crossterm(key),
                };
                if let Err(err) = transport.send(&encode(&msg)?).await {
                    eprintln!("\nremote: failed to forward keypress: {err:#}");
                    return Ok(());
                }
            }
        }
    }
}

/// Returns `Ok(true)` to signal the render loop should exit (on a host-
/// initiated `Bye`).
async fn handle_incoming<T: Transport>(
    out: &mut Stdout,
    transport: &mut T,
    msg: RemoteMessage,
) -> Result<bool> {
    match msg {
        RemoteMessage::FrameDiff { cells, .. } => {
            paint_cells(out, &cells)?;
            out.flush()?;
            Ok(false)
        }
        RemoteMessage::FullFrame {
            cols, rows, cells, ..
        } => {
            execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;
            if cols > 0 && rows > 0 {
                paint_cells(out, &cells)?;
            }
            out.flush()?;
            Ok(false)
        }
        RemoteMessage::Resize { cols, rows } => {
            eprintln!("\nremote: host is {cols}x{rows}");
            Ok(false)
        }
        RemoteMessage::Bye { reason } => {
            eprintln!("\nremote: host said goodbye: {reason:?}");
            Ok(true)
        }
        RemoteMessage::Ping { nonce } => {
            let _ = transport
                .send(&encode(&RemoteMessage::Pong { nonce })?)
                .await;
            Ok(false)
        }
        // Host → client informational or client-only variants: ignore.
        RemoteMessage::PtySnapshotDiff { .. }
        | RemoteMessage::Hello { .. }
        | RemoteMessage::Input { .. }
        | RemoteMessage::InputKey { .. }
        | RemoteMessage::LeaderChange { .. }
        | RemoteMessage::LeaderRequest
        | RemoteMessage::LeaderResponse { .. }
        | RemoteMessage::Pong { .. } => Ok(false),
    }
}

/// The client exits on Ctrl-Q. We choose Ctrl-Q (not Ctrl-C) because
/// Ctrl-C is a common keystroke a user will want to forward to the host
/// (e.g. to cancel an agent command). There is no ambiguity in a
/// view/control context: Ctrl-Q always means "quit the client".
fn is_quit_shortcut(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn paint_cells(out: &mut Stdout, cells: &[WireCell]) -> Result<()> {
    for cell in cells {
        let modifier = modifier_from_bits(cell.modifier);
        // Reset before each cell so the per-cell attribute set below is
        // authoritative. Without this, a bold cell followed by a plain
        // cell would remain bold because we'd never cleared the
        // previous attributes.
        execute!(out, SetAttribute(CtAttribute::Reset))?;
        apply_modifier(out, modifier)?;
        execute!(
            out,
            MoveTo(cell.col, cell.row),
            SetForegroundColor(to_crossterm_color(cell.fg)),
            SetBackgroundColor(to_crossterm_color(cell.bg)),
            Print(&cell.symbol),
        )?;
    }
    execute!(out, SetAttribute(CtAttribute::Reset), ResetColor)?;
    Ok(())
}

/// Map ratatui `Modifier` flags to crossterm `Attribute` and apply them
/// to the stdout stream. Each flag that matches a ratatui bit emits one
/// `SetAttribute` call; flags with no crossterm equivalent are dropped.
fn apply_modifier(out: &mut Stdout, modifier: Modifier) -> Result<()> {
    // Ordered roughly by visual weight so a future reader can tell at a
    // glance which ratatui flags round-trip. Hidden/RapidBlink map
    // cleanly; ratatui's `CROSSED_OUT` corresponds to crossterm's
    // `CrossedOut`.
    if modifier.contains(Modifier::BOLD) {
        execute!(out, SetAttribute(CtAttribute::Bold))?;
    }
    if modifier.contains(Modifier::DIM) {
        execute!(out, SetAttribute(CtAttribute::Dim))?;
    }
    if modifier.contains(Modifier::ITALIC) {
        execute!(out, SetAttribute(CtAttribute::Italic))?;
    }
    if modifier.contains(Modifier::UNDERLINED) {
        execute!(out, SetAttribute(CtAttribute::Underlined))?;
    }
    if modifier.contains(Modifier::SLOW_BLINK) {
        execute!(out, SetAttribute(CtAttribute::SlowBlink))?;
    }
    if modifier.contains(Modifier::RAPID_BLINK) {
        execute!(out, SetAttribute(CtAttribute::RapidBlink))?;
    }
    if modifier.contains(Modifier::REVERSED) {
        execute!(out, SetAttribute(CtAttribute::Reverse))?;
    }
    if modifier.contains(Modifier::HIDDEN) {
        execute!(out, SetAttribute(CtAttribute::Hidden))?;
    }
    if modifier.contains(Modifier::CROSSED_OUT) {
        execute!(out, SetAttribute(CtAttribute::CrossedOut))?;
    }
    Ok(())
}

fn to_crossterm_color(c: WireColor) -> CtColor {
    let rc = from_wire_color(c);
    match rc {
        ratatui::style::Color::Reset => CtColor::Reset,
        ratatui::style::Color::Black => CtColor::Black,
        ratatui::style::Color::Red => CtColor::DarkRed,
        ratatui::style::Color::Green => CtColor::DarkGreen,
        ratatui::style::Color::Yellow => CtColor::DarkYellow,
        ratatui::style::Color::Blue => CtColor::DarkBlue,
        ratatui::style::Color::Magenta => CtColor::DarkMagenta,
        ratatui::style::Color::Cyan => CtColor::DarkCyan,
        ratatui::style::Color::Gray => CtColor::Grey,
        ratatui::style::Color::DarkGray => CtColor::DarkGrey,
        ratatui::style::Color::LightRed => CtColor::Red,
        ratatui::style::Color::LightGreen => CtColor::Green,
        ratatui::style::Color::LightYellow => CtColor::Yellow,
        ratatui::style::Color::LightBlue => CtColor::Blue,
        ratatui::style::Color::LightMagenta => CtColor::Magenta,
        ratatui::style::Color::LightCyan => CtColor::Cyan,
        ratatui::style::Color::White => CtColor::White,
        ratatui::style::Color::Rgb(r, g, b) => CtColor::Rgb { r, g, b },
        ratatui::style::Color::Indexed(i) => CtColor::AnsiValue(i),
    }
}
