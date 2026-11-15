//! The live post shader always writes alpha 255, including for a legitimately black frame.

pub(super) fn offsets(width: u32, height: u32) -> [u64; 9] {
    let xs = [0, width / 2, width - 1];
    let ys = [0, height / 2, height - 1];
    std::array::from_fn(|n| (ys[n / 3] as u64 * width as u64 + xs[n % 3] as u64) * 4)
}

pub(super) fn validate(words: &[u32]) -> Result<usize, &'static str> {
    if words.len() != 9 || words.iter().any(|p| p >> 24 != 255) {
        return Err("First output pixels were not written by the rendering pipeline");
    }
    Ok(words.iter().filter(|p| **p & 0x00ff_ffff != 0).count())
}

/// Per-channel byte bounds are diagnostic only: opaque black is a valid written output.
pub(super) fn rgb_bounds(words: &[u32]) -> ([u8; 3], [u8; 3]) {
    let mut min = [255; 3];
    let mut max = [0; 3];
    for word in words {
        for channel in 0..3 {
            let value = (word >> (channel * 8)) as u8;
            min[channel] = min[channel].min(value);
            max[channel] = max[channel].max(value);
        }
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_black_is_a_written_frame() { assert_eq!(validate(&[0xff00_0000; 9]), Ok(0)); }

    #[test]
    fn rgb_bounds_report_channels_without_rejecting_black() {
        assert_eq!(rgb_bounds(&[0xff00_0000; 9]), ([0; 3], [0; 3]));
        assert_eq!(rgb_bounds(&[0xff12_3456, 0xffab_cd01]), ([1, 0x34, 0x12], [0x56, 0xcd, 0xab]));
    }

    #[test]
    fn zero_partial_and_short_output_are_rejected() {
        assert!(validate(&[0; 9]).is_err());
        let mut partial = [0xffff_ffff; 9]; partial[4] = 0;
        assert!(validate(&partial).is_err());
        assert!(validate(&[0xffff_ffff; 8]).is_err());
    }

    #[test]
    fn probe_spans_the_output_without_aliasing_or_overrun() {
        for (w, h) in [(8, 8), (180, 314), (320, 180)] {
            let positions = offsets(w, h);
            assert_eq!(positions[0], 0);
            assert_eq!(positions[8], (w as u64 * h as u64 - 1) * 4);
            assert_eq!(positions.into_iter().collect::<std::collections::BTreeSet<_>>().len(), 9);
        }
        let mut words = [0xff00_0000; 9]; words[8] = 0xff01_0000;
        assert_eq!(validate(&words), Ok(1));
    }
}
