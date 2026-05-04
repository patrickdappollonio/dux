use std::thread;
use std::time::Duration;

/// Smoke test: verify that spawning a simple command via PTY works
/// by checking that the process exits cleanly.
#[test]
fn pty_spawn_and_detect_exit() {
    // We cannot import PtyClient directly (private module), so we test
    // the underlying portable-pty crate to ensure it works on this platform.
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open PTY");

    let mut cmd = CommandBuilder::new("echo");
    cmd.arg("hello-from-pty");

    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    drop(pair.slave);

    // Wait for exit.
    let status = child.wait().expect("failed to wait");
    assert!(status.success());
}

/// Verify that PTY output can be read and parsed by alacritty_terminal.
#[test]
fn pty_read_output_into_alacritty_terminal() {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::term::{self, Config, Term};
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
    use std::io::Read;

    struct TerminalDimensions {
        rows: usize,
        cols: usize,
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

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open PTY");

    let mut cmd = CommandBuilder::new("echo");
    cmd.arg("hello-from-pty");

    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("failed to clone reader");
    let mut parser: Processor<StdSyncHandler> = Processor::new();
    let dimensions = TerminalDimensions { rows: 24, cols: 80 };
    let mut term = Term::new(Config::default(), &dimensions, VoidListener);

    // Read output in a loop until EOF.
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => parser.advance(&mut term, &buf[..n]),
            Err(_) => break,
        }
    }

    child.wait().expect("failed to wait");

    let renderable = term.renderable_content();
    let mut viewport = vec![String::new(); 24];
    for indexed in renderable.display_iter {
        let Some(point) = term::point_to_viewport(renderable.display_offset, indexed.point) else {
            continue;
        };
        let row = &mut viewport[point.line];
        while row.len() < indexed.point.column.0 {
            row.push(' ');
        }
        row.push(indexed.cell.c);
        if let Some(zerowidth) = indexed.cell.zerowidth() {
            for ch in zerowidth {
                row.push(*ch);
            }
        }
    }
    assert!(
        viewport.iter().any(|line| line.contains("hello-from-pty")),
        "Expected 'hello-from-pty' in terminal output"
    );
}

/// Verify that writing to the PTY sends input to the child.
#[test]
fn pty_write_input() {
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::Instant;

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open PTY");

    // Use `cat` which echoes stdin to stdout.
    let cmd = CommandBuilder::new("cat");
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");

    // Write some text followed by EOF (Ctrl-D).
    writer.write_all(b"test-input\n").expect("write");
    writer.write_all(b"\x04").expect("write eof");
    let _ = writer.flush();

    // The PTY reader is a blocking `Box<dyn Read + Send>`. To avoid racing
    // the kill against the child's echo on slow runners (notably macOS-14
    // GitHub runners, where the original write→sleep(200ms)→kill→read flow
    // could land the kill before `cat` had echoed bytes back through the
    // master), we drain on a background thread and poll the channel until
    // either we observe "test-input" in the accumulated output or a 5s
    // deadline elapses. The kill is issued *after* we've seen the bytes
    // (or timed out), so we never starve the read.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });

    let needle = b"test-input";
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output: Vec<u8> = Vec::new();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let chunk_timeout = std::cmp::min(remaining, Duration::from_millis(100));
        match rx.recv_timeout(chunk_timeout) {
            Ok(chunk) => {
                output.extend_from_slice(&chunk);
                if output.windows(needle.len()).any(|w| w == needle) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let text = String::from_utf8_lossy(&output);

    // The output should contain our input echoed back.
    assert!(
        text.contains("test-input"),
        "Expected 'test-input' in output (after 5s polling), got: {text}"
    );
}

/// Verify PTY resize doesn't panic.
#[test]
fn pty_resize() {
    use portable_pty::{NativePtySystem, PtySize, PtySystem};

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open PTY");

    // Resize should not panic.
    pair.master
        .resize(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("resize should succeed");
}
