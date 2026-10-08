//! The label: the size in large digits, the mode and tolerance underneath.

use std::process::Command;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};

use crate::draw::{Canvas, LABEL_BG, LABEL_DIM, LABEL_FG, Rect, Rgba};

/// Logical sizes; multiplied by `px` for each output.
const MAIN_SIZE: i32 = 13;
const DETAIL_SIZE: i32 = 10;
const PADDING: i32 = 6;
const GAP: i32 = 2;

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

#[allow(clippy::cast_precision_loss, reason = "font sizes are small integers")]
pub fn label_size(text: &Text, label: &Label, px: i32) -> (i32, i32) {
    let main = text.line_size(label.main, (MAIN_SIZE * px) as f32);
    let detail = text.line_size(label.detail, (DETAIL_SIZE * px) as f32);
    (
        main.0.max(detail.0) + 2 * PADDING * px,
        main.1 + GAP * px + detail.1 + 2 * PADDING * px,
    )
}

#[allow(clippy::cast_precision_loss, reason = "font sizes are small integers")]
pub fn draw_label(
    text: &Text,
    canvas: &mut Canvas,
    label: &Label,
    at: (i32, i32),
    px: i32,
) -> Rect {
    let (w, h) = label_size(text, label, px);
    let background = Rect {
        x: at.0,
        y: at.1,
        w,
        h,
    };
    canvas.fill(background, LABEL_BG);

    let main_size = (MAIN_SIZE * px) as f32;
    let main_height = text.line_size(label.main, main_size).1;
    let inner = (at.0 + PADDING * px, at.1 + PADDING * px);
    text.draw_line(canvas, label.main, inner, main_size, LABEL_FG);
    text.draw_line(
        canvas,
        label.detail,
        (inner.0, inner.1 + main_height + GAP * px),
        (DETAIL_SIZE * px) as f32,
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
            1,
        );
        let long = label_size(
            &text,
            &Label {
                main: "1234 × 5678",
                detail: "box",
            },
            1,
        );
        assert!(long.0 > short.0);
        assert!(short.1 > 0);
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
            1,
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
