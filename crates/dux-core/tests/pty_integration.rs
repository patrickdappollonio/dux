use std::thread;
use std::time::Duration;

use dux_core::pty::PtyClient;

/// Smoke test: verify that spawning a simple command via PTY works
/// by checking that the process exits cleanly.
#[test]
fn pty_spawn_and_detect_exit() {
    // Exercise the underlying portable-pty crate directly to ensure raw PTY
    // spawn/exit works on this platform; the higher-level vt100-backed
    // `PtyClient` path is covered by the snapshot/round-trip tests below.
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

/// Read a viewport row into a single string from a terminal snapshot.
fn viewport_lines(snapshot: &dux_core::pty::TerminalSnapshot) -> Vec<String> {
    let mut rows = vec![String::new(); usize::from(snapshot.rows)];
    for cell in &snapshot.cells {
        if let Some(line) = rows.get_mut(usize::from(cell.row)) {
            while line.chars().count() < usize::from(cell.col) {
                line.push(' ');
            }
            line.push_str(&cell.symbol);
        }
    }
    rows
}

/// Verify that PTY output is parsed by the vt100-backed `PtyClient` terminal
/// and surfaces as visible cells in the snapshot. This is the end-to-end
/// equivalent of the previous alacritty-driven test: bytes from a spawned
/// command must show up in the rendered viewport.
#[test]
fn pty_output_renders_into_terminal_snapshot() {
    let args = vec!["hello-from-pty".to_string()];
    let mut client =
        PtyClient::spawn("echo", &args, std::path::Path::new("."), 24, 80, 1000).expect("spawn");

    // Poll the snapshot until the echoed text shows up (or time out).
    for _ in 0..200 {
        thread::sleep(Duration::from_millis(10));
        let snapshot = client.snapshot();
        if viewport_lines(&snapshot)
            .iter()
            .any(|line| line.contains("hello-from-pty"))
        {
            let _ = client.try_wait();
            return;
        }
    }

    let snapshot = client.snapshot();
    panic!(
        "expected 'hello-from-pty' in terminal output, got {:?}",
        viewport_lines(&snapshot)
    );
}

/// Verify that the cursor position reported by the snapshot tracks the child's
/// output: after a command that prints text without a trailing newline, the
/// cursor must sit past the printed glyphs on the same row.
#[test]
fn pty_cursor_tracks_output() {
    // `printf` writes "ab" with no newline, leaving the cursor at column 2.
    let args = vec!["ab".to_string()];
    let mut client =
        PtyClient::spawn("printf", &args, std::path::Path::new("."), 5, 40, 100).expect("spawn");

    for _ in 0..200 {
        thread::sleep(Duration::from_millis(10));
        let snapshot = client.snapshot();
        let has_text = viewport_lines(&snapshot)
            .iter()
            .any(|line| line.contains("ab"));
        if has_text && let Some(cursor) = snapshot.cursor {
            // The cursor should be on the first row at or past column 2.
            assert!(
                cursor.col >= 2,
                "cursor should advance past printed text, got {cursor:?}"
            );
            let _ = client.try_wait();
            return;
        }
    }

    let snapshot = client.snapshot();
    panic!(
        "expected printed text and a cursor, got {:?} cursor={:?}",
        viewport_lines(&snapshot),
        snapshot.cursor
    );
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

/// Verify the high-level client can forward keystrokes to a child and observe
/// the echoed result in the snapshot (vt100 path end-to-end).
#[test]
fn pty_client_round_trips_input_to_output() {
    let args = vec![
        "-c".to_string(),
        "printf READY; read line; printf 'GOT:%s' \"$line\"".to_string(),
    ];
    let mut client =
        PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 5, 40, 100).expect("spawn");

    let mut ready = false;
    for _ in 0..200 {
        thread::sleep(Duration::from_millis(10));
        if viewport_lines(&client.snapshot())
            .iter()
            .any(|line| line.contains("READY"))
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "shell did not reach `read` within 2s");
    client.write_bytes(b"hello\n").expect("write");

    for _ in 0..200 {
        thread::sleep(Duration::from_millis(10));
        if viewport_lines(&client.snapshot())
            .iter()
            .any(|line| line.contains("GOT:hello"))
        {
            let _ = client.try_wait();
            return;
        }
    }

    panic!(
        "expected echoed input in output, got {:?}",
        viewport_lines(&client.snapshot())
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
