//! Physical and logical pixels, and how results are written.

/// The scale of an output, read from its capture rather than from the advertised integer
/// scale: niri's scales are fractional, and the capture's width is the truth.
pub fn scale(physical_width: u32, logical_width: i32) -> f64 {
    f64::from(physical_width) / f64::from(logical_width)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a positive pixel count divided by a positive scale, rounded"
)]
pub fn to_logical(physical: u32, scale: f64) -> u32 {
    (f64::from(physical) / scale).round() as u32
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..limit before the cast"
)]
pub fn to_physical(logical: f64, scale: f64, limit: u32) -> u32 {
    let last = f64::from(limit.saturating_sub(1));
    (logical * scale).floor().clamp(0.0, last) as u32
}

/// What is copied: ready to paste into configuration or code.
pub fn clipboard(width: u32, height: u32) -> String {
    format!("{width}x{height}")
}

/// What is shown.
pub fn label(width: u32, height: u32) -> String {
    format!("{width} × {height}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_comes_from_the_capture_and_the_logical_size() {
        assert!((scale(3840, 2560) - 1.5).abs() < 1e-9);
        assert!((scale(1920, 1920) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn logical_sizes_round_to_nearest() {
        assert_eq!(to_logical(150, 1.5), 100);
        assert_eq!(to_logical(151, 1.5), 101); // 100.67
        assert_eq!(to_logical(1, 1.5), 1); // 0.67
        assert_eq!(to_logical(1920, 1.0), 1920);
        assert_eq!(to_logical(3840, 2.0), 1920);
    }

    #[test]
    fn logical_points_map_into_the_capture() {
        assert_eq!(to_physical(100.4, 1.5, 3840), 150);
        assert_eq!(to_physical(0.0, 1.5, 3840), 0);
        // The far edge and beyond clamp to the last pixel.
        assert_eq!(to_physical(2560.0, 1.5, 3840), 3839);
        assert_eq!(to_physical(9999.0, 1.0, 1920), 1919);
    }

    #[test]
    fn results_are_written_two_ways() {
        assert_eq!(clipboard(412, 38), "412x38");
        assert_eq!(label(412, 38), "412 × 38");
    }
}
