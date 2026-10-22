//! Palette, type scale and value formatting.
//!
//! The panel is a ~4.3" 960x376 LCD (~225 ppi) usually read from a desk
//! away, so the smallest text is 34 px and headline values are 84+ px.

use crate::config::Tone;

use super::canvas::{Style, Weight};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl From<Rgb> for tiny_skia::Color {
    fn from(value: Rgb) -> Self {
        tiny_skia::Color::from_rgba8(value.0, value.1, value.2, 255)
    }
}

pub const BACKGROUND: Rgb = Rgb(0x08, 0x0C, 0x11);
pub const PANEL: Rgb = Rgb(0x12, 0x1A, 0x23);
pub const TRACK: Rgb = Rgb(0x24, 0x31, 0x3F);
pub const TEXT: Rgb = Rgb(0xEE, 0xF3, 0xF8);
pub const MUTED: Rgb = Rgb(0x93, 0xA3, 0xB4);
pub const DIM: Rgb = Rgb(0x5A, 0x69, 0x79);
pub const GREEN: Rgb = Rgb(0x3D, 0xD6, 0x8C);
pub const AMBER: Rgb = Rgb(0xFF, 0xB8, 0x3D);
pub const RED: Rgb = Rgb(0xFF, 0x5A, 0x52);
pub const BLUE: Rgb = Rgb(0x5A, 0xB0, 0xFF);
pub const GREY: Rgb = Rgb(0x8A, 0x96, 0xA4);

/// Status bar backgrounds: dark tints for calm states, solid red for outages.
pub const BAR_OK: Rgb = Rgb(0x0E, 0x2A, 0x1D);
pub const BAR_WARN: Rgb = Rgb(0x3D, 0x2C, 0x08);
pub const BAR_CRIT: Rgb = Rgb(0xC4, 0x2B, 0x25);
pub const BAR_MAINT: Rgb = Rgb(0x26, 0x2E, 0x38);
/// Full-screen alarm background (alternates with black every second).
pub const ALARM_RED: Rgb = Rgb(0xD8, 0x10, 0x10);

pub const VALUE_XL: Style = Style::new(108.0, Weight::Bold, TEXT);
pub const VALUE_L: Style = Style::new(84.0, Weight::Bold, TEXT);
pub const VALUE_M: Style = Style::new(64.0, Weight::Bold, TEXT);
pub const TITLE: Style = Style::new(46.0, Weight::Bold, TEXT);
pub const LABEL: Style = Style::new(38.0, Weight::Bold, MUTED);
pub const DETAIL: Style = Style::new(38.0, Weight::Medium, MUTED);
/// Smallest text on the panel.
pub const MIN_SIZE: f32 = 34.0;

pub fn tone_color(tone: Tone) -> Rgb {
    match tone {
        Tone::Normal => TEXT,
        Tone::Warning => AMBER,
        Tone::Critical => RED,
    }
}

/// Meter fill: calm values are green rather than white.
pub fn tone_fill(tone: Tone) -> Rgb {
    match tone {
        Tone::Normal => GREEN,
        Tone::Warning => AMBER,
        Tone::Critical => RED,
    }
}

const KIB: f64 = 1024.0;

/// Throughput in MB/s only, for compact `R 12 · W 140` lines.
pub fn megabytes(value: u64) -> String {
    let amount = value as f64 / (KIB * KIB);
    if amount < 0.05 {
        "0".to_owned()
    } else {
        significant(amount)
    }
}

fn significant(amount: f64) -> String {
    if amount < 9.95 {
        let text = format!("{amount:.1}");
        text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
    } else {
        format!("{amount:.0}")
    }
}

/// Stopwatch: `7:05`, `1:02:09`.
pub fn stopwatch(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (hours, minutes, secs) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

pub fn temperature(value: Option<f32>) -> String {
    value.map_or_else(|| "--".to_owned(), |value| format!("{value:.0}°"))
}

pub fn percent(value: Option<f32>) -> String {
    value.map_or_else(|| "--".to_owned(), |value| format!("{value:.0}%"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn megabyte_rates_hide_noise() {
        assert_eq!(megabytes(30_000), "0");
        assert_eq!(megabytes(2_621_440), "2.5");
    }

    #[test]
    fn stopwatch_formats_durations() {
        assert_eq!(stopwatch(425), "7:05");
        assert_eq!(stopwatch(3729), "1:02:09");
    }
}
