use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Info,
    Busy,
    Warning,
    Error,
}

pub struct StatusLine {
    message: String,
    tone: StatusTone,
    since: Instant,
}

impl StatusLine {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            tone: StatusTone::Info,
            since: Instant::now(),
        }
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.tone = StatusTone::Info;
        self.since = Instant::now();
    }

    pub fn busy(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.tone = StatusTone::Busy;
        self.since = Instant::now();
    }

    pub fn warning(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.tone = StatusTone::Warning;
        self.since = Instant::now();
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.tone = StatusTone::Error;
        self.since = Instant::now();
    }

    pub fn text(&self) -> String {
        match self.tone {
            StatusTone::Busy => format!("{} {}", self.spinner_frame(), self.message),
            StatusTone::Info | StatusTone::Warning | StatusTone::Error => self.message.clone(),
        }
    }

    pub fn tone(&self) -> StatusTone {
        self.tone
    }

    /// The raw message text, without tone-specific prefixes or spinners.
    pub fn message(&self) -> &str {
        &self.message
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

    #[test]
    fn warning_tone_keeps_message_plain() {
        let mut status = StatusLine::new("ready");
        status.warning("something changed");
        assert_eq!(status.tone(), StatusTone::Warning);
        // The warning colour carries the meaning — no "[warning]" prefix.
        assert_eq!(status.text(), "something changed");
    }
}
