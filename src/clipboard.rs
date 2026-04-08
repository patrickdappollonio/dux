use std::sync::mpsc;

use anyhow::{Result, anyhow};

use crate::app::WorkerEvent;

/// Request sent from the main thread to the clipboard worker.
struct CopyRequest {
    text: String,
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
    pub(crate) fn copy_text(
        &self,
        text: &str,
        worker_tx: &mpsc::Sender<WorkerEvent>,
    ) -> Result<()> {
        self.tx
            .send(CopyRequest {
                text: text.to_string(),
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
                        path: req.text,
                        result,
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
                let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
                    path: req.text,
                    result: Err(msg.clone()),
                });
            }
            return;
        }
    };

    while let Ok(req) = rx.recv() {
        let result = board
            .set_text(&req.text)
            .map_err(|e| format!("Failed to copy to clipboard: {e}"));
        let _ = req.worker_tx.send(WorkerEvent::ClipboardCopyCompleted {
            path: req.text,
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
        clipboard.copy_text("hello", &worker_tx).unwrap();

        let event = worker_rx.recv().unwrap();
        match event {
            WorkerEvent::ClipboardCopyCompleted { path, result } => {
                assert_eq!(path, "hello");
                assert!(result.is_ok());
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn clipboard_from_fn_reports_errors() {
        let (worker_tx, worker_rx) = mpsc::channel();
        let clipboard = Clipboard::from_fn(|_| Err(anyhow!("test error")));
        clipboard.copy_text("hello", &worker_tx).unwrap();

        let event = worker_rx.recv().unwrap();
        match event {
            WorkerEvent::ClipboardCopyCompleted { path, result } => {
                assert_eq!(path, "hello");
                assert!(result.unwrap_err().contains("test error"));
            }
            _ => panic!("unexpected event"),
        }
    }
}
