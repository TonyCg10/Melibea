//! The label: the size in large digits, the mode and tolerance underneath.

use std::process::Command;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};

use crate::draw::{Canvas, LABEL_BG, LABEL_DIM, LABEL_FG, Rect, Rgba};

/// Logical sizes; multiplied by the output's real (fractional) scale, so the label is the same
/// logical size on every output.
const MAIN_SIZE: f32 = 13.0;
const DETAIL_SIZE: f32 = 10.0;
const PADDING: f32 = 6.0;
const GAP: f32 = 2.0;

/// A logical length in physical pixels, rounded.
#[allow(clippy::cast_possible_truncation, reason = "lengths are small")]
fn physical(logical: f32, scale: f32) -> i32 {
    (logical * scale).round() as i32
}

pub struct Text {
    font: FontVec,
}

pub struct Label<'a> {
    pub main: &'a str,
    pub detail: &'a str,
}

impl Text {
    /// The system's monospace font, as fontconfig names it.
    pub fn load() -> Result<Text, String> {
        let output = Command::new("fc-match")
            .args(["-f", "%{file}", "monospace"])
            .output()
            .map_err(|e| format!("cannot run fc-match: {e}"))?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if path.is_empty() {
            return Err("fc-match found no monospace font".to_owned());
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let font =
            FontVec::try_from_vec(bytes).map_err(|_| format!("{path} is not a usable font"))?;
        Ok(Text::from_font(font))
    }

    pub fn from_font(font: FontVec) -> Text {
        Text { font }
    }

    #[allow(clippy::cast_possible_truncation, reason = "glyph metrics are small")]
    fn line_size(&self, s: &str, size: f32) -> (i32, i32) {
        let scaled = self.font.as_scaled(PxScale::from(size));
        let width: f32 = s
            .chars()
            .map(|c| scaled.h_advance(self.font.glyph_id(c)))
            .sum();
        (width.ceil() as i32, scaled.height().ceil() as i32)
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        reason = "glyph metrics are small and coverage is clamped to 0..=1"
    )]
    fn draw_line(&self, canvas: &mut Canvas, s: &str, at: (i32, i32), size: f32, color: Rgba) {
        let scaled = self.font.as_scaled(PxScale::from(size));
        let mut caret = 0.0f32;
        for c in s.chars() {
            let id = self.font.glyph_id(c);
            let glyph = id.with_scale_and_position(size, point(caret, scaled.ascent()));
            caret += scaled.h_advance(id);
            if let Some(outlined) = self.font.outline_glyph(glyph) {
                let origin = outlined.px_bounds().min;
                outlined.draw(|gx, gy, coverage| {
                    let alpha = (f32::from(color[3]) * coverage.clamp(0.0, 1.0)).round() as u8;
                    canvas.blend(
                        at.0 + origin.x as i32 + gx as i32,
                        at.1 + origin.y as i32 + gy as i32,
                        [color[0], color[1], color[2], alpha],
                    );
                });
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation, reason = "scales are small")]
pub fn label_size(text: &Text, label: &Label, scale: f64) -> (i32, i32) {
    let scale = scale as f32;
    let main = text.line_size(label.main, MAIN_SIZE * scale);
    let detail = text.line_size(label.detail, DETAIL_SIZE * scale);
    let (padding, gap) = (physical(PADDING, scale), physical(GAP, scale));
    (
        main.0.max(detail.0) + 2 * padding,
        main.1 + gap + detail.1 + 2 * padding,
    )
}

#[allow(clippy::cast_possible_truncation, reason = "scales are small")]
pub fn draw_label(
    text: &Text,
    canvas: &mut Canvas,
    label: &Label,
    at: (i32, i32),
    scale: f64,
) -> Rect {
    let (w, h) = label_size(text, label, scale);
    let scale = scale as f32;
    let background = Rect {
        x: at.0,
        y: at.1,
        w,
        h,
    };
    canvas.fill(background, LABEL_BG);

    let main_size = MAIN_SIZE * scale;
    let main_height = text.line_size(label.main, main_size).1;
    let (padding, gap) = (physical(PADDING, scale), physical(GAP, scale));
    let inner = (at.0 + padding, at.1 + padding);
    text.draw_line(canvas, label.main, inner, main_size, LABEL_FG);
    text.draw_line(
        canvas,
        label.detail,
        (inner.0, inner.1 + main_height + gap),
        DETAIL_SIZE * scale,
        LABEL_DIM,
    );
    background
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Canvas;

    #[test]
    fn longer_text_is_wider() {
        let text = Text::load().expect("a monospace font via fc-match");
        let short = label_size(
            &text,
            &Label {
                main: "1 × 1",
                detail: "box",
            },
            1.0,
        );
        let long = label_size(
            &text,
            &Label {
                main: "1234 × 5678",
                detail: "box",
            },
            1.0,
        );
        assert!(long.0 > short.0);
        assert!(short.1 > 0);
    }

    #[test]
    fn the_label_is_the_same_logical_size_at_scale_one_and_a_half() {
        let text = Text::load().expect("a monospace font via fc-match");
        // A monospace line is about 1.0 to 1.4 em tall, so 13 px gives 13..=18, and the
        // same line at scale 1.5 is 19.5 px of font, about 1.5 times as tall (ceilings
        // may add a pixel or two).
        let one = text.line_size("412 × 38", MAIN_SIZE).1;
        let one_and_a_half = text.line_size("412 × 38", MAIN_SIZE * 1.5).1;
        assert!((13..=18).contains(&one), "{one}");
        assert!(
            (one_and_a_half - one * 3 / 2).abs() <= 2,
            "{one_and_a_half} is not about 1.5 x {one}"
        );
        let label = Label {
            main: "412 × 38",
            detail: "box",
        };
        assert!(label_size(&text, &label, 1.5).1 > label_size(&text, &label, 1.0).1);
    }

    #[test]
    fn the_label_paints_only_inside_what_it_reports() {
        let text = Text::load().expect("a monospace font via fc-match");
        let (width, height) = (200u32, 80u32);
        let mut data = [255u8, 255, 255, 255].repeat((width * height) as usize);
        let label = Label {
            main: "412 × 38",
            detail: "box · tol 30 · DP-1",
        };
        let touched = draw_label(
            &text,
            &mut Canvas::new(&mut data, width, height, width * 4),
            &label,
            (10, 10),
            1.0,
        );
        for y in 0..height {
            for x in 0..width {
                let i = ((y * width + x) * 4) as usize;
                let inside = (touched.x..touched.x + touched.w)
                    .contains(&i32::try_from(x).unwrap())
                    && (touched.y..touched.y + touched.h).contains(&i32::try_from(y).unwrap());
                if !inside {
                    assert_eq!(
                        &data[i..i + 3],
                        &[255, 255, 255],
                        "({x},{y}) painted outside"
                    );
                }
            }
        }
        assert_eq!((touched.x, touched.y), (10, 10));
    }
}
