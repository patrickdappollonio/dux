use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Info,
    Busy,
    Warning,
    Error,
}

impl StatusTone {
    /// The wire tone string shared with the web client (matches `WireStatus`).
    pub fn as_wire(self) -> &'static str {
        match self {
            StatusTone::Info => "info",
            StatusTone::Busy => "busy",
            StatusTone::Warning => "warning",
            StatusTone::Error => "error",
        }
    }

    /// Parse a wire tone string back to a tone; an unknown tone maps to `Info`
    /// (the neutral default), matching how the web client treats it.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "busy" => StatusTone::Busy,
            "warning" => StatusTone::Warning,
            "error" => StatusTone::Error,
            _ => StatusTone::Info,
        }
    }

    /// Whether a status of this tone auto-clears after the timeout. Only `Info`
    /// (which dux uses for success/positive confirmations) expires: `Busy` waits
    /// for the final state that replaces it, and `Warning`/`Error` persist until
    /// the next status so a problem is never missed.
    fn auto_clears(self) -> bool {
        matches!(self, StatusTone::Info)
    }
}

/// The shared status-line controller used by BOTH the TUI (in-process) and the
/// web (server-side, in the engine actor), so the two surfaces get identical
/// behaviour from one place:
///
/// - A `Busy`/"pending" status persists until a final state replaces it (the
///   `set_busy` → `set_info`/`set_error` convention). It never auto-clears, so a
///   pending that never resolves shows as a stuck spinner — a visible bug, not a
///   silent vanish.
/// - `Info`/success statuses auto-clear after `clear_after` so the line doesn't
///   keep stale confirmations forever.
/// - `Warning`/`Error` persist until the next status replaces them.
///
/// Time is passed into [`set`](Self::set)/[`tick`](Self::tick) so the expiry
/// logic is deterministic in tests. A `clear_after` of zero disables auto-clear
/// entirely (everything persists until replaced).
pub struct StatusLine {
    message: String,
    tone: StatusTone,
    since: Instant,
    clear_after: Duration,
    /// When true, the current message is exempt from auto-clear even if its tone
    /// would normally expire (e.g. the first-run help hint, which should stay
    /// until the user's first action replaces it). Any later `set` clears the pin.
    pinned: bool,
}

impl StatusLine {
    /// Construct with auto-clear DISABLED (`clear_after` = 0). The owning surface
    /// should call [`set_clear_after`](Self::set_clear_after) with the configured
    /// value, or use [`with_clear_after`](Self::with_clear_after).
    pub fn new(message: impl Into<String>) -> Self {
        Self::with_clear_after(message, Duration::ZERO)
    }

    pub fn with_clear_after(message: impl Into<String>, clear_after: Duration) -> Self {
        Self {
            message: message.into(),
            tone: StatusTone::Info,
            since: Instant::now(),
            clear_after,
            pinned: false,
        }
    }

    /// Set the auto-clear timeout. Zero disables it (statuses persist until
    /// replaced).
    pub fn set_clear_after(&mut self, clear_after: Duration) {
        self.clear_after = clear_after;
    }

    /// Exempt the CURRENT message from auto-clear (it persists until the next
    /// `set` replaces it). Used for the first-run help hint so it doesn't vanish
    /// after the timeout. A subsequent `set` clears the pin.
    pub fn pin(&mut self) {
        self.pinned = true;
    }

    /// Set the status with an explicit timestamp (so tests can control timing).
    /// The `info`/`busy`/`warning`/`error` helpers wrap this with `Instant::now`.
    pub fn set(&mut self, now: Instant, tone: StatusTone, message: impl Into<String>) {
        self.message = message.into();
        self.tone = tone;
        self.since = now;
        self.pinned = false;
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.set(Instant::now(), StatusTone::Info, message);
    }

    pub fn busy(&mut self, message: impl Into<String>) {
        self.set(Instant::now(), StatusTone::Busy, message);
    }

    pub fn warning(&mut self, message: impl Into<String>) {
        self.set(Instant::now(), StatusTone::Warning, message);
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.set(Instant::now(), StatusTone::Error, message);
    }

    /// Clear the status if it has expired. Returns `true` when the visible status
    /// changed (i.e. an `Info` status was cleared), so the caller can react (the
    /// TUI re-renders; the web broadcasts the cleared state). Only `Info` clears,
    /// and only once a non-zero `clear_after` has elapsed.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.clear_after.is_zero()
            || self.message.is_empty()
            || self.pinned
            || !self.tone.auto_clears()
        {
            return false;
        }
        if now.duration_since(self.since) >= self.clear_after {
            self.message.clear();
            return true;
        }
        false
    }

    pub fn text(&self) -> String {
        if self.message.is_empty() {
            return String::new();
        }
        match self.tone {
            StatusTone::Busy => format!("{} {}", self.spinner_frame(), self.message),
            StatusTone::Info | StatusTone::Warning | StatusTone::Error => self.message.clone(),
        }
    }

    pub fn tone(&self) -> StatusTone {
        self.tone
    }

    /// The raw message text, without tone-specific prefixes or spinners. Empty
    /// once the status has been cleared.
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn is_empty(&self) -> bool {
        self.message.is_empty()
    }

    fn spinner_frame(&self) -> &'static str {
        const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let index = ((self.since.elapsed().as_millis() / 100) as usize) % FRAMES.len();
        FRAMES[index]
    }
}

#[cfg(test)]
mod tests {
    use super::{StatusLine, StatusTone};
    use std::time::{Duration, Instant};

    #[test]
    fn warning_tone_keeps_message_plain() {
        let mut status = StatusLine::new("ready");
        status.warning("something changed");
        assert_eq!(status.tone(), StatusTone::Warning);
        // The warning colour carries the meaning — no "[warning]" prefix.
        assert_eq!(status.text(), "something changed");
    }

    #[test]
    fn info_auto_clears_after_the_timeout() {
        let t0 = Instant::now();
        let mut status = StatusLine::with_clear_after("", Duration::from_secs(6));
        status.set(t0, StatusTone::Info, "Saved.");

        // Not yet expired.
        assert!(!status.tick(t0 + Duration::from_secs(5)));
        assert_eq!(status.message(), "Saved.");

        // Expired → cleared, and `tick` reports the change exactly once.
        assert!(status.tick(t0 + Duration::from_secs(6)));
        assert!(status.is_empty());
        assert_eq!(status.text(), "", "cleared status renders nothing");
        assert!(!status.tick(t0 + Duration::from_secs(7)));
    }

    #[test]
    fn a_pinned_info_status_does_not_auto_clear() {
        let t0 = Instant::now();
        let mut status = StatusLine::with_clear_after("", Duration::from_secs(6));
        status.set(t0, StatusTone::Info, "Press ? for help");
        status.pin();
        // Pinned Info survives well past the timeout…
        assert!(!status.tick(t0 + Duration::from_secs(3600)));
        assert_eq!(status.message(), "Press ? for help");
        // …until a real status replaces it, which clears the pin and resumes
        // normal auto-clear.
        status.set(t0 + Duration::from_secs(3600), StatusTone::Info, "Saved.");
        assert!(status.tick(t0 + Duration::from_secs(3607)));
        assert!(status.is_empty());
    }

    #[test]
    fn busy_warning_error_never_auto_clear() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(3600);
        for tone in [StatusTone::Busy, StatusTone::Warning, StatusTone::Error] {
            let mut status = StatusLine::with_clear_after("", Duration::from_secs(6));
            status.set(t0, tone, "pending or problem");
            assert!(!status.tick(later), "{tone:?} must not auto-clear");
            assert_eq!(status.message(), "pending or problem");
        }
    }

    #[test]
    fn setting_a_new_status_resets_the_timer() {
        let t0 = Instant::now();
        let mut status = StatusLine::with_clear_after("", Duration::from_secs(6));
        status.set(t0, StatusTone::Info, "first");
        // Re-set just before the first would expire; the clock restarts.
        status.set(t0 + Duration::from_secs(5), StatusTone::Info, "second");
        assert!(!status.tick(t0 + Duration::from_secs(10)));
        assert_eq!(status.message(), "second");
        assert!(status.tick(t0 + Duration::from_secs(11)));
    }

    #[test]
    fn zero_timeout_disables_auto_clear() {
        let t0 = Instant::now();
        let mut status = StatusLine::with_clear_after("", Duration::ZERO);
        status.set(t0, StatusTone::Info, "stays");
        assert!(!status.tick(t0 + Duration::from_secs(3600)));
        assert_eq!(status.message(), "stays");
    }

    #[test]
    fn wire_tone_round_trips() {
        for tone in [
            StatusTone::Info,
            StatusTone::Busy,
            StatusTone::Warning,
            StatusTone::Error,
        ] {
            assert_eq!(StatusTone::from_wire(tone.as_wire()), tone);
        }
        // Unknown tones fall back to Info.
        assert_eq!(StatusTone::from_wire("nonsense"), StatusTone::Info);
    }
}
