//! Session-only UTC jump editor. `Effect::Jump` requests a viewport change, not
//! a claim that a candle exists. The controller must page native history in order
//! for the current market and report unavailable or incomplete coverage itself.

use crate::Message;
use chrono::{DateTime, NaiveDate, Utc};
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Length, Task, keyboard};

const INPUT_ID: &str = "goto-time-utc";
const FORMAT_ERROR: &str = "Use YYYY-MM-DD or YYYY-MM-DD HH:MM[:SS] in UTC (optional T and Z).";

#[derive(Debug, Clone)]
pub enum Event {
    Changed(String),
    Submit,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// UTC epoch seconds. No history or inspection is created by this editor.
    Jump(i64),
    Close,
}

#[derive(Default)]
pub struct Editor {
    pub open: bool,
    input: String,
    error: Option<String>,
}

impl Editor {
    /// Seed from a real viewport/inspection timestamp; never consult local time.
    pub fn show(&mut self, time_seconds: i64) -> Task<Message> {
        self.open = true;
        self.error = None;
        self.input = DateTime::<Utc>::from_timestamp(time_seconds, 0)
            .map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        self.focus()
    }

    pub fn focus(&self) -> Task<Message> {
        text_input::focus(INPUT_ID)
    }

    /// Reopen a rejected jump with controller-provided native coverage feedback.
    /// Keep the submitted UTC input available for correction; change no chart data.
    pub fn reject(&mut self, message: impl Into<String>) -> Task<Message> {
        self.open = true;
        self.error = Some(message.into());
        self.focus()
    }

    /// Invalid submits leave the modal open with an explicit error; successful
    /// submits close it and return seconds for `Action::GotoTime` and paging.
    pub fn update(&mut self, event: Event) -> Option<Effect> {
        if !self.open {
            return None;
        }
        match event {
            Event::Changed(input) => {
                self.input = input;
                self.error = None;
                None
            }
            Event::Submit => match parse_utc(&self.input) {
                Ok(time) => {
                    self.open = false;
                    self.error = None;
                    Some(Effect::Jump(time))
                }
                Err(error) => {
                    self.error = Some(error.into());
                    None
                }
            },
            Event::Close => {
                self.open = false;
                self.error = None;
                Some(Effect::Close)
            }
        }
    }

    /// Route Escape before chart shortcuts. Enter is handled by the focused
    /// text input's on_submit; callers should route this only if not captured.
    pub fn key(&self, key: keyboard::Key<&str>, modifiers: keyboard::Modifiers) -> Option<Event> {
        if !self.open {
            return None;
        }
        match key {
            keyboard::Key::Named(keyboard::key::Named::Escape) => Some(Event::Close),
            keyboard::Key::Named(keyboard::key::Named::Enter)
                if !modifiers.control() && !modifiers.alt() && !modifiers.logo() =>
            {
                Some(Event::Submit)
            }
            _ => None,
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let mut body = column![
            text("Jump to UTC time").size(20),
            text("YYYY-MM-DD HH:MM[:SS] · date only means midnight UTC")
                .size(12).style(crate::ui::muted_text),
            text_input("2026-01-01 00:00:00", &self.input)
                .id(INPUT_ID)
                .on_input(|input| Message::GotoTime(Event::Changed(input)))
                .on_submit(Message::GotoTime(Event::Submit))
                .padding(9).style(crate::ui::input),
            text("UTC only, never local time. History availability depends on the native market; a jump does not invent a bar or price.")
                .size(12).style(crate::ui::muted_text),
        ].spacing(10);
        if let Some(error) = self.error.as_deref() {
            body = body.push(text(error).size(12).style(crate::ui::negative_text));
        }
        body = body.push(
            row![
                button("Jump [Enter]")
                    .on_press(Message::GotoTime(Event::Submit))
                    .style(crate::ui::primary),
                button("Cancel [Esc]")
                    .on_press(Message::GotoTime(Event::Close))
                    .style(crate::ui::quiet),
            ]
            .spacing(8),
        );
        container(body)
            .padding(18)
            .width(Length::Fixed(510.0))
            .style(crate::ui::panel)
            .into()
    }
}

/// Parse a deliberately small, unambiguous UTC grammar. Numeric offsets,
/// fractional seconds and leap seconds are rejected, never rounded/normalized.
/// Both date and time require zero-padded ASCII components; dates use years
/// 0001 through 9999. Date-only input is midnight UTC, with optional terminal Z.
pub fn parse_utc(input: &str) -> Result<i64, &'static str> {
    let input = input.trim();
    if input
        .get(10..)
        .is_some_and(|suffix| suffix.contains('+') || suffix.contains('-'))
    {
        return Err(
            "Timezone offsets are not accepted. Enter UTC directly, optionally ending in Z.",
        );
    }
    let input = input.strip_suffix('Z').unwrap_or(input);
    let bytes = input.as_bytes();
    if !matches!(bytes.len(), 10 | 16 | 19)
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return Err(FORMAT_ERROR);
    }
    let number = |start: usize, end: usize| -> u32 {
        bytes[start..end]
            .iter()
            .fold(0, |value, digit| value * 10 + u32::from(digit - b'0'))
    };
    let year = number(0, 4) as i32;
    let date = (year > 0)
        .then(|| NaiveDate::from_ymd_opt(year, number(5, 7), number(8, 10)))
        .flatten()
        .ok_or("Invalid UTC date: check the year, month, day and leap year.")?;
    let (hour, minute, second) = if bytes.len() == 10 {
        (0, 0, 0)
    } else {
        if !matches!(bytes[10], b' ' | b'T')
            || bytes[13] != b':'
            || !bytes[11..13].iter().all(u8::is_ascii_digit)
            || !bytes[14..16].iter().all(u8::is_ascii_digit)
            || (bytes.len() == 19
                && (bytes[16] != b':' || !bytes[17..19].iter().all(u8::is_ascii_digit)))
        {
            return Err(FORMAT_ERROR);
        }
        (
            number(11, 13),
            number(14, 16),
            if bytes.len() == 19 { number(17, 19) } else { 0 },
        )
    };
    date.and_hms_opt(hour, minute, second)
        .map(|time| time.and_utc().timestamp())
        .ok_or("Invalid UTC time: hour must be 00–23, minute and second 00–59; leap seconds are not supported.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_utc_and_date_only_without_a_local_timezone() {
        assert_eq!(parse_utc("1970-01-01"), Ok(0));
        assert_eq!(parse_utc("1970-01-01Z"), Ok(0));
        assert_eq!(parse_utc("1970-01-01 00:01"), Ok(60));
        assert_eq!(parse_utc("1970-01-01T00:01:02Z"), Ok(62));
        assert_eq!(parse_utc("1969-12-31 23:59:59"), Ok(-1));
        assert_eq!(parse_utc("2024-02-29"), parse_utc("2024-02-29T00:00:00Z"));
        assert!(parse_utc("2024-02-29").is_ok());
    }

    #[test]
    fn rejects_impossible_dates_times_offsets_and_non_strict_inputs() {
        for input in [
            "2023-02-29",
            "2024-04-31",
            "0000-01-01",
            "2024-00-01",
            "2024-01-00",
            "2024-01-01 24:00",
            "2024-01-01 00:60",
            "2024-01-01 00:00:60",
            "2024-1-01",
            "2024-01-1",
            "2024-01-01 1:00",
            "2024-01-01 00:00:00.1Z",
            "2024-01-01 00:00 UTC",
            "2024-01-01T00:00:00z",
            "２０２４-01-01",
            "",
            "1",
        ] {
            assert!(parse_utc(input).is_err(), "accepted {input:?}");
        }
        for input in ["2024-01-01T00:00:00+00:00", "2024-01-01 00:00-01:00"] {
            assert!(parse_utc(input).unwrap_err().contains("offsets"));
        }
    }

    #[test]
    fn invalid_submit_keeps_editor_open_and_cancel_has_no_jump_effect() {
        let mut editor = Editor {
            open: true,
            ..Editor::default()
        };
        assert_eq!(editor.update(Event::Changed("2023-02-29".into())), None);
        assert_eq!(editor.update(Event::Submit), None);
        assert!(editor.open);
        assert!(editor.error.is_some());
        assert_eq!(editor.update(Event::Changed("1970-01-01".into())), None);
        assert!(editor.error.is_none());
        assert_eq!(editor.update(Event::Submit), Some(Effect::Jump(0)));
        assert!(!editor.open);
        assert_eq!(editor.update(Event::Submit), None);
        editor.open = true;
        assert_eq!(editor.update(Event::Close), Some(Effect::Close));
        assert!(!editor.open);
    }

    #[test]
    fn native_history_rejection_reopens_the_submitted_time_for_correction() {
        let mut editor = Editor {
            open: true,
            ..Editor::default()
        };
        editor.update(Event::Changed("1970-01-01".into()));
        assert_eq!(editor.update(Event::Submit), Some(Effect::Jump(0)));
        let _ = editor.reject("Before this native market's available history");
        assert!(editor.open);
        assert_eq!(editor.input, "1970-01-01");
        assert_eq!(
            editor.error.as_deref(),
            Some("Before this native market's available history")
        );
        editor.update(Event::Changed("2024-01-01".into()));
        assert!(editor.error.is_none());
    }
}
