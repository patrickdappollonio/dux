use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[derive(Clone, Debug)]
pub enum TerminalKind {
    Shell,
}

#[derive(Clone, Debug)]
pub struct TerminalOutput;

pub struct TerminalSession {
    writer: Box<dyn Write + Send>,
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl TerminalSession {
    pub fn spawn(
        _kind: TerminalKind,
        cwd: &Path,
        command: &str,
        args: &[String],
        tx: Sender<TerminalOutput>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new(command);
        cmd.cwd(cwd);
        for arg in args {
            cmd.arg(arg);
        }
        let _child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn terminal command {command}"))?;
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let lines = Arc::new(Mutex::new(VecDeque::with_capacity(800)));
        let lines_clone = Arc::clone(&lines);
        thread::spawn(move || {
            let mut buf = [0_u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]).replace('\r', "");
                        {
                            let mut lines = lines_clone.lock().expect("terminal buffer lock");
                            for line in chunk.split('\n') {
                                if !line.is_empty() {
                                    if lines.len() >= 800 {
                                        lines.pop_front();
                                    }
                                    lines.push_back(line.to_string());
                                }
                            }
                        }
                        let _ = tx.send(TerminalOutput);
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self { writer, lines })
    }

    pub fn send(&mut self, input: &str) -> Result<()> {
        self.writer.write_all(input.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.lines
            .lock()
            .expect("terminal buffer lock")
            .iter()
            .cloned()
            .collect()
    }
}
