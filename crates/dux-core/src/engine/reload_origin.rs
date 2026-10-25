//! Who asked for a config reload, which decides whether it is announced.
//!
//! dux has no config-file watcher, so a reload has one of two origins:
//! somebody asked for it (SIGUSR1, the palette, the web's Reload config,
//! `dux config reload`), or dux asked for it after writing `config.toml`
//! itself (a password, a ban, the no-password warning's dismissal). Only the
//! first is news.

/// Who a reload is for: somebody who asked for it, or dux alone, after a
/// write of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asker {
    Somebody,
    DuxAfterOwnWrite,
}

/// Which reload something waits on: the one running now, or the follow-up
/// queued behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadSlot {
    /// The reload running now, or, when none runs, the one that starts now.
    Now,
    /// The follow-up queued behind the reload that is running.
    AfterTheRunningOne,
}

/// The origin of the reload running now, of the one deferred behind it, and
/// of the reload that last finished.
#[derive(Debug)]
pub struct ReloadOrigin {
    /// Who is asking for a reload at this moment: dux, while it asks for the
    /// reload a write of its own owes, and otherwise somebody.
    asking: Asker,
    /// Who the reload running now, and the one deferred behind it, are for.
    /// Asked for by anybody, a reload announces, however many reloads of
    /// dux's own it was folded into.
    running: Asker,
    deferred: Asker,
    last: Asker,
}

impl Default for ReloadOrigin {
    fn default() -> Self {
        Self {
            asking: Asker::Somebody,
            running: Asker::DuxAfterOwnWrite,
            deferred: Asker::DuxAfterOwnWrite,
            last: Asker::Somebody,
        }
    }
}

impl ReloadOrigin {
    /// Whether the reload that last finished was asked for by dux alone,
    /// after a write of its own, and so is not announced.
    pub fn last_reload_was_own(&self) -> bool {
        self.last == Asker::DuxAfterOwnWrite
    }

    pub(crate) fn begin_asking_after_own_write(&mut self) {
        self.asking = Asker::DuxAfterOwnWrite;
    }

    pub(crate) fn end_asking_after_own_write(&mut self) {
        self.asking = Asker::Somebody;
    }

    /// A reload is being asked for, to run in `slot`.
    pub(crate) fn reload_asked(&mut self, slot: ReloadSlot) {
        match slot {
            ReloadSlot::AfterTheRunningOne => {
                if self.asking == Asker::Somebody {
                    self.deferred = Asker::Somebody;
                }
            }
            ReloadSlot::Now => self.running = self.asking,
        }
    }

    /// The deferred reload is about to be asked for again, as whoever asked
    /// for it first; [`Self::end_asking_after_own_write`] follows.
    pub(crate) fn deferred_reload_resumes(&mut self) {
        self.asking = std::mem::replace(&mut self.deferred, Asker::DuxAfterOwnWrite);
    }

    /// The running reload finished. Its origin alone decides: a hand edit
    /// made but never reloaded is adopted by dux's own quiet reload without
    /// being announced. It is still applied; that cost is accepted.
    pub(crate) fn reload_finished(&mut self) {
        self.last = std::mem::replace(&mut self.running, Asker::DuxAfterOwnWrite);
    }
}

#[cfg(test)]
mod tests {
    use super::{ReloadOrigin, ReloadSlot};

    /// dux's own reload, with a reload somebody asked for deferred behind it:
    /// the own one is quiet and the deferred one announces.
    #[test]
    fn a_reload_asked_for_behind_an_own_one_announces_and_the_own_one_does_not() {
        let mut origin = ReloadOrigin::default();
        assert!(!origin.last_reload_was_own());
        origin.begin_asking_after_own_write();
        origin.reload_asked(ReloadSlot::Now);
        origin.end_asking_after_own_write();
        origin.reload_asked(ReloadSlot::AfterTheRunningOne);
        origin.reload_finished();
        assert!(origin.last_reload_was_own());
        origin.deferred_reload_resumes();
        origin.reload_asked(ReloadSlot::Now);
        origin.end_asking_after_own_write();
        origin.reload_finished();
        assert!(!origin.last_reload_was_own());
    }

    /// A reload somebody asked for, with dux's own deferred behind it: the
    /// first announces and the deferred own one is quiet.
    #[test]
    fn an_own_reload_deferred_behind_an_asked_one_stays_quiet() {
        let mut origin = ReloadOrigin::default();
        origin.reload_asked(ReloadSlot::Now);
        origin.begin_asking_after_own_write();
        origin.reload_asked(ReloadSlot::AfterTheRunningOne);
        origin.end_asking_after_own_write();
        origin.reload_finished();
        assert!(!origin.last_reload_was_own());
        origin.deferred_reload_resumes();
        origin.reload_asked(ReloadSlot::Now);
        origin.end_asking_after_own_write();
        origin.reload_finished();
        assert!(origin.last_reload_was_own());
    }

    /// Deferred asks fold together: one by somebody among dux's own makes
    /// the follow-up announce.
    #[test]
    fn one_asked_for_reload_among_deferred_own_ones_makes_the_follow_up_announce() {
        let mut origin = ReloadOrigin::default();
        origin.reload_asked(ReloadSlot::Now);
        origin.reload_asked(ReloadSlot::AfterTheRunningOne);
        origin.begin_asking_after_own_write();
        origin.reload_asked(ReloadSlot::AfterTheRunningOne);
        origin.end_asking_after_own_write();
        origin.reload_finished();
        origin.deferred_reload_resumes();
        origin.reload_asked(ReloadSlot::Now);
        origin.end_asking_after_own_write();
        origin.reload_finished();
        assert!(!origin.last_reload_was_own());
    }
}
