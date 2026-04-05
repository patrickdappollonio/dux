use std::io::{self, ErrorKind};

pub(crate) fn retry_on_interrupt<T, F>(mut op: F) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    loop {
        match op() {
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

pub(crate) fn retry_on_interrupt_errno<T, F>(mut op: F) -> rustix::io::Result<T>
where
    F: FnMut() -> rustix::io::Result<T>,
{
    loop {
        match op() {
            Err(rustix::io::Errno::INTR) => continue,
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn retries_std_io_interrupts_until_success() {
        let calls = Cell::new(0);

        let result = retry_on_interrupt(|| {
            let call = calls.get();
            calls.set(call + 1);
            if call == 0 {
                Err(io::Error::new(ErrorKind::Interrupted, "signal"))
            } else {
                Ok(7)
            }
        })
        .expect("retry should succeed");

        assert_eq!(result, 7);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn retries_rustix_interrupts_until_success() {
        let calls = Cell::new(0);

        let result = retry_on_interrupt_errno(|| {
            let call = calls.get();
            calls.set(call + 1);
            if call == 0 {
                Err(rustix::io::Errno::INTR)
            } else {
                Ok(5usize)
            }
        })
        .expect("retry should succeed");

        assert_eq!(result, 5);
        assert_eq!(calls.get(), 2);
    }
}
