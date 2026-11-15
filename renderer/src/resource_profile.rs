//! Browser resource reservations. The native renderer retains its environment-variable controls.
//! A browser host changes the profile only while its view and pending start are both absent.

pub const DESKTOP_CACHE_ENTRIES: u64 = 1 << 18;
pub const MOBILE_CACHE_ENTRIES: u64 = 1 << 16;
pub const MOBILE_MAX_OUTPUT_PIXELS: u32 = 1 << 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResourceProfile {
    #[default]
    Desktop,
    Mobile,
}

impl ResourceProfile {
    pub fn parse(name: &str) -> Result<Self, &'static str> {
        match name {
            "desktop" => Ok(Self::Desktop),
            "mobile" => Ok(Self::Mobile),
            _ => Err("resource profile must be 'desktop' or 'mobile'"),
        }
    }

    pub fn name(self) -> &'static str {
        match self { Self::Desktop => "desktop", Self::Mobile => "mobile" }
    }

    pub fn cache_entries(self) -> u64 {
        match self { Self::Desktop => DESKTOP_CACHE_ENTRIES, Self::Mobile => MOBILE_CACHE_ENTRIES }
    }

    /// Mobile keeps all existing scene data, but reserves no space for future geometry/instance additions.
    /// Existing transforms/camera edits remain valid; a structural edit can require a scene rebuild.
    pub fn edit_reserve_scale(self) -> f64 {
        match self {
            Self::Mobile => 0.0,
            Self::Desktop => std::env::var("THREERT_EDIT_RESERVE").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0),
        }
    }

    pub fn max_output_pixels(self) -> Option<u32> {
        match self { Self::Desktop => None, Self::Mobile => Some(MOBILE_MAX_OUTPUT_PIXELS) }
    }

    pub fn validate_output(self, width: u32, height: u32) -> Result<(), &'static str> {
        if let Some(limit) = self.max_output_pixels() {
            if width.checked_mul(height).is_none_or(|pixels| pixels > limit) {
                return Err("mobile resource profile: output exceeds 65536 pixels; use e.g. 320 x 180");
            }
        }
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
std::thread_local! {
    static PROFILE: std::cell::Cell<ResourceProfile> = const { std::cell::Cell::new(ResourceProfile::Desktop) };
}

/// Native execution always retains its original desktop defaults and environment controls.
pub fn current() -> ResourceProfile {
    #[cfg(target_arch = "wasm32")]
    { PROFILE.with(std::cell::Cell::get) }
    #[cfg(not(target_arch = "wasm32"))]
    { ResourceProfile::Desktop }
}

/// Call only with no live view and no pending start. The browser session export enforces that boundary.
pub fn set_browser_profile(profile: ResourceProfile) -> Result<(), &'static str> {
    #[cfg(target_arch = "wasm32")]
    { PROFILE.with(|value| value.set(profile)); Ok(()) }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if profile == ResourceProfile::Desktop { Ok(()) }
        else { Err("mobile resource profile is available only in the browser runtime") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_preserve_desktop_and_bound_mobile_cache() {
        assert_eq!(ResourceProfile::default(), ResourceProfile::Desktop);
        for (profile, expected) in [(ResourceProfile::Desktop, 262144), (ResourceProfile::Mobile, 65536)] {
            assert_eq!(profile.cache_entries(), expected);
            assert!(profile.cache_entries().is_power_of_two());
            assert!(profile.cache_entries() >= 8, "cache probing uses eight slots");
            assert_eq!(ResourceProfile::parse(profile.name()).unwrap(), profile);
        }
        assert_eq!(ResourceProfile::Mobile.edit_reserve_scale(), 0.0);
        assert_eq!((DESKTOP_CACHE_ENTRIES - MOBILE_CACHE_ENTRIES) * 68, 13_369_344);
        assert!(ResourceProfile::parse("Mobile").is_err());
        assert!(ResourceProfile::parse("").is_err());
    }

    #[test]
    fn mobile_output_is_bounded_without_silently_changing_size() {
        for (w, h) in [(320, 180), (180, 320), (256, 256)] {
            assert!(ResourceProfile::Mobile.validate_output(w, h).is_ok());
        }
        for (w, h) in [(640, 360), (257, 256), (u32::MAX, 8)] {
            assert!(ResourceProfile::Mobile.validate_output(w, h).is_err());
        }
        assert!(ResourceProfile::Desktop.validate_output(1920, 1080).is_ok());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_execution_cannot_be_switched_by_a_browser_profile() {
        assert!(set_browser_profile(ResourceProfile::Mobile).is_err());
        assert_eq!(current(), ResourceProfile::Desktop);
    }
}
