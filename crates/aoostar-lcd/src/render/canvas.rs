//! Minimal drawing surface: tiny-skia shapes plus ab_glyph text on an opaque
//! background. Because every pixel stays opaque, premultiplied and straight
//! colour are identical and text coverage blends directly into the pixmap.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use image::RgbImage;
use std::sync::LazyLock;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

use super::theme::{MIN_SIZE, Rgb};

#[derive(Debug, Clone, Copy)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Area {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
}

static MEDIUM: &[u8] = include_bytes!("../../fonts/RobotoCondensed-Medium.ttf");
static BOLD: &[u8] = include_bytes!("../../fonts/RobotoCondensed-Bold.ttf");
static SC_MEDIUM: &[u8] = include_bytes!("../../fonts/NotoSansSC-Medium.ttf");
static SC_BOLD: &[u8] = include_bytes!("../../fonts/NotoSansSC-Bold.ttf");
static KR_MEDIUM: &[u8] = include_bytes!("../../fonts/NotoSansKR-Medium.ttf");
static KR_BOLD: &[u8] = include_bytes!("../../fonts/NotoSansKR-Bold.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Medium,
    Bold,
}

struct Fonts {
    medium: FontRef<'static>,
    bold: FontRef<'static>,
    sc_medium: FontRef<'static>,
    sc_bold: FontRef<'static>,
    kr_medium: FontRef<'static>,
    kr_bold: FontRef<'static>,
}
// The fonts are compiled in; `check_fonts` proves they parse.
static FONTS: LazyLock<Fonts> = LazyLock::new(|| Fonts {
    medium: FontRef::try_from_slice(MEDIUM).expect("embedded medium font parses"),
    bold: FontRef::try_from_slice(BOLD).expect("embedded bold font parses"),
    sc_medium: FontRef::try_from_slice(SC_MEDIUM).expect("embedded SC medium font parses"),
    sc_bold: FontRef::try_from_slice(SC_BOLD).expect("embedded SC bold font parses"),
    kr_medium: FontRef::try_from_slice(KR_MEDIUM).expect("embedded KR medium font parses"),
    kr_bold: FontRef::try_from_slice(KR_BOLD).expect("embedded KR bold font parses"),
});
/// Fails if an embedded font cannot be parsed (used by `aoostar-lcd check`).
pub fn check_fonts() -> anyhow::Result<()> {
    FontRef::try_from_slice(MEDIUM)?;
    FontRef::try_from_slice(BOLD)?;
    FontRef::try_from_slice(SC_MEDIUM)?;
    FontRef::try_from_slice(SC_BOLD)?;
    FontRef::try_from_slice(KR_MEDIUM)?;
    FontRef::try_from_slice(KR_BOLD)?;
    Ok(())
}

fn font(weight: Weight) -> &'static FontRef<'static> {
    match weight {
        Weight::Medium => &FONTS.medium,
        Weight::Bold => &FONTS.bold,
    }
}

/// The font that has a glyph for `ch`; CJK codepoints fall back to the
/// subset Noto fonts, everything else to the default.
fn font_for(weight: Weight, ch: char) -> &'static FontRef<'static> {
    let c = ch as u32;
    let is_cjk = (0x2E80..=0x9FFF).contains(&c)
        || (0xAC00..=0xD7AF).contains(&c)
        || (0xF900..=0xFAFF).contains(&c)
        || (0x3000..=0x30FF).contains(&c)
        || (0x1100..=0x11FF).contains(&c);
    if !is_cjk {
        return font(weight);
    }
    let is_korean = (0xAC00..=0xD7AF).contains(&c) || (0x1100..=0x11FF).contains(&c);
    match (weight, is_korean) {
        (Weight::Medium, true) => &FONTS.kr_medium,
        (Weight::Bold, true) => &FONTS.kr_bold,
        (Weight::Medium, false) => &FONTS.sc_medium,
        (Weight::Bold, false) => &FONTS.sc_bold,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Text style: pixel size is the font's em height.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub size: f32,
    pub weight: Weight,
    pub color: Rgb,
}

impl Style {
    pub const fn new(size: f32, weight: Weight, color: Rgb) -> Self {
        Self {
            size,
            weight,
            color,
        }
    }

    pub const fn color(self, color: Rgb) -> Self {
        Self { color, ..self }
    }

    pub const fn size(self, size: f32) -> Self {
        Self { size, ..self }
    }
}

pub struct Canvas {
    pixmap: Pixmap,
}

impl Canvas {
    pub fn new(width: u32, height: u32, background: Rgb) -> Self {
        let mut pixmap = Pixmap::new(width, height).expect("non-zero canvas size");
        pixmap.fill(background.into());
        Self { pixmap }
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        if let Some(rect) = Rect::from_xywh(x, y, w, h) {
            self.pixmap
                .fill_rect(rect, &paint(color), Transform::identity(), None);
        }
    }

    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgb) {
        let radius = radius.min(w / 2.0).min(h / 2.0);
        if radius <= 0.0 {
            self.rect(x, y, w, h, color);
            return;
        }
        // Cubic approximation of a quarter circle.
        let k = radius * 0.552_284_8;
        let mut builder = PathBuilder::new();
        builder.move_to(x + radius, y);
        builder.line_to(x + w - radius, y);
        builder.cubic_to(
            x + w - radius + k,
            y,
            x + w,
            y + radius - k,
            x + w,
            y + radius,
        );
        builder.line_to(x + w, y + h - radius);
        builder.cubic_to(
            x + w,
            y + h - radius + k,
            x + w - radius + k,
            y + h,
            x + w - radius,
            y + h,
        );
        builder.line_to(x + radius, y + h);
        builder.cubic_to(
            x + radius - k,
            y + h,
            x,
            y + h - radius + k,
            x,
            y + h - radius,
        );
        builder.line_to(x, y + radius);
        builder.cubic_to(x, y + radius - k, x + radius - k, y, x + radius, y);
        builder.close();
        if let Some(path) = builder.finish() {
            self.pixmap.fill_path(
                &path,
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, radius: f32, color: Rgb) {
        if let Some(path) = PathBuilder::from_circle(cx, cy, radius) {
            self.pixmap.fill_path(
                &path,
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    /// Horizontal meter: track plus a filled share, `fraction` clamped to 0..=1.
    pub fn meter(&mut self, area: Area, fraction: f32, fill: Rgb, track: Rgb) {
        let Area { x, y, w, h } = area;
        self.rounded_rect(x, y, w, h, h / 2.0, track);
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if fraction > 0.0 {
            self.rounded_rect(x, y, (w * fraction).max(h), h, h / 2.0, fill);
        }
    }

    /// Width of `text` in pixels.
    pub fn measure(style: Style, text: &str) -> f32 {
        let mut width = 0.0;
        let mut previous: Option<(char, &'static FontRef<'static>)> = None;
        for ch in text.chars() {
            let f = font_for(style.weight, ch);
            let scaled = f.as_scaled(PxScale::from(style.size));
            let id = scaled.glyph_id(ch);
            if let Some((prev_ch, prev_font)) = previous
                && std::ptr::eq(prev_font, f)
            {
                width += scaled.kern(
                    prev_font
                        .as_scaled(PxScale::from(style.size))
                        .glyph_id(prev_ch),
                    id,
                );
            }
            width += scaled.h_advance(id);
            previous = Some((ch, f));
        }
        width
    }

    /// Cap height for vertical centring of digits and capitals.
    pub fn cap_height(style: Style) -> f32 {
        style.size * 0.711
    }

    /// Exact ink extents of `text`: how far the drawn pixels reach above
    /// and below the baseline. Vertical centring must use this, not a
    /// size ratio: without descenders the block would sit too low, without
    /// overshoot digits (Å) too high.
    pub fn ink_box(style: Style, text: &str) -> (f32, f32) {
        let scale = PxScale::from(style.size);
        let mut top = 0.0_f32;
        let mut bottom = 0.0_f32;
        let mut caret = 0.0;
        let mut previous: Option<(char, &'static FontRef<'static>)> = None;
        for ch in text.chars() {
            let font = font_for(style.weight, ch);
            let scaled = font.as_scaled(scale);
            let id = scaled.glyph_id(ch);
            if let Some((prev_ch, prev_font)) = previous
                && std::ptr::eq(prev_font, font)
            {
                caret += scaled.kern(prev_font.as_scaled(scale).glyph_id(prev_ch), id);
            }
            let glyph = id.with_scale_and_position(scale, point(caret, 0.0));
            caret += scaled.h_advance(id);
            previous = Some((ch, font));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                top = top.min(bounds.min.y);
                bottom = bottom.max(bounds.max.y);
            }
        }
        (-top, bottom)
    }

    /// Draws `text` with its baseline at `baseline`; returns the drawn width.
    pub fn text(&mut self, x: f32, baseline: f32, align: Align, style: Style, text: &str) -> f32 {
        let width = Self::measure(style, text);
        let start = match align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };
        let scale = PxScale::from(style.size);
        let (canvas_w, canvas_h) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let stride = self.pixmap.width() as usize;
        let pixels = self.pixmap.pixels_mut();
        let mut caret = start;
        let mut previous: Option<(char, &'static FontRef<'static>)> = None;
        for ch in text.chars() {
            let font = font_for(style.weight, ch);
            let scaled = font.as_scaled(scale);
            let id = scaled.glyph_id(ch);
            if let Some((prev_ch, prev_font)) = previous
                && std::ptr::eq(prev_font, font)
            {
                caret += scaled.kern(prev_font.as_scaled(scale).glyph_id(prev_ch), id);
            }
            let glyph = id.with_scale_and_position(scale, point(caret, baseline));
            caret += scaled.h_advance(id);
            previous = Some((ch, font));
            let Some(outlined) = font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= canvas_w || py >= canvas_h {
                    return;
                }
                let coverage = coverage.clamp(0.0, 1.0);
                let index = py as usize * stride + px as usize;
                let old = pixels[index];
                let blend = |dst: u8, src: u8| -> u8 {
                    (f32::from(dst) * (1.0 - coverage) + f32::from(src) * coverage).round() as u8
                };
                if let Some(color) = tiny_skia::PremultipliedColorU8::from_rgba(
                    blend(old.red(), style.color.0),
                    blend(old.green(), style.color.1),
                    blend(old.blue(), style.color.2),
                    255,
                ) {
                    pixels[index] = color;
                }
            });
        }
        width
    }

    /// Like [`Canvas::text`] but shrinks the font down to [`MIN_SIZE`], then
    /// truncates with an ellipsis, so the text never exceeds `max_width`.
    pub fn text_fit(
        &mut self,
        x: f32,
        baseline: f32,
        align: Align,
        style: Style,
        max_width: f32,
        text: &str,
    ) -> f32 {
        let fitted = fit(style, MIN_SIZE.min(style.size), max_width, text);
        self.text(x, baseline, align, fitted.0, &fitted.1)
    }

    pub fn into_rgb(self) -> RgbImage {
        let (width, height) = (self.pixmap.width(), self.pixmap.height());
        let mut out = RgbImage::new(width, height);
        for (target, source) in out.pixels_mut().zip(self.pixmap.pixels()) {
            *target = image::Rgb([source.red(), source.green(), source.blue()]);
        }
        out
    }
}

/// Picks the largest size ≥ `min_size` that fits, otherwise truncates.
pub fn fit(style: Style, min_size: f32, max_width: f32, text: &str) -> (Style, String) {
    let mut size = style.size;
    while size > min_size && Canvas::measure(style.size(size), text) > max_width {
        size -= 1.0;
    }
    let style = style.size(size.max(min_size));
    if Canvas::measure(style, text) <= max_width {
        return (style, text.to_owned());
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>().trim_end().to_owned() + "…";
        if Canvas::measure(style, &candidate) <= max_width {
            return (style, candidate);
        }
    }
    (style, String::new())
}

fn paint(color: Rgb) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.0, color.1, color.2, 255);
    paint.anti_alias = true;
    paint
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::theme::TEXT;

    #[test]
    fn fit_shrinks_then_truncates_within_width() {
        let style = Style::new(40.0, Weight::Bold, TEXT);
        let (fitted, text) = fit(style, 28.0, 120.0, "homelab-compose-very-long-name");
        assert_eq!(fitted.size, 28.0);
        assert!(text.ends_with('…'));
        assert!(Canvas::measure(fitted, &text) <= 120.0);

        let (fitted, text) = fit(style, 28.0, 400.0, "gitea");
        assert_eq!(fitted.size, 40.0);
        assert_eq!(text, "gitea");
    }

    #[test]
    fn embedded_fonts_cover_cyrillic_and_degree_sign() {
        for weight in [Weight::Medium, Weight::Bold] {
            for ch in "ПАМЯТЬ свободно ° … ·".chars() {
                assert_ne!(font(weight).glyph_id(ch).0, 0, "{ch:?} missing");
            }
        }
    }
}
