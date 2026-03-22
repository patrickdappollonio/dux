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

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 200)));
        let exited = Arc::new(AtomicBool::new(false));

        // Background reader thread.
        let parser_ref = Arc::clone(&parser);
        let exited_ref = Arc::clone(&exited);
        thread::spawn(move || {
            Self::reader_loop(reader, parser_ref, exited_ref);
        });

        Ok(Self {
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            parser,
            child,
            exited,
        })
    }

    fn reader_loop(
        mut reader: Box<dyn std::io::Read + Send>,
        parser: Arc<Mutex<vt100::Parser>>,
        exited: Arc<AtomicBool>,
    ) {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    exited.store(true, Ordering::Release);
                    break;
                }
                Ok(n) => {
                    if let Ok(mut p) = parser.lock() {
                        p.process(&buf[..n]);
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
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        writer.write_all(bytes).context("failed to write to PTY")?;
        writer.flush().context("failed to flush PTY writer")?;
        Ok(())
    }

    /// Get a snapshot of the current terminal screen.
    pub fn screen(&self) -> vt100::Screen {
        self.parser
            .lock()
            .expect("parser mutex poisoned")
            .screen()
            .clone()
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
            let mut fresh = vt100::Parser::new(rows, cols, 200);
            fresh.process(&contents);
            *p = fresh;
        }
        Ok(())
    }

    /// Check whether the child process has exited (reader thread detected EOF).
    pub fn is_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
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
