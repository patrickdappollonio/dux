//! A file descriptor that becomes readable when the window is resized, so a
//! wait for the terminal's input can wait on it too and end at once.

use std::io::Read as _;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;

#[derive(Debug)]
pub(crate) struct ResizeWake {
    read: UnixStream,
    sig_id: signal_hook::SigId,
}

impl ResizeWake {
    /// Have every SIGWINCH write a byte this wake reads.
    pub(crate) fn register() -> std::io::Result<Self> {
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        let sig_id = signal_hook::low_level::pipe::register(signal_hook::consts::SIGWINCH, write)?;
        Ok(Self { read, sig_id })
    }

    /// Read every byte resizes have written, so the next wait sleeps until the
    /// next resize.
    pub(crate) fn clear(&self) {
        let mut buf = [0u8; 64];
        while matches!((&self.read).read(&mut buf), Ok(n) if n > 0) {}
    }

    pub(crate) fn unregister(self) {
        signal_hook::low_level::unregister(self.sig_id);
    }
}

impl AsFd for ResizeWake {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.read.as_fd()
    }
}
