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
            // out_px is 0xAARRGGBB (BGRA8 in memory); report channels in RGB order.
            let value = (word >> ((2 - channel) * 8)) as u8;
            min[channel] = min[channel].min(value);
            max[channel] = max[channel].max(value);
        }
    }
    (min, max)
}

#[derive(Debug, PartialEq)]
pub(super) struct SignalBounds {
    pub samples: usize,
    pub nonzero: usize,
    pub nonfinite: usize,
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// Pixel is color/albedo/normal/emit/spec (five vec4<f32>). Compare summed linear
/// diffuse + emission + specular signals with post output; this is not a correctness test.
pub(super) fn signal_bounds(words: &[u32]) -> SignalBounds {
    let mut out = SignalBounds { samples: 0, nonzero: 0, nonfinite: 0, min: [f64::INFINITY; 3], max: [f64::NEG_INFINITY; 3] };
    for pixel in words.chunks_exact(20) {
        let mut nonzero = false;
        for channel in 0..3 {
            let mut sum = 0.0;
            for base in [0, 12, 16] {
                let value = f32::from_bits(pixel[base + channel]);
                if value.is_finite() { sum += value as f64; } else { out.nonfinite += 1; }
            }
            out.min[channel] = out.min[channel].min(sum);
            out.max[channel] = out.max[channel].max(sum);
            nonzero |= sum.abs() > 1e-6;
        }
        out.samples += 1;
        out.nonzero += usize::from(nonzero);
    }
    if out.samples == 0 { out.min = [0.0; 3]; out.max = [0.0; 3]; }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_black_is_a_written_frame() { assert_eq!(validate(&[0xff00_0000; 9]), Ok(0)); }

    #[test]
    fn rgb_bounds_report_channels_without_rejecting_black() {
        assert_eq!(rgb_bounds(&[0xff00_0000; 9]), ([0; 3], [0; 3]));
        assert_eq!(rgb_bounds(&[0xff12_3456, 0xffab_cd01]), ([0x12, 0x34, 1], [0xab, 0xcd, 0x56]));
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

    #[test]
    fn signals_distinguish_kernel_black_emission_and_nonfinite_data() {
        let mut words = [0u32; 40];
        assert_eq!(signal_bounds(&words).nonzero, 0);
        words[0] = 1.0f32.to_bits();
        words[12] = 0.5f32.to_bits();
        words[16] = 0.25f32.to_bits();
        words[20 + 13] = 2.0f32.to_bits();
        words[20 + 2] = f32::NAN.to_bits();
        assert_eq!(signal_bounds(&words), SignalBounds {
            samples: 2, nonzero: 2, nonfinite: 1, min: [0.0; 3], max: [1.75, 2.0, 0.0],
        });
    }
}
