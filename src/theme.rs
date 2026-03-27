#![allow(dead_code)]

use ratatui::prelude::{Color, Modifier, Style};
use ratatui::text::Span;

pub struct Theme {
    pub header_fg: Color,
    pub header_bg: Color,
    pub header_label_fg: Color,
    pub header_separator_fg: Color,
    pub border_focused: Color,
    pub border_normal: Color,
    pub title_focused: Color,
    pub title_normal: Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
    pub project_icon: Color,
    pub session_active: Color,
    pub session_detached: Color,
    pub session_exited: Color,
    pub status_info_fg: Color,
    pub status_info_bg: Color,
    pub status_busy_fg: Color,
    pub status_busy_bg: Color,
    pub status_error_fg: Color,
    pub status_error_bg: Color,
    pub diff_add: Color,
    pub diff_remove: Color,
    pub diff_hunk: Color,
    pub diff_file_header: Color,
    pub diff_line_number: Color,
    pub file_status_fg: Color,
    pub hint_key_fg: Color,
    pub hint_bracket_fg: Color,
    pub hint_key_bg: Color,
    pub hint_desc_fg: Color,
    pub hint_dim_key_fg: Color,
    pub hint_dim_bracket_fg: Color,
    pub hint_dim_desc_fg: Color,
    pub hint_bar_bg: Color,
    pub overlay_border: Color,
    pub overlay_bg: Color,
    pub overlay_dim_bg: Color,
    pub prompt_cursor: Color,
    pub provider_label_fg: Color,
    pub branch_fg: Color,
    pub terminal_hint_fg: Color,
    pub warning_fg: Color,
    pub button_active_fg: Color,
    pub button_confirm_border: Color,
    pub button_danger_border: Color,
}

impl Theme {
    pub fn default_dark() -> Self {
        Self {
            header_fg: Color::White,
            header_bg: Color::Rgb(30, 30, 30),
            header_label_fg: Color::Rgb(120, 120, 120),
            header_separator_fg: Color::Rgb(60, 60, 60),
            border_focused: Color::Cyan,
            border_normal: Color::Rgb(80, 80, 80),
            title_focused: Color::Cyan,
            title_normal: Color::Rgb(140, 140, 140),
            selection_fg: Color::Black,
            selection_bg: Color::Cyan,
            project_icon: Color::Rgb(100, 149, 237),
            session_active: Color::Green,
            session_detached: Color::Yellow,
            session_exited: Color::Rgb(100, 100, 100),
            status_info_fg: Color::Rgb(100, 100, 100),
            status_info_bg: Color::Rgb(25, 25, 25),
            status_busy_fg: Color::Yellow,
            status_busy_bg: Color::Rgb(40, 35, 15),
            status_error_fg: Color::Red,
            status_error_bg: Color::Rgb(50, 20, 20),
            diff_add: Color::Green,
            diff_remove: Color::Red,
            diff_hunk: Color::Magenta,
            diff_file_header: Color::White,
            diff_line_number: Color::Rgb(110, 110, 110),
            file_status_fg: Color::Yellow,
            hint_key_fg: Color::Cyan,
            hint_bracket_fg: Color::DarkGray,
            hint_key_bg: Color::Rgb(35, 35, 35),
            hint_desc_fg: Color::Rgb(160, 160, 160),
            hint_dim_key_fg: Color::Rgb(80, 140, 160),
            hint_dim_bracket_fg: Color::Rgb(60, 60, 60),
            hint_dim_desc_fg: Color::Rgb(100, 100, 100),
            hint_bar_bg: Color::Rgb(25, 25, 25),
            overlay_border: Color::Cyan,
            overlay_bg: Color::Rgb(20, 20, 20),
            overlay_dim_bg: Color::Rgb(15, 15, 15),
            prompt_cursor: Color::Cyan,
            provider_label_fg: Color::Rgb(100, 100, 100),
            branch_fg: Color::Cyan,
            terminal_hint_fg: Color::Rgb(80, 80, 80),
            warning_fg: Color::Yellow,
            button_active_fg: Color::White,
            button_confirm_border: Color::Cyan,
            button_danger_border: Color::Red,
        }
    }

    pub fn header_style(&self) -> Style {
        Style::default().fg(self.header_fg).bg(self.header_bg)
    }

    pub fn border_style(&self, focused: bool) -> Style {
        if focused {
            Style::default().fg(self.border_focused)
        } else {
            Style::default().fg(self.border_normal)
        }
    }

    pub fn title_style(&self, focused: bool) -> Style {
        if focused {
            Style::default()
                .fg(self.title_focused)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.title_normal)
        }
    }

    pub fn selection_style(&self) -> Style {
        Style::default()
            .fg(self.selection_fg)
            .bg(self.selection_bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_style(&self, tone: crate::statusline::StatusTone) -> Style {
        match tone {
            crate::statusline::StatusTone::Info => Style::default()
                .fg(self.status_info_fg)
                .bg(self.status_info_bg),
            crate::statusline::StatusTone::Busy => Style::default()
                .fg(self.status_busy_fg)
                .bg(self.status_busy_bg),
            crate::statusline::StatusTone::Error => Style::default()
                .fg(self.status_error_fg)
                .bg(self.status_error_bg),
        }
    }

    pub fn status_dot(&self, tone: crate::statusline::StatusTone) -> (&'static str, Color) {
        match tone {
            crate::statusline::StatusTone::Info => ("●", self.session_active),
            crate::statusline::StatusTone::Busy => ("●", self.session_detached),
            crate::statusline::StatusTone::Error => ("●", Color::Red),
        }
    }

    pub fn session_dot(&self, status: &crate::model::SessionStatus) -> (&'static str, Color) {
        match status {
            crate::model::SessionStatus::Active => ("●", self.session_active),
            crate::model::SessionStatus::Detached => ("◐", self.session_detached),
            crate::model::SessionStatus::Exited => ("○", self.session_exited),
        }
    }

    /// Render a key badge as `<key>` with the angle brackets in an accent color
    /// and the key name in bold. Returns 3 spans.
    pub fn dim_key_badge<'a>(&self, key: &'a str, bg: Color) -> Vec<Span<'a>> {
        vec![
            Span::styled("<", Style::default().fg(self.hint_dim_bracket_fg).bg(bg)),
            Span::styled(
                key,
                Style::default()
                    .fg(self.hint_dim_key_fg)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(">", Style::default().fg(self.hint_dim_bracket_fg).bg(bg)),
        ]
    }

    pub fn key_badge<'a>(&self, key: &'a str, bg: Color) -> Vec<Span<'a>> {
        vec![
            Span::styled("<", Style::default().fg(self.hint_bracket_fg).bg(bg)),
            Span::styled(
                key,
                Style::default()
                    .fg(self.hint_key_fg)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(">", Style::default().fg(self.hint_bracket_fg).bg(bg)),
        ]
    }
}
