use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

use crate::logger;

/// A PTY-based client that spawns a CLI tool in a pseudo-terminal
/// and provides access to its parsed terminal screen via `vt100`.
pub struct PtyClient {
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    parser: Arc<Mutex<vt100::Parser>>,
    child: Box<dyn Child + Send + Sync>,
    exited: Arc<AtomicBool>,
    has_output: Arc<AtomicBool>,
}

impl PtyClient {
    /// Spawn a CLI command in a new PTY with the given size.
    pub fn spawn(command: &str, args: &[String], cwd: &Path, rows: u16, cols: u16) -> Result<Self> {
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
        cmd.env("TERM", "xterm-256color");

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
        let writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10_000)));
        let writer = Arc::new(Mutex::new(writer));
        let exited = Arc::new(AtomicBool::new(false));
        let has_output = Arc::new(AtomicBool::new(false));

        // Background reader thread.
        let parser_ref = Arc::clone(&parser);
        let writer_ref = Arc::clone(&writer);
        let exited_ref = Arc::clone(&exited);
        let has_output_ref = Arc::clone(&has_output);
        thread::spawn(move || {
            Self::reader_loop(reader, parser_ref, writer_ref, exited_ref, has_output_ref);
        });

        Ok(Self {
            master: pair.master,
            writer,
            parser,
            child,
            exited,
            has_output,
        })
    }

    fn reader_loop(
        mut reader: Box<dyn std::io::Read + Send>,
        parser: Arc<Mutex<vt100::Parser>>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        exited: Arc<AtomicBool>,
        has_output: Arc<AtomicBool>,
    ) {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    exited.store(true, Ordering::Release);
                    break;
                }
                Ok(n) => {
                    let data = &buf[..n];

                    // Respond to terminal capability queries from the child.
                    // The vt100 crate is a parser only — it doesn't write
                    // responses back, so CLI tools that probe the terminal
                    // (DA1, DA2, DSR) would get no reply and fall back to
                    // minimal colour output.
                    Self::respond_to_queries(data, &writer);

                    if let Ok(mut p) = parser.lock() {
                        p.process(data);
                        // Only mark output once the screen contains visible
                        // (non-whitespace) characters, so ANSI setup sequences
                        // don't prematurely hide the loading indicator.
                        if !has_output.load(Ordering::Acquire) {
                            let contents = p.screen().contents();
                            if contents.bytes().any(|b| !b.is_ascii_whitespace()) {
                                has_output.store(true, Ordering::Release);
                            }
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

    /// Scan the output stream for terminal queries and write back appropriate
    /// responses. This makes the PTY behave like a real terminal emulator
    /// rather than a black hole for capability probes.
    fn respond_to_queries(data: &[u8], writer: &Arc<Mutex<Box<dyn Write + Send>>>) {
        let mut i = 0;
        while i + 2 < data.len() {
            if data[i] != 0x1b || data[i + 1] != b'[' {
                i += 1;
                continue;
            }
            let rest = &data[i + 2..];
            match rest {
                // DA1: CSI c  or  CSI 0 c
                // Reply as VT220 with ANSI colour.
                [b'c', ..] | [b'0', b'c', ..] => {
                    if let Ok(mut w) = writer.lock() {
                        let _ = w.write_all(b"\x1b[?62;22c");
                        let _ = w.flush();
                    }
                }
                // DA2: CSI > c  or  CSI > 0 c
                // Reply as VT220, firmware version 0.
                [b'>', b'c', ..] | [b'>', b'0', b'c', ..] => {
                    if let Ok(mut w) = writer.lock() {
                        let _ = w.write_all(b"\x1b[>1;0;0c");
                        let _ = w.flush();
                    }
                }
                // DSR: CSI 5 n  (device status report — "are you OK?")
                // Reply: CSI 0 n  ("terminal OK").
                [b'5', b'n', ..] => {
                    if let Ok(mut w) = writer.lock() {
                        let _ = w.write_all(b"\x1b[0n");
                        let _ = w.flush();
                    }
                }
                // DSR-CPR: CSI 6 n  (cursor position report)
                // Reply with current cursor position from the parser.
                [b'6', b'n', ..] => {
                    // We don't hold the parser lock here, so report 1;1.
                    // This is close enough for capability probing.
                    if let Ok(mut w) = writer.lock() {
                        let _ = w.write_all(b"\x1b[1;1R");
                        let _ = w.flush();
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Write raw bytes to the PTY (forwards keystrokes to the child process).
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        writer.write_all(bytes).context("failed to write to PTY")?;
        writer.flush().context("failed to flush PTY writer")?;
        Ok(())
    }

    /// Get a snapshot of the terminal screen together with its scrollback
    /// offset in a single lock acquisition, avoiding race conditions with the
    /// background reader thread.
    pub fn screen_and_scrollback(&self) -> (vt100::Screen, usize) {
        let p = self.parser.lock().expect("parser mutex poisoned");
        let screen = p.screen().clone();
        let offset = screen.scrollback();
        (screen, offset)
    }

    /// Atomically adjust the scrollback offset by the given amount in the
    /// given direction, returning the new offset. Uses a single lock to avoid
    /// races between reading the current offset and writing the new one.
    pub fn scroll(&self, up: bool, amount: usize) {
        if let Ok(mut p) = self.parser.lock() {
            let current = p.screen().scrollback();
            let new_offset = if up {
                current.saturating_add(amount)
            } else {
                current.saturating_sub(amount)
            };
            p.set_scrollback(new_offset);
        }
    }

    /// Set the scrollback offset (0 = normal view, positive = scrolled back).
    pub fn set_scrollback(&self, rows: usize) {
        if let Ok(mut p) = self.parser.lock() {
            p.set_scrollback(rows);
        }
    }

    /// Resize the PTY and the internal terminal parser.
    ///
    /// The parser is resized by creating a fresh parser at the new dimensions
    /// and replaying the current screen contents, which avoids clearing the
    /// display (as `set_size` would) while keeping cursor maths correct.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize PTY")?;
        if let Ok(mut p) = self.parser.lock() {
            let contents = p.screen().contents_formatted();
            let mut fresh = vt100::Parser::new(rows, cols, 10_000);
            fresh.process(&contents);
            *p = fresh;
        }
        Ok(())
    }

    /// Check whether the child process has exited (reader thread detected EOF).
    pub fn is_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    /// Check whether the PTY has received any output from the child process.
    pub fn has_output(&self) -> bool {
        self.has_output.load(Ordering::Acquire)
    }

    /// Non-blocking check of the child's exit status.
    pub fn try_wait(&mut self) -> Option<portable_pty::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

impl Drop for PtyClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
