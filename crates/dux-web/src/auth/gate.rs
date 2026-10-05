//! The bound on password checks. Each Argon2id run takes about 19 MiB and a few
//! dozen milliseconds of CPU on purpose, so a flood of logins must not become a
//! flood of them:
//!
//! - `max_concurrent_password_checks` run at once, across every client.
//! - `password_check_queue` more may wait for one to finish.
//! - Anything beyond that is answered "too many requests" at once, without
//!   waiting and without checking anything.
//!
//! Both numbers are read from the live config on every entry, so a reload
//! applies to the next attempt. A waiter that gives up (its request dropped)
//! leaves the queue on the way out, so an abandoned request never holds a
//! queue place. A RUNNING check is different (decided, after review): its
//! permit is owned and moved into the blocking work, so the slot is released
//! only when Argon2 actually finishes, never when the request that asked for it
//! goes away. Otherwise a client could start a check, hang up, and start
//! another, running far more Argon2 work at once than the limit allows.

use tokio::sync::Notify;

#[derive(Default)]
struct Counts {
    running: u32,
    waiting: u32,
}

/// The gate a password check passes through.
#[derive(Default)]
pub(crate) struct CheckGate {
    counts: std::sync::Mutex<Counts>,
    freed: Notify,
}

/// A running check's slot, released when dropped. It owns its gate so it can
/// travel into the blocking work.
pub(crate) struct CheckPermit {
    gate: std::sync::Arc<CheckGate>,
}

/// The gate is full: every check slot is taken and the queue is too.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct GateFull;

impl CheckGate {
    fn lock(&self) -> std::sync::MutexGuard<'_, Counts> {
        self.counts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Take a check slot, waiting in the queue when they are all busy, or
    /// answer [`GateFull`] at once when the queue is full too.
    pub(crate) async fn enter(
        self: &std::sync::Arc<Self>,
        running: u32,
        queue: u32,
    ) -> Result<CheckPermit, GateFull> {
        let running = running.max(1);
        {
            let mut counts = self.lock();
            if counts.running < running {
                counts.running += 1;
                return Ok(CheckPermit {
                    gate: std::sync::Arc::clone(self),
                });
            }
            if counts.waiting >= queue {
                return Err(GateFull);
            }
            counts.waiting += 1;
        }
        let waiting = WaitingSlot { gate: self };
        loop {
            let freed = self.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            {
                let mut counts = self.lock();
                if counts.running < running {
                    counts.running += 1;
                    counts.waiting -= 1;
                    std::mem::forget(waiting);
                    return Ok(CheckPermit {
                        gate: std::sync::Arc::clone(self),
                    });
                }
            }
            freed.await;
        }
    }

    /// How many checks run and wait right now.
    #[cfg(test)]
    pub(crate) fn load(&self) -> (u32, u32) {
        let counts = self.lock();
        (counts.running, counts.waiting)
    }
}

/// A queued waiter's place, given back if the waiter is dropped while waiting.
struct WaitingSlot<'a> {
    gate: &'a CheckGate,
}

impl Drop for WaitingSlot<'_> {
    fn drop(&mut self) {
        self.gate.lock().waiting -= 1;
    }
}

impl Drop for CheckPermit {
    fn drop(&mut self) {
        self.gate.lock().running -= 1;
        self.gate.freed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn checks_run_up_to_the_limit_then_queue_then_are_refused_at_once() {
        let gate = std::sync::Arc::new(CheckGate::default());
        let a = gate.enter(2, 1).await.expect("first");
        let b = gate.enter(2, 1).await.expect("second");
        assert_eq!(gate.load(), (2, 0));
        // The third waits in the queue; the fourth is refused without waiting.
        let waiter = {
            let gate = std::sync::Arc::clone(&gate);
            tokio::spawn(async move {
                let _permit = gate.enter(2, 1).await.expect("queued, then let in");
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(gate.load(), (2, 1));
        assert_eq!(gate.enter(2, 1).await.err(), Some(GateFull));
        drop(a);
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("the waiter got the freed slot")
            .unwrap();
        drop(b);
        assert_eq!(gate.load(), (0, 0));
    }

    #[tokio::test]
    async fn an_abandoned_waiter_gives_its_place_back() {
        let gate = std::sync::Arc::new(CheckGate::default());
        let held = gate.enter(1, 1).await.expect("slot");
        let abandoned = {
            let gate = std::sync::Arc::clone(&gate);
            tokio::spawn(async move {
                let _ = gate.enter(1, 1).await;
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(gate.load(), (1, 1));
        abandoned.abort();
        let _ = abandoned.await;
        assert_eq!(gate.load(), (1, 0), "the place is free again");
        drop(held);
        assert_eq!(gate.load(), (0, 0));
    }

    #[tokio::test]
    async fn a_queue_of_zero_refuses_as_soon_as_every_slot_is_busy() {
        let gate = std::sync::Arc::new(CheckGate::default());
        let _held = gate.enter(1, 0).await.expect("slot");
        assert_eq!(gate.enter(1, 0).await.err(), Some(GateFull));
    }
}
