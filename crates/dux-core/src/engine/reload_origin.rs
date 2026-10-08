//! Who asked for a config reload, which decides whether it is announced.
//!
//! dux has no config-file watcher, so a reload has one of two origins:
//! somebody asked for it (SIGUSR1, the palette, the web's Reload config,
//! `dux config reload`), or dux asked for it after writing `config.toml`
//! itself (a password, a ban, the no-password warning's dismissal). Only the
//! first is news.

/// The origin of the reload running now, of the one deferred behind it, and
/// of the reload that last finished.
#[derive(Debug, Default)]
pub struct ReloadOrigin {
    /// Set while dux asks for the reload a write of its own owes.
    asking_after_own_write: bool,
    /// Whether somebody asked for the reload running now, and for the one
    /// deferred behind it. Asked for by anybody, a reload announces, however
    /// many reloads of dux's own it was folded into.
    running_asked_for: bool,
    deferred_asked_for: bool,
    last_was_own: bool,
}

impl ReloadOrigin {
    /// Whether the reload that last finished was asked for by dux alone,
    /// after a write of its own, and so is not announced.
    pub fn last_reload_was_own(&self) -> bool {
        self.last_was_own
    }

    pub(crate) fn begin_asking_after_own_write(&mut self) {
        self.asking_after_own_write = true;
    }

    pub(crate) fn end_asking_after_own_write(&mut self) {
        self.asking_after_own_write = false;
    }

    /// A reload is being asked for, to run now or (`deferred`) after the one
    /// running.
    pub(crate) fn reload_asked(&mut self, deferred: bool) {
        let asked_for = !self.asking_after_own_write;
        if deferred {
            self.deferred_asked_for |= asked_for;
        } else {
            self.running_asked_for = asked_for;
        }
    }

    /// The deferred reload is about to be asked for again, as whoever asked
    /// for it first; [`Self::end_asking_after_own_write`] follows.
    pub(crate) fn deferred_reload_resumes(&mut self) {
        self.asking_after_own_write = !std::mem::take(&mut self.deferred_asked_for);
    }

    /// The running reload finished. Its origin alone decides: a hand edit
    /// made but never reloaded is adopted by dux's own quiet reload without
    /// being announced. It is still applied; that cost is accepted.
    pub(crate) fn reload_finished(&mut self) {
        self.last_was_own = !std::mem::take(&mut self.running_asked_for);
    }
}
