//! What the ruler draws over the frozen picture.
//!
//! Every line is horizontal or vertical and every fill a plain rectangle, so this is a handful
//! of clipped rectangle fills into the `Xrgb8888` buffer rather than a vector library. Each
//! drawing function returns the rectangle it touched, which is what gets damaged and, on the
//! next frame, restored.

use crate::edges::Bounds;

/// A colour as r, g, b, a with straight (not premultiplied) alpha.
pub type Rgba = [u8; 4];

/// The guides: one colour that reads on light and dark alike, thanks to the outline.
pub const ACCENT: Rgba = [0x4c, 0xd6, 0xff, 0xff];
pub const OUTLINE: Rgba = [0x00, 0x00, 0x00, 0xc8];
pub const LABEL_BG: Rgba = [0x10, 0x10, 0x14, 0xd8];
pub const LABEL_FG: Rgba = [0xf2, 0xf2, 0xf2, 0xff];
pub const LABEL_DIM: Rgba = [0xb0, 0xb0, 0xb8, 0xff];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.w).max(other.x + other.w);
        let bottom = (self.y + self.h).max(other.y + other.h);
        Rect {
            x,
            y,
            w: right - x,
            h: bottom - y,
        }
    }
}

/// An `Xrgb8888` buffer to draw into.
pub struct Canvas<'a> {
    data: &'a mut [u8],
    width: i32,
    height: i32,
    stride: usize,
}

impl<'a> Canvas<'a> {
    #[allow(clippy::cast_possible_wrap, reason = "output sizes are far below i32::MAX")]
    pub fn new(data: &'a mut [u8], width: u32, height: u32, stride: u32) -> Canvas<'a> {
        Canvas {
            data,
            width: width as i32,
            height: height as i32,
            stride: stride as usize,
        }
    }

    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    #[allow(clippy::cast_sign_loss, reason = "x and y are checked non-negative first")]
    #[allow(clippy::many_single_char_names, reason = "x, y and the r, g, b, a channels")]
    pub fn blend(&mut self, x: i32, y: i32, color: Rgba) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let i = y as usize * self.stride + x as usize * 4;
        let [r, g, b, a] = color;
        let alpha = u16::from(a);
        // B, G, R in memory.
        for (offset, channel) in [(0, b), (1, g), (2, r)] {
            let under = u16::from(self.data[i + offset]);
            let mixed = (u16::from(channel) * alpha + under * (255 - alpha)) / 255;
            self.data[i + offset] = u8::try_from(mixed).unwrap_or(u8::MAX);
        }
    }

    /// Fills `rect`, clipped to the canvas; returns the part actually touched.
    pub fn fill(&mut self, rect: Rect, color: Rgba) -> Option<Rect> {
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w).min(self.width);
        let y1 = (rect.y + rect.h).min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, color);
            }
        }
        Some(Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// An axis-aligned accent line `px` thick with a `px` outline on both sides.
fn line(canvas: &mut Canvas, from: (i32, i32), to: (i32, i32), px: i32) -> Rect {
    let x = from.0.min(to.0);
    let y = from.1.min(to.1);
    let core = Rect {
        x,
        y,
        w: (from.0 - to.0).abs() + px,
        h: (from.1 - to.1).abs() + px,
    };
    let outline = Rect {
        x: core.x - px,
        y: core.y - px,
        w: core.w + 2 * px,
        h: core.h + 2 * px,
    };
    canvas.fill(outline, OUTLINE);
    canvas.fill(core, ACCENT);
    outline
}

/// A small cross centred on the measured pixel, standing in for the hidden system cursor.
pub fn crosshair(canvas: &mut Canvas, at: (i32, i32), px: i32) -> Rect {
    let arm = 8 * px;
    let horizontal = line(canvas, (at.0 - arm, at.1), (at.0 + arm, at.1), px);
    let vertical = line(canvas, (at.0, at.1 - arm), (at.0, at.1 + arm), px);
    horizontal.union(vertical)
}

#[allow(clippy::cast_possible_wrap, reason = "output sizes are far below i32::MAX")]
pub fn spacing_guides(canvas: &mut Canvas, bounds: Bounds, at: (i32, i32), px: i32) -> Rect {
    let (x0, y0, x1, y1) = (bounds.x0 as i32, bounds.y0 as i32, bounds.x1 as i32, bounds.y1 as i32);
    let tick = 4 * px;
    let mut touched = line(canvas, (x0, at.1), (x1, at.1), px);
    touched = touched.union(line(canvas, (at.0, y0), (at.0, y1), px));
    touched = touched.union(line(canvas, (x0, at.1 - tick), (x0, at.1 + tick), px));
    touched = touched.union(line(canvas, (x1, at.1 - tick), (x1, at.1 + tick), px));
    touched = touched.union(line(canvas, (at.0 - tick, y0), (at.0 + tick, y0), px));
    touched.union(line(canvas, (at.0 - tick, y1), (at.0 + tick, y1), px))
}

/// The box's outline, drawn just outside it so its edge pixels stay visible.
#[allow(clippy::cast_possible_wrap, reason = "output sizes are far below i32::MAX")]
pub fn box_guides(canvas: &mut Canvas, bounds: Bounds, px: i32) -> Rect {
    let left = bounds.x0 as i32 - 2 * px;
    let top = bounds.y0 as i32 - 2 * px;
    let right = bounds.x1 as i32 + 1 + px;
    let bottom = bounds.y1 as i32 + 1 + px;
    let mut touched = line(canvas, (left, top), (right, top), px);
    touched = touched.union(line(canvas, (left, bottom), (right, bottom), px));
    touched = touched.union(line(canvas, (left, top), (left, bottom), px));
    touched.union(line(canvas, (right, top), (right, bottom), px))
}

/// Moves a label of `size` from `preferred` just enough to stay inside `area`.
pub fn place_label(size: (i32, i32), preferred: (i32, i32), area: (i32, i32)) -> (i32, i32) {
    (
        preferred.0.clamp(0, (area.0 - size.0).max(0)),
        preferred.1.clamp(0, (area.1 - size.1).max(0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edges::Bounds;

    fn canvas_data(width: u32, height: u32) -> Vec<u8> {
        // White, opaque: B G R X.
        [255u8, 255, 255, 255].repeat(width as usize * height as usize)
    }

    fn pixel(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let i = (y * width * 4 + x * 4) as usize;
        [data[i + 2], data[i + 1], data[i]]
    }

    #[test]
    fn opaque_colour_is_written_in_bgr_order() {
        let mut data = canvas_data(2, 1);
        Canvas::new(&mut data, 2, 1, 8).blend(1, 0, [10, 20, 30, 255]);
        assert_eq!(pixel(&data, 2, 1, 0), [10, 20, 30]);
        assert_eq!(pixel(&data, 2, 0, 0), [255, 255, 255]);
    }

    #[test]
    fn half_alpha_mixes_with_the_picture() {
        let mut data = canvas_data(1, 1);
        Canvas::new(&mut data, 1, 1, 4).blend(0, 0, [0, 0, 0, 128]);
        // 255 * 127 / 255 = 127.
        assert_eq!(pixel(&data, 1, 0, 0), [127, 127, 127]);
    }

    #[test]
    fn drawing_off_the_canvas_is_ignored() {
        let mut data = canvas_data(1, 1);
        let mut canvas = Canvas::new(&mut data, 1, 1, 4);
        canvas.blend(-1, 0, [0, 0, 0, 255]);
        canvas.blend(0, 1, [0, 0, 0, 255]);
        assert_eq!(pixel(&data, 1, 0, 0), [255, 255, 255]);
    }

    #[test]
    fn fills_are_clipped_and_report_what_they_touched() {
        let mut data = canvas_data(10, 10);
        let mut canvas = Canvas::new(&mut data, 10, 10, 40);
        let touched = canvas.fill(Rect { x: 8, y: -2, w: 5, h: 4 }, [0, 0, 0, 255]);
        assert_eq!(touched, Some(Rect { x: 8, y: 0, w: 2, h: 2 }));
        assert_eq!(canvas.fill(Rect { x: 20, y: 0, w: 5, h: 5 }, [0, 0, 0, 255]), None);
    }

    #[test]
    fn rects_union() {
        let a = Rect { x: 0, y: 0, w: 2, h: 2 };
        let b = Rect { x: 5, y: 1, w: 1, h: 4 };
        assert_eq!(a.union(b), Rect { x: 0, y: 0, w: 6, h: 5 });
    }

    #[test]
    fn the_box_outline_leaves_the_measured_pixels_alone() {
        let mut data = canvas_data(30, 30);
        let bounds = Bounds { x0: 10, y0: 10, x1: 19, y1: 19 };
        let touched = box_guides(&mut Canvas::new(&mut data, 30, 30, 120), bounds, 1);
        for (x, y) in [(10, 10), (19, 19), (10, 19), (19, 10), (15, 15)] {
            assert_eq!(pixel(&data, 30, x, y), [255, 255, 255], "({x},{y}) was painted");
        }
        assert_ne!(pixel(&data, 30, 9, 9), [255, 255, 255], "the outline is just outside");
        assert!(touched.x <= 9 && touched.y <= 9);
        assert!(touched.x + touched.w >= 21 && touched.y + touched.h >= 21);
    }

    #[test]
    fn spacing_guides_cover_the_gap_and_report_it() {
        let mut data = canvas_data(30, 30);
        let bounds = Bounds { x0: 5, y0: 8, x1: 24, y1: 20 };
        let touched = spacing_guides(&mut Canvas::new(&mut data, 30, 30, 120), bounds, (12, 14), 1);
        assert_ne!(pixel(&data, 30, 5, 14), [255, 255, 255], "the row line reaches x0");
        assert_ne!(pixel(&data, 30, 12, 20), [255, 255, 255], "the column line reaches y1");
        assert!(touched.x <= 5 && touched.x + touched.w >= 25);
        assert!(touched.y <= 8 && touched.y + touched.h >= 21);
    }

    #[test]
    fn the_label_stays_on_the_output() {
        let area = (100, 50);
        assert_eq!(place_label((20, 10), (10, 10), area), (10, 10));
        assert_eq!(place_label((20, 10), (90, 45), area), (80, 40));
        assert_eq!(place_label((20, 10), (-5, -8), area), (0, 0));
    }
}
