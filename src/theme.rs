#![allow(dead_code)]

use ratatui::prelude::{Color, Modifier, Style};
use ratatui::text::Span;

/// Braille dot-pattern frames for spinner animations. Shared by the loading
/// card, status line, and left-pane streaming indicator.
pub const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

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
    pub scroll_indicator_fg: Color,
    pub scroll_indicator_bg: Color,
    pub warning_fg: Color,
    pub button_active_fg: Color,
    pub button_confirm_border: Color,
    pub button_danger_border: Color,
    pub overlay_dim_fg: Color,
    pub diff_add_bg: Color,
    pub diff_remove_bg: Color,
    pub help_section_header_fg: Color,
    pub input_cursor_fg: Color,
    pub input_cursor_bg: Color,
    pub input_label_fg: Color,
    pub diff_binary_fg: Color,
    pub diff_stat_add_fg: Color,
    pub diff_stat_remove_fg: Color,
    pub runtime_context_value_fg: Color,
    pub nudge_border: Color,
    pub tip_pill_fg: Color,
    pub tip_pill_bg: Color,
    pub tip_text_fg: Color,
    pub tip_highlight_fg: Color,
    pub diff_line_number_fg: Color,
    pub diff_line_number_sep: Color,
    pub pr_open_fg: Color,
    pub pr_merged_fg: Color,
    pub pr_closed_fg: Color,
    pub pr_merged_label: Color,
    pub pr_closed_label: Color,
    pub pr_pill_border_fg: Color,
    pub pr_pill_secondary_fg: Color,
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
            overlay_dim_bg: Color::Rgb(10, 10, 10),
            prompt_cursor: Color::Cyan,
            provider_label_fg: Color::Rgb(100, 100, 100),
            branch_fg: Color::Cyan,
            terminal_hint_fg: Color::Rgb(80, 80, 80),
            scroll_indicator_fg: Color::Rgb(210, 210, 210),
            scroll_indicator_bg: Color::Rgb(55, 55, 55),
            warning_fg: Color::Yellow,
            button_active_fg: Color::White,
            button_confirm_border: Color::Cyan,
            button_danger_border: Color::Red,
            overlay_dim_fg: Color::DarkGray,
            diff_add_bg: Color::Rgb(20, 50, 20),
            diff_remove_bg: Color::Rgb(60, 20, 20),
            help_section_header_fg: Color::Cyan,
            input_cursor_fg: Color::Black,
            input_cursor_bg: Color::White,
            input_label_fg: Color::White,
            diff_binary_fg: Color::Yellow,
            diff_stat_add_fg: Color::Green,
            diff_stat_remove_fg: Color::Red,
            runtime_context_value_fg: Color::Rgb(125, 150, 160),
            nudge_border: Color::Rgb(180, 150, 50),
            tip_pill_fg: Color::Rgb(180, 180, 180),
            tip_pill_bg: Color::Rgb(70, 50, 120),
            tip_text_fg: Color::Rgb(90, 90, 90),
            tip_highlight_fg: Color::Rgb(0, 120, 120),
            diff_line_number_fg: Color::Rgb(90, 90, 110),
            diff_line_number_sep: Color::Rgb(60, 60, 70),
            pr_open_fg: Color::Rgb(35, 134, 54),
            pr_merged_fg: Color::Rgb(130, 80, 223),
            pr_closed_fg: Color::Rgb(110, 54, 48),
            pr_merged_label: Color::Rgb(170, 100, 220),
            pr_closed_label: Color::Rgb(140, 80, 80),
            pr_pill_border_fg: Color::White,
            pr_pill_secondary_fg: Color::Rgb(220, 220, 220),
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
            crate::statusline::StatusTone::Warning => {
                Style::default().fg(self.warning_fg).bg(self.status_info_bg)
            }
            crate::statusline::StatusTone::Error => Style::default()
                .fg(self.status_error_fg)
                .bg(self.status_error_bg),
        }
    }

    pub fn status_dot(&self, tone: crate::statusline::StatusTone) -> (&'static str, Color) {
        match tone {
            crate::statusline::StatusTone::Info => ("●", self.session_active),
            crate::statusline::StatusTone::Busy => ("●", self.session_detached),
            crate::statusline::StatusTone::Warning => ("●", self.warning_fg),
            crate::statusline::StatusTone::Error => ("●", self.status_error_fg),
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

    pub fn key_badge_default<'a>(&self, key: &'a str) -> Vec<Span<'a>> {
        self.key_badge(key, Color::Reset)
    }

    pub fn dim_key_badge_default<'a>(&self, key: &'a str) -> Vec<Span<'a>> {
        self.dim_key_badge(key, Color::Reset)
    }
}
