//! `dux config set` signals whoever holds dux.lock with SIGUSR1. A dux from
//! before this branch (7437238c) holds the same exclusive flock on the same
//! file but installs no SIGUSR1 handler, so the signal's default action
//! terminates it. This stands in for that older dux with a process that holds
//! the lock the same way and leaves SIGUSR1 at its default.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn signalling_a_lock_holder_without_a_reload_handler_kills_it() {
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
        assert_eq!(line.trim(), "locked");
    }
    let outcome = dux_core::reload_signal::signal_running_dux(&lock);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut status = None;
    while Instant::now() < deadline {
        if let Some(s) = child.try_wait().unwrap() {
            status = Some(s);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Always reaped, whether it exited (already waited by `try_wait`) or not.
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        status.is_none(),
        "the lock holder (an older dux with no SIGUSR1 handler) was terminated by \
         `dux config set`'s reload signal: outcome {outcome:?}, exit {status:?}"
    );
}
