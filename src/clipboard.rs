use std::io::Write;
use std::sync::mpsc;

use anyhow::{Result, anyhow};

use crate::app::WorkerEvent;

/// Maximum payload size for an OSC 52 copy. Many terminals refuse longer
/// sequences. 100 KiB matches the limit used by tmux / WezTerm.
const OSC52_MAX_BYTES: usize = 100_000;

/// Request sent from the main thread to the clipboard worker.
struct CopyRequest {
    text: String,
    label: String,
    worker_tx: mpsc::Sender<WorkerEvent>,
}

/// Handle for sending clipboard copy requests to a long-lived background
/// thread. The background thread owns the `arboard::Clipboard` instance so
/// it stays alive for the entire app lifetime — this is required on X11
/// where the clipboard owner must remain running to serve paste requests.
pub(crate) struct Clipboard {
    tx: mpsc::Sender<CopyRequest>,
}

impl Clipboard {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel::<CopyRequest>();

        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                clipboard_worker(rx);
            })
            .expect("failed to spawn clipboard worker thread");

        Self { tx }
    }

    /// Send a clipboard copy request. Returns immediately — the result will
    /// arrive later as a `WorkerEvent::ClipboardCopyCompleted`.
    ///
    /// `label` is the human-readable success message shown in the status bar
    /// when the copy completes.
    pub(crate) fn copy_text(
        &self,
        text: &str,
        label: &str,
        worker_tx: &mpsc::Sender<WorkerEvent>,
    ) -> Result<()> {
        self.tx
            .send(CopyRequest {
                text: text.to_string(),
                label: label.to_string(),
                worker_tx: worker_tx.clone(),
            })
            .map_err(|_| anyhow!("Clipboard worker thread is not running"))?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn from_fn(copy_text_fn: fn(&str) -> Result<()>) -> Self {
        let (tx, rx) = mpsc::channel::<CopyRequest>();

        std::thread::Builder::new()
            .name("clipboard-test".into())
            .spawn(move || {
                while let Ok(req) = rx.recv() {
                    let result = (copy_text_fn)(&req.text).map_err(|e| e.to_string());
                    let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
                        label: req.label,
                        result,
                    });
                }
            })
            .expect("failed to spawn test clipboard thread");

        Self { tx }
    }
}

fn no_display() -> bool {
    std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none()
}

/// Minimal RFC 4648 base64 encoder so we don't pull in a dep just for this.
fn base64_encode(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut chunks = data.chunks_exact(3);
    for chunk in chunks.by_ref() {
        let b = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        out.push(A[((b >> 18) & 0x3f) as usize] as char);
        out.push(A[((b >> 12) & 0x3f) as usize] as char);
        out.push(A[((b >> 6) & 0x3f) as usize] as char);
        out.push(A[(b & 0x3f) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let b = u32::from(rem[0]) << 16;
            out.push(A[((b >> 18) & 0x3f) as usize] as char);
            out.push(A[((b >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let b = (u32::from(rem[0]) << 16) | (u32::from(rem[1]) << 8);
            out.push(A[((b >> 18) & 0x3f) as usize] as char);
            out.push(A[((b >> 12) & 0x3f) as usize] as char);
            out.push(A[((b >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Format an OSC 52 escape sequence for the system clipboard ("c").
/// Terminator is BEL (`\x07`) which is more widely supported than ST.
fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

/// Emit an OSC 52 copy via /dev/tty so the escape reaches the controlling
/// terminal regardless of stdout/stderr redirection. The host terminal
/// (xterm, WezTerm, kitty, alacritty, VSCode integrated terminal, tmux with
/// `set-clipboard on`, …) intercepts the sequence and writes the payload
/// to the local system clipboard. This is the standard way to copy from a
/// remote shell over SSH without an X server.
fn osc52_copy(text: &str) -> Result<()> {
    if text.len() > OSC52_MAX_BYTES {
        return Err(anyhow!(
            "text too large for OSC 52 clipboard ({} bytes; cap is {})",
            text.len(),
            OSC52_MAX_BYTES
        ));
    }
    let mut tty = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .map_err(|e| anyhow!("cannot open /dev/tty for OSC 52: {e}"))?;
    tty.write_all(osc52_sequence(text).as_bytes())
        .map_err(|e| anyhow!("OSC 52 write failed: {e}"))?;
    tty.flush().ok();
    Ok(())
}

/// Drain pending requests using OSC 52 only. Used when arboard is
/// unavailable (e.g. headless SSH session with no $DISPLAY).
fn osc52_only_loop(rx: mpsc::Receiver<CopyRequest>) {
    while let Ok(req) = rx.recv() {
        let result = osc52_copy(&req.text).map_err(|e| format!("Failed to copy to clipboard: {e}"));
        let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
            label: req.label,
            result,
        });
    }
}

fn clipboard_worker(rx: mpsc::Receiver<CopyRequest>) {
    // No display server → arboard's X11 path will hang/timeout. Skip
    // straight to OSC 52, which works over SSH and through VSCode's
    // integrated terminal.
    if no_display() {
        osc52_only_loop(rx);
        return;
    }

    let mut board = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(_) => {
            // Display vars set but arboard still failed (broken X auth,
            // unreachable server, etc.). Fall through to OSC 52 rather
            // than giving up.
            osc52_only_loop(rx);
            return;
        }
    };

    while let Ok(req) = rx.recv() {
        let result = match board.set_text(&req.text) {
            Ok(()) => Ok(()),
            // Per-request fallback: arboard initialized but a single set
            // failed (server timeout, lost selection ownership, etc.).
            Err(_) => {
                osc52_copy(&req.text).map_err(|e| format!("Failed to copy to clipboard: {e}"))
            }
        };
        let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
            label: req.label,
            result,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_from_fn_sends_and_receives() {
        let (worker_tx, worker_rx) = mpsc::channel();
        let clipboard = Clipboard::from_fn(|_| Ok(()));
        clipboard.copy_text("hello", "Copied.", &worker_tx).unwrap();

        let event = worker_rx.recv().unwrap();
        match event {
            WorkerEvent::ClipboardCopyCompleted { label, result } => {
                assert_eq!(label, "Copied.");
                assert!(result.is_ok());
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn clipboard_from_fn_reports_errors() {
        let (worker_tx, worker_rx) = mpsc::channel();
        let clipboard = Clipboard::from_fn(|_| Err(anyhow!("test error")));
        clipboard.copy_text("hello", "Copied.", &worker_tx).unwrap();

        let event = worker_rx.recv().unwrap();
        match event {
            WorkerEvent::ClipboardCopyCompleted { label, result } => {
                assert_eq!(label, "Copied.");
                assert!(result.unwrap_err().contains("test error"));
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn base64_known_vectors() {
        // RFC 4648 §10 test vectors.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn osc52_sequence_format() {
        let s = osc52_sequence("hi");
        // ESC ] 52 ; c ; <base64> BEL
        assert!(s.starts_with("\x1b]52;c;"));
        assert!(s.ends_with('\x07'));
        let payload = &s[7..s.len() - 1];
        assert_eq!(payload, "aGk=");
    }
}
