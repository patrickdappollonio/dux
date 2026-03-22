use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Info,
    Busy,
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

    pub fn error(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.tone = StatusTone::Error;
        self.since = Instant::now();
    }

    pub fn text(&self) -> String {
        match self.tone {
            StatusTone::Busy => format!("{} {}", self.spinner_frame(), self.message),
            StatusTone::Error => format!("[error] {}", self.message),
            StatusTone::Info => self.message.clone(),
        }
    }

    pub fn tone(&self) -> StatusTone {
        self.tone
    }

    fn spinner_frame(&self) -> &'static str {
        const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let index = ((self.since.elapsed().as_millis() / 100) as usize) % FRAMES.len();
        FRAMES[index]
    }
}
