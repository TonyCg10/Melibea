//! A frozen frame, in the layout niri's captures arrive in.

/// A frozen frame: `Xrgb8888`, which in memory is B, G, R, X per pixel.
#[derive(Clone, Debug)]
pub struct Image {
    width: u32,
    height: u32,
    stride: u32,
    data: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, stride: u32, data: Vec<u8>) -> Image {
        assert!(stride >= width * 4, "stride shorter than a row");
        assert!(data.len() >= stride as usize * height as usize, "data shorter than the frame");
        Image {
            width,
            height,
            stride,
            data,
        }
    }

    #[allow(clippy::many_single_char_names)]
    pub fn from_rgb(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let stride = width * 4;
        let mut data = vec![0; stride as usize * height as usize];
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = f(x, y);
                let i = y as usize * stride as usize + x as usize * 4;
                data[i..i + 4].copy_from_slice(&[b, g, r, 0xff]);
            }
        }
        Image::new(width, height, stride, data)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> u32 {
        self.stride
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        let i = y as usize * self.stride as usize + x as usize * 4;
        [self.data[i + 2], self.data[i + 1], self.data[i]]
    }

    /// Captures can arrive bottom-up; the flag says so, and this puts them right.
    pub fn flip_vertical(&mut self) {
        let row = self.stride as usize;
        let rows = self.height as usize;
        for y in 0..rows / 2 {
            let (top, bottom) = self.data.split_at_mut((rows - 1 - y) * row);
            top[y * row..(y + 1) * row].swap_with_slice(&mut bottom[..row]);
        }
    }
}

/// How an output's framebuffer relates to its logical content, as `wl_output.transform`.
///
/// The compositor applies the transform to the logical content to make the framebuffer, and
/// screencopy hands back the framebuffer. A `Flipped*` value is a mirror about the vertical
/// axis, then a counter-clockwise rotation by the named angle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transform {
    #[default]
    Normal,
    Rotated90,
    Rotated180,
    Rotated270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Image {
    /// Puts a capture into the output's logical orientation: undoes `y_invert` first (the
    /// capture is bottom-up when set), then the inverse of the output transform. A quarter turn
    /// swaps the width and the height.
    #[must_use]
    pub fn oriented(mut self, transform: Transform, y_invert: bool) -> Image {
        if y_invert {
            self.flip_vertical();
        }
        if transform == Transform::Normal {
            return self;
        }
        let (pw, ph) = (self.width, self.height);
        let quarter = matches!(
            transform,
            Transform::Rotated90 | Transform::Rotated270 | Transform::Flipped90 | Transform::Flipped270
        );
        let (width, height) = if quarter { (ph, pw) } else { (pw, ph) };
        // Each logical pixel (x, y) is read from this framebuffer pixel.
        Image::from_rgb(width, height, |x, y| {
            let (sx, sy) = match transform {
                Transform::Normal => (x, y),
                Transform::Rotated90 => (y, ph - 1 - x),
                Transform::Rotated180 => (pw - 1 - x, ph - 1 - y),
                Transform::Rotated270 => (pw - 1 - y, x),
                Transform::Flipped => (pw - 1 - x, y),
                Transform::Flipped90 => (y, x),
                Transform::Flipped180 => (x, ph - 1 - y),
                Transform::Flipped270 => (pw - 1 - y, ph - 1 - x),
            };
            self.rgb(sx, sy)
        })
    }
}

/// Whether two colours are the same within `tolerance` on every channel.
pub fn matches(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_are_read_from_bgrx_memory() {
        // One pixel: B=1, G=2, R=3, X=255.
        let image = Image::new(1, 1, 4, vec![1, 2, 3, 255]);
        assert_eq!(image.rgb(0, 0), [3, 2, 1]);
    }

    #[test]
    fn stride_padding_is_skipped() {
        // Two rows of one pixel each, 8 bytes per row.
        let image = Image::new(1, 2, 8, vec![1, 1, 1, 0, 9, 9, 9, 9, 2, 2, 2, 0, 9, 9, 9, 9]);
        assert_eq!(image.rgb(0, 1), [2, 2, 2]);
    }

    #[test]
    fn from_rgb_round_trips() {
        let image = Image::from_rgb(3, 2, |x, y| [u8::try_from(x).unwrap(), u8::try_from(y).unwrap(), 7]);
        assert_eq!(image.rgb(2, 1), [2, 1, 7]);
        assert_eq!(image.width(), 3);
        assert_eq!(image.height(), 2);
    }

    #[test]
    fn flip_vertical_swaps_rows() {
        let mut image = Image::from_rgb(1, 3, |_, y| [u8::try_from(y).unwrap(), 0, 0]);
        image.flip_vertical();
        assert_eq!(image.rgb(0, 0), [2, 0, 0]);
        assert_eq!(image.rgb(0, 2), [0, 0, 0]);
    }

    /// 3x2, red channel = 10 * row + column: [0 1 2] over [10 11 12].
    fn numbered() -> Image {
        Image::from_rgb(3, 2, |x, y| [u8::try_from(10 * y + x).unwrap(), 0, 0])
    }

    fn rows(image: &Image) -> Vec<Vec<u8>> {
        (0..image.height())
            .map(|y| (0..image.width()).map(|x| image.rgb(x, y)[0]).collect())
            .collect()
    }

    #[test]
    fn normal_is_the_identity() {
        let image = numbered().oriented(Transform::Normal, false);
        assert_eq!(rows(&image), [[0, 1, 2], [10, 11, 12]]);
    }

    #[test]
    fn y_invert_flips_the_rows() {
        let image = numbered().oriented(Transform::Normal, true);
        assert_eq!(rows(&image), [[10, 11, 12], [0, 1, 2]]);
    }

    #[test]
    fn flipped_180_is_a_vertical_flip() {
        let image = numbered().oriented(Transform::Flipped180, false);
        assert_eq!(rows(&image), [[10, 11, 12], [0, 1, 2]]);
    }

    #[test]
    fn rotated_180_turns_the_image_half_way() {
        let image = numbered().oriented(Transform::Rotated180, false);
        assert_eq!(rows(&image), [[12, 11, 10], [2, 1, 0]]);
    }

    #[test]
    fn flipped_is_a_horizontal_mirror() {
        let image = numbered().oriented(Transform::Flipped, false);
        assert_eq!(rows(&image), [[2, 1, 0], [12, 11, 10]]);
    }

    #[test]
    fn rotated_90_undoes_a_counter_clockwise_turn() {
        let image = numbered().oriented(Transform::Rotated90, false);
        assert_eq!((image.width(), image.height()), (2, 3));
        assert_eq!(rows(&image), [[10, 0], [11, 1], [12, 2]]);
    }

    #[test]
    fn rotated_270_undoes_a_clockwise_turn() {
        let image = numbered().oriented(Transform::Rotated270, false);
        assert_eq!((image.width(), image.height()), (2, 3));
        assert_eq!(rows(&image), [[2, 12], [1, 11], [0, 10]]);
    }

    #[test]
    fn the_quarter_turns_cancel_each_other() {
        let image = numbered()
            .oriented(Transform::Rotated90, false)
            .oriented(Transform::Rotated270, false);
        assert_eq!(rows(&image), [[0, 1, 2], [10, 11, 12]]);
    }

    #[test]
    fn flipped_90_is_a_transpose() {
        let image = numbered().oriented(Transform::Flipped90, false);
        assert_eq!((image.width(), image.height()), (2, 3));
        assert_eq!(rows(&image), [[0, 10], [1, 11], [2, 12]]);
    }

    #[test]
    fn flipped_270_is_the_other_diagonal() {
        let image = numbered().oriented(Transform::Flipped270, false);
        assert_eq!((image.width(), image.height()), (2, 3));
        assert_eq!(rows(&image), [[12, 2], [11, 1], [10, 0]]);
    }

    #[test]
    fn y_invert_applies_before_the_transform() {
        let image = numbered().oriented(Transform::Rotated90, true);
        assert_eq!(rows(&image), [[0, 10], [1, 11], [2, 12]]);
    }

    #[test]
    fn matching_is_per_channel_and_inclusive() {
        assert!(matches([100, 100, 100], [130, 70, 100], 30));
        assert!(!matches([100, 100, 100], [131, 100, 100], 30));
        assert!(!matches([100, 100, 100], [100, 100, 69], 30));
        assert!(matches([0, 0, 0], [0, 0, 0], 0));
        assert!(!matches([0, 0, 0], [0, 0, 1], 0));
    }
}
