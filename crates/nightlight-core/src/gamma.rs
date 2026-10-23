// SPDX-License-Identifier: MPL-2.0

//! Color-temperature math: turn a Kelvin value into a per-channel gamma
//! ramp suitable for `drmModeCrtcSetGamma`.
//!
//! The white point is approximated with Tanner Helland's well-known
//! blackbody curve fit. At ~6500K the white point is (1, 1, 1), i.e. an
//! identity ramp that applies no tint; lower temperatures progressively
//! cut the green and blue channels to warm (redden) the image.

/// Lowest temperature we allow callers to request, in Kelvin.
pub const MIN_KELVIN: u32 = 1000;
/// Neutral daylight temperature; applying this is a no-op (identity ramp).
pub const NEUTRAL_KELVIN: u32 = 6500;
/// Highest temperature we allow callers to request, in Kelvin.
pub const MAX_KELVIN: u32 = 10000;

/// Returns the normalized RGB white point (each channel in `0.0..=1.0`)
/// for a black-body radiator at `kelvin`.
pub fn white_point(kelvin: u32) -> [f64; 3] {
    let kelvin = kelvin.clamp(MIN_KELVIN, MAX_KELVIN) as f64;
    let t = kelvin / 100.0;

    let red = if t <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)
    };

    let green = if t <= 66.0 {
        99.470_802_586_1 * t.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
    };

    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
    };

    [
        (red / 255.0).clamp(0.0, 1.0),
        (green / 255.0).clamp(0.0, 1.0),
        (blue / 255.0).clamp(0.0, 1.0),
    ]
}

/// Builds red/green/blue gamma LUTs of length `size` for the given
/// temperature and overall `brightness` (`0.0..=1.0`, where `1.0` is full).
///
/// Each LUT is a linear ramp scaled by the channel's white point and the
/// brightness, expressed as the 16-bit values the DRM API expects.
pub fn ramp(kelvin: u32, brightness: f64, size: usize) -> [Vec<u16>; 3] {
    ramp_for(white_point(kelvin), brightness, size)
}

/// A true identity ramp, with every channel at full scale. Not quite the same
/// as [`ramp`] at [`NEUTRAL_KELVIN`], whose blue tops out around 98%.
pub fn identity(size: usize) -> [Vec<u16>; 3] {
    ramp_for([1.0, 1.0, 1.0], 1.0, size)
}

fn ramp_for(wp: [f64; 3], brightness: f64, size: usize) -> [Vec<u16>; 3] {
    let brightness = brightness.clamp(0.0, 1.0);
    let last = (size.max(1) - 1) as f64;

    let channel = |w: f64| -> Vec<u16> {
        (0..size)
            .map(|i| {
                let base = if last > 0.0 { i as f64 / last } else { 0.0 };
                let value = (base * w * brightness * 65535.0).round();
                value.clamp(0.0, 65535.0) as u16
            })
            .collect()
    };

    [channel(wp[0]), channel(wp[1]), channel(wp[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_is_identity_white_point() {
        let wp = white_point(NEUTRAL_KELVIN);
        assert!((wp[0] - 1.0).abs() < 0.02, "red {}", wp[0]);
        assert!((wp[1] - 1.0).abs() < 0.02, "green {}", wp[1]);
        assert!((wp[2] - 1.0).abs() < 0.02, "blue {}", wp[2]);
    }

    #[test]
    fn warm_cuts_blue_more_than_red() {
        let wp = white_point(3000);
        assert_eq!(wp[0], 1.0, "red should stay full when warm");
        assert!(wp[2] < wp[1], "blue should be cut below green");
        assert!(wp[1] < wp[0], "green should be cut below red");
    }

    #[test]
    fn ramp_endpoints_and_monotonic() {
        let [r, g, b] = ramp(3500, 1.0, 256);
        assert_eq!(r.len(), 256);
        assert_eq!(r[0], 0);
        // Red is unscaled at full brightness for warm temps -> tops out at max.
        assert_eq!(*r.last().unwrap(), 65535);
        // Blue is attenuated, so its top entry is below the red top.
        assert!(*b.last().unwrap() < *r.last().unwrap());
        // Non-decreasing.
        assert!(g.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn brightness_scales_down() {
        let [r, _, _] = ramp(6500, 0.5, 256);
        assert!(*r.last().unwrap() < 65535 / 2 + 1024);
    }
}
