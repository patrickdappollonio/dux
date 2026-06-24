use std::sync::mpsc;

use anyhow::{Result, anyhow};
use dux_core::engine::{Final, StatusUpdate, status_op};

use crate::app::WorkerEvent;

/// Request sent from the main thread to the clipboard worker. Carries the
/// op's success and failure finals (already keyed by the StatusOp minted at
/// the call site), so the worker can ship back the resolved `status` without
/// the engine having to know the per-call label.
struct CopyRequest {
    text: String,
    label: String,
    op: dux_core::engine::StatusOp<(), String>,
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

    /// Send a clipboard copy request. Returns immediately with the keyed
    /// pending [`StatusUpdate`] for the busy spinner; the matching final
    /// arrives later as a `WorkerEvent::ClipboardCopyCompleted` carrying the
    /// resolved status. Callers that want the spinner apply the returned
    /// pending; fire-and-forget callers can drop it.
    ///
    /// `label` is the human-readable success message shown in the status bar
    /// when the copy completes.
    pub(crate) fn copy_text(
        &self,
        text: &str,
        label: &str,
        worker_tx: &mpsc::Sender<WorkerEvent>,
    ) -> Result<StatusUpdate> {
        // Declare the loading→final states together. The success message is the
        // label; the failure message matches the engine's prior wording.
        let success_label = label.to_string();
        let op = status_op("Copying path to clipboard\u{2026}")
            .on_success(move |_: &()| Final::info(success_label.clone()))
            .on_failure(|e: &String| Final::error(format!("Clipboard copy failed: {e}")));
        let pending = op.pending_status();
        self.tx
            .send(CopyRequest {
                text: text.to_string(),
                label: label.to_string(),
                op,
                worker_tx: worker_tx.clone(),
            })
            .map_err(|_| anyhow!("Clipboard worker thread is not running"))?;
        Ok(pending)
    }

    #[cfg(test)]
    pub(crate) fn from_fn(copy_text_fn: fn(&str) -> Result<()>) -> Self {
        let (tx, rx) = mpsc::channel::<CopyRequest>();

        std::thread::Builder::new()
            .name("clipboard-test".into())
            .spawn(move || {
                while let Ok(req) = rx.recv() {
                    let result = (copy_text_fn)(&req.text).map_err(|e| e.to_string());
                    let status = req.op.resolve(&result);
                    let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
                        label: req.label,
                        result,
                        status,
                    });
                }
            })
            .expect("failed to spawn test clipboard thread");

        Self { tx }
    }
}

fn clipboard_worker(rx: mpsc::Receiver<CopyRequest>) {
    let mut board = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            // If we can't initialize arboard at all, still drain requests
            // so senders don't block, and report the error for each.
            let msg = format!("Failed to access clipboard: {e}");
            for req in rx {
                let result = Err(msg.clone());
                let status = req.op.resolve(&result);
                let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
                    label: req.label,
                    result,
                    status,
                });
            }
            return;
        }
    };

    while let Ok(req) = rx.recv() {
        let result = board
            .set_text(&req.text)
            .map_err(|e| format!("Failed to copy to clipboard: {e}"));
        let status = req.op.resolve(&result);
        let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
            label: req.label,
            result,
            status,
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
            WorkerEvent::ClipboardCopyCompleted {
                label,
                result,
                status,
            } => {
                assert_eq!(label, "Copied.");
                assert!(result.is_ok());
                assert_eq!(status.outcome, Final::info("Copied."));
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
            WorkerEvent::ClipboardCopyCompleted {
                label,
                result,
                status,
            } => {
                assert_eq!(label, "Copied.");
                assert!(result.unwrap_err().contains("test error"));
                assert!(matches!(
                    status.outcome,
                    Final::Message {
                        tone: dux_core::statusline::StatusTone::Error,
                        ..
                    }
                ));
            }
            _ => panic!("unexpected event"),
        }
    }
}
