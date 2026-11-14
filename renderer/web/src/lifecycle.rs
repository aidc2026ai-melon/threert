//! Session epochs are acquired by the synchronous JS entry point, before the future's first poll.
//! A cancelled/older start may finish an adapter/device request, but must never publish its view.

#[derive(Default)]
pub(crate) struct Lifecycle {
    generation: u64,
    pending: bool,
}

impl Lifecycle {
    pub(crate) fn begin(&mut self) -> u64 {
        self.cancel();
        self.pending = true;
        self.generation
    }

    pub(crate) fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
    }

    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.pending && self.generation == generation
    }

    pub(crate) fn finish(&mut self, generation: u64) {
        if self.generation == generation {
            self.pending = false;
        }
    }

    pub(crate) fn snapshot(&self) -> (u64, bool) {
        (self.generation, self.pending)
    }
}

/// The core clamps its traced grid to at least 8 pixels and multiplies w*h as u32.
pub(crate) fn validate_output(width: u32, height: u32) -> Result<(), &'static str> {
    if width < 8 || height < 8 {
        return Err("start: output width and height must both be at least 8");
    }
    if width.checked_mul(height).is_none() {
        return Err("start: output pixel count exceeds u32");
    }
    Ok(())
}

/// Normalized canvas coordinates -> normalized fixed-output coordinates (the BLIT shader's letterbox).
pub(crate) fn surface_pick(u: f32, v: f32, width: u32, height: u32, sw: u32, sh: u32) -> Option<(f32, f32)> {
    if !u.is_finite() || !v.is_finite() || width == 0 || height == 0 || sw == 0 || sh == 0 { return None; }
    let (w, h, sw, sh) = (width as f32, height as f32, sw as f32, sh as f32);
    let fit = (sw / w).min(sh / h);
    let (shown_w, shown_h) = (w * fit, h * fit);
    let x = (u * sw - (sw - shown_w) * 0.5) / shown_w;
    let y = (v * sh - (sh - shown_h) * 0.5) / shown_h;
    ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y)).then_some((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_before_first_poll_invalidates_the_ticket() {
        let mut state = Lifecycle::default();
        let ticket = state.begin();
        state.cancel();
        assert!(!state.is_current(ticket));
        assert!(!state.snapshot().1);
    }

    #[test]
    fn an_old_completion_cannot_finish_a_new_start() {
        let mut state = Lifecycle::default();
        let first = state.begin();
        let second = state.begin();
        state.finish(first);
        assert!(!state.is_current(first));
        assert!(state.is_current(second));
        state.finish(second);
        assert!(!state.snapshot().1);
    }

    #[test]
    fn cancelled_completion_does_not_restore_pending_state() {
        let mut state = Lifecycle::default();
        let ticket = state.begin();
        state.cancel();
        let after_cancel = state.snapshot();
        state.finish(ticket);
        assert_eq!(state.snapshot(), after_cancel);
    }

    #[test]
    fn output_rejects_core_panics_before_allocating() {
        assert!(validate_output(0, 90).is_err());
        assert!(validate_output(160, 7).is_err());
        assert!(validate_output(u32::MAX, 8).is_err());
        assert!(validate_output(8, 8).is_ok());
        assert!(validate_output(160, 90).is_ok());
    }

    #[test]
    fn resized_presentation_picks_the_image_and_excludes_letterbox_bars() {
        assert_eq!(surface_pick(0.5, 0.5, 160, 90, 200, 200), Some((0.5, 0.5)));
        assert_eq!(surface_pick(0.5, 0.05, 160, 90, 200, 200), None);
        assert_eq!(surface_pick(0.05, 0.5, 90, 160, 200, 200), None);
        assert_eq!(surface_pick(0.25, 0.75, 160, 90, 320, 180), Some((0.25, 0.75)));
        assert_eq!(surface_pick(f32::NAN, 0.5, 160, 90, 320, 180), None);
    }
}
