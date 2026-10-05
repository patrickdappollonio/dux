//! A lock holder whose lock file says it handles the reload signal, but
//! whose process does not catch SIGUSR1 (a handler that is gone, or a
//! marker written by mistake), is never signalled where the system can say
//! so: the marker and the process's own signal mask must agree.

#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dux_core::reload_signal::{NoReloadHandler, SignalOutcome, signal_running_dux};

#[test]
fn a_holder_that_advertises_the_marker_but_catches_no_sigusr1_is_not_signalled() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("dux.lock");
    std::fs::write(&lock, "").unwrap();
    // Holds the lock the way dux does and writes the marker, but leaves
    // SIGUSR1 at its default action.
    let script = format!(
        "import fcntl,os,sys,time\nf=open({:?},'r+')\nfcntl.flock(f,fcntl.LOCK_EX)\n\
         f.seek(0);f.truncate();f.write(str(os.getpid())+'\\n{}\\n');f.flush()\n\
         sys.stdout.write('locked\\n');sys.stdout.flush()\ntime.sleep(30)\n",
        lock.display().to_string(),
        dux_core::lockfile::RELOAD_SIGNAL_MARKER,
    );
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3");
    {
        use std::io::BufRead;
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "locked");
    }
    let outcome = signal_running_dux(&lock);
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut status = None;
    while Instant::now() < deadline {
        if let Some(s) = child.try_wait().unwrap() {
            status = Some(s);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(status.is_none(), "the holder was killed: {outcome:?}");
    assert_eq!(
        outcome,
        SignalOutcome::NotSignalled {
            pid: child.id(),
            why: NoReloadHandler::NotCaught,
        }
    );
}

/// The same holder without the marker is an older dux: not signalled, and
/// said to be one.
#[test]
fn a_holder_without_the_marker_is_named_an_older_dux() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("dux.lock");
    std::fs::write(&lock, "").unwrap();
    let script = format!(
        "import fcntl,os,sys,time\nf=open({:?},'r+')\nfcntl.flock(f,fcntl.LOCK_EX)\n\
         f.seek(0);f.truncate();f.write(str(os.getpid()));f.flush()\n\
         sys.stdout.write('locked\\n');sys.stdout.flush()\ntime.sleep(30)\n",
        lock.display().to_string()
    );
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3");
    {
        use std::io::BufRead;
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
    }
    let outcome = signal_running_dux(&lock);
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    assert!(alive);
    assert_eq!(
        outcome,
        SignalOutcome::NotSignalled {
            pid: child.id(),
            why: NoReloadHandler::OlderVersion,
        }
    );
}
