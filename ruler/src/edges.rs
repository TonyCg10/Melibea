//! Finding edges in a frozen frame, in physical pixels.
//!
//! A pixel belongs if no channel differs from the reference — the pixel under the pointer —
//! by more than the tolerance. Comparing with the reference rather than with the neighbour is
//! what makes a gradient end somewhere.

use crate::image::{Image, matches};

/// An inclusive rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Bounds {
    pub fn width(&self) -> u32 {
        self.x1 - self.x0 + 1
    }

    pub fn height(&self) -> u32 {
        self.y1 - self.y0 + 1
    }
}

/// The run of matching pixels along the pointer's row (`x0..=x1`) and column (`y0..=y1`).
pub fn spacing(image: &Image, x: u32, y: u32, tolerance: u8) -> Bounds {
    let reference = image.rgb(x, y);
    let same = |px: u32, py: u32| matches(image.rgb(px, py), reference, tolerance);

    let mut bounds = Bounds {
        x0: x,
        y0: y,
        x1: x,
        y1: y,
    };
    while bounds.x0 > 0 && same(bounds.x0 - 1, y) {
        bounds.x0 -= 1;
    }
    while bounds.x1 + 1 < image.width() && same(bounds.x1 + 1, y) {
        bounds.x1 += 1;
    }
    while bounds.y0 > 0 && same(x, bounds.y0 - 1) {
        bounds.y0 -= 1;
    }
    while bounds.y1 + 1 < image.height() && same(x, bounds.y1 + 1) {
        bounds.y1 += 1;
    }
    bounds
}

/// The connected area of one colour, as filled from a point.
pub struct Region {
    bounds: Bounds,
    width: u32,
    mask: Vec<bool>,
}

impl Region {
    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// Whether a point is in the filled area itself, not merely inside its bounds.
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x < self.width
            && self
                .mask
                .get(y as usize * self.width as usize + x as usize)
                .copied()
                .unwrap_or(false)
    }
}

/// Fills the 4-connected area matching the pixel at `(x, y)`.
///
/// Four rays would stop at the first letter of a button's label. The fill flows around the
/// label instead, so its bounding box is the whole button.
pub fn region(image: &Image, x: u32, y: u32, tolerance: u8) -> Region {
    let (width, height) = (image.width(), image.height());
    let index = |px: u32, py: u32| py as usize * width as usize + px as usize;
    let reference = image.rgb(x, y);

    let mut mask = vec![false; width as usize * height as usize];
    let mut bounds = Bounds {
        x0: x,
        y0: y,
        x1: x,
        y1: y,
    };
    let mut stack = vec![(x, y)];
    mask[index(x, y)] = true;

    while let Some((px, py)) = stack.pop() {
        bounds.x0 = bounds.x0.min(px);
        bounds.y0 = bounds.y0.min(py);
        bounds.x1 = bounds.x1.max(px);
        bounds.y1 = bounds.y1.max(py);

        let neighbours = [
            (px > 0).then(|| (px - 1, py)),
            (px + 1 < width).then(|| (px + 1, py)),
            (py > 0).then(|| (px, py - 1)),
            (py + 1 < height).then(|| (px, py + 1)),
        ];
        for (nx, ny) in neighbours.into_iter().flatten() {
            let i = index(nx, ny);
            if !mask[i] && matches(image.rgb(nx, ny), reference, tolerance) {
                mask[i] = true;
                stack.push((nx, ny));
            }
        }
    }

    Region {
        bounds,
        width,
        mask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BG: [u8; 3] = [255, 255, 255];
    const FG: [u8; 3] = [40, 90, 200];

    /// 40×30 white, with a blue button at x 10..=29, y 5..=14, "text" inside it at x 15..=17,
    /// y 8..=10.
    fn button() -> Image {
        Image::from_rgb(40, 30, |x, y| {
            let in_button = (10..=29).contains(&x) && (5..=14).contains(&y);
            let in_text = (15..=17).contains(&x) && (8..=10).contains(&y);
            if in_text {
                [0, 0, 0]
            } else if in_button {
                FG
            } else {
                BG
            }
        })
    }

    #[test]
    fn the_box_is_the_element_not_the_text_inside_it() {
        let region = region(&button(), 12, 6, 30);
        assert_eq!(
            region.bounds(),
            Bounds {
                x0: 10,
                y0: 5,
                x1: 29,
                y1: 14
            }
        );
        assert_eq!(region.bounds().width(), 20);
        assert_eq!(region.bounds().height(), 10);
    }

    #[test]
    fn the_region_knows_what_it_covers() {
        let region = region(&button(), 12, 6, 30);
        assert!(region.contains(29, 14));
        assert!(!region.contains(16, 9), "the text is a hole in the region");
        assert!(!region.contains(5, 5), "outside the button");
    }

    #[test]
    fn a_region_reaching_the_frame_edge_stops_there() {
        let region = region(&button(), 0, 0, 30);
        assert_eq!(
            region.bounds(),
            Bounds {
                x0: 0,
                y0: 0,
                x1: 39,
                y1: 29
            }
        );
    }

    #[test]
    fn diagonal_neighbours_do_not_connect() {
        // A white pixel at (0,0) and one at (1,1), black elsewhere.
        let image = Image::from_rgb(2, 2, |x, y| if x == y { BG } else { [0, 0, 0] });
        assert_eq!(
            region(&image, 0, 0, 0).bounds(),
            Bounds {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: 0
            }
        );
    }

    #[test]
    fn an_isolated_pixel_is_a_one_by_one_box() {
        let image = Image::from_rgb(5, 5, |x, y| if (x, y) == (2, 2) { FG } else { BG });
        let bounds = region(&image, 2, 2, 30).bounds();
        assert_eq!((bounds.width(), bounds.height()), (1, 1));
    }

    #[test]
    fn an_antialiased_edge_is_in_or_out_by_the_tolerance() {
        // Columns: 0..=4 at 100, column 5 at 125, 6..=9 at 200.
        let image = Image::from_rgb(10, 1, |x, _| match x {
            0..=4 => [100; 3],
            5 => [125; 3],
            _ => [200; 3],
        });
        assert_eq!(region(&image, 0, 0, 24).bounds().x1, 4);
        assert_eq!(region(&image, 0, 0, 25).bounds().x1, 5);
    }

    #[test]
    fn a_gradient_ends_where_it_leaves_the_tolerance() {
        // Each column one step brighter than the last.
        let image = Image::from_rgb(100, 1, |x, _| [u8::try_from(x).unwrap(); 3]);
        assert_eq!(
            region(&image, 10, 0, 30).bounds(),
            Bounds {
                x0: 0,
                y0: 0,
                x1: 40,
                y1: 0
            }
        );
    }

    #[test]
    fn spacing_is_the_gap_around_the_pointer() {
        // Inside the button, away from the text: row 6 spans the button, column 12 too.
        let bounds = spacing(&button(), 12, 6, 30);
        assert_eq!(
            bounds,
            Bounds {
                x0: 10,
                y0: 5,
                x1: 29,
                y1: 14
            }
        );
    }

    #[test]
    fn spacing_stops_at_the_first_different_pixel() {
        // On row 9 the text starts at x 15, so the ray to the right stops at 14.
        let bounds = spacing(&button(), 12, 9, 30);
        assert_eq!((bounds.x0, bounds.x1), (10, 14));
    }

    #[test]
    fn spacing_stops_at_the_frame_edge() {
        let bounds = spacing(&button(), 2, 2, 30);
        assert_eq!(
            bounds,
            Bounds {
                x0: 0,
                y0: 0,
                x1: 39,
                y1: 29
            }
        );
    }
}
