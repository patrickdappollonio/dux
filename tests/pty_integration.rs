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

/// Verify that PTY output can be read and parsed by vt100.
#[test]
fn pty_read_output_into_vt100() {
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
    use std::io::Read;

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
    let mut parser = vt100::Parser::new(24, 80, 0);

    // Read output in a loop until EOF.
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => parser.process(&buf[..n]),
            Err(_) => break,
        }
    }

    child.wait().expect("failed to wait");

    // The screen should contain "hello-from-pty".
    let screen = parser.screen();
    let mut found = false;
    for row in 0..24 {
        for col in 0..80 {
            if let Some(cell) = screen.cell(row, col) {
                let contents = cell.contents();
                if contents == "h" {
                    // Check if "hello-from-pty" starts here.
                    let mut text = String::new();
                    for c in col..80 {
                        if let Some(cell) = screen.cell(row, c) {
                            let ch = cell.contents();
                            if ch.is_empty() || ch == " " {
                                break;
                            }
                            text.push_str(&ch);
                        }
                    }
                    if text == "hello-from-pty" {
                        found = true;
                    }
                }
            }
        }
    }
    assert!(found, "Expected 'hello-from-pty' in vt100 screen output");
}

/// Verify that writing to the PTY sends input to the child.
#[test]
fn pty_write_input() {
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
    use std::io::{Read, Write};

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

    // Give it a moment to process.
    thread::sleep(Duration::from_millis(200));

    // Read whatever is available.
    let mut output = vec![0u8; 4096];
    // Non-blocking: try to read
    let _ = child.kill();
    let n = reader.read(&mut output).unwrap_or(0);
    let text = String::from_utf8_lossy(&output[..n]);

    // The output should contain our input echoed back.
    assert!(
        text.contains("test-input"),
        "Expected 'test-input' in output, got: {text}"
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
