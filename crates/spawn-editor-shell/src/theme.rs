//! The editor's single dark theme.
//!
//! A named ramp expressed as constants, no runtime theming. The accent
//! (`#C1440E`) is reserved for selection/active/focus state only; everything else
//! is neutral surface/text. Grounded in `docs/research/editor-design.md` §6.

use spawn_core::Color;

/// The editor color ramp: three neutral surface steps, two text steps, and the
/// rust accent (plus a dimmed accent for hover/secondary state).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Editor chrome / panel backgrounds (near-black, not pure black).
    pub surface_base: Color,
    /// Inspector rows, the active panel, the toolbar.
    pub surface_raised: Color,
    /// Menus / tooltips.
    pub surface_overlay: Color,
    /// Primary text (slightly off-white to cut halation).
    pub text_primary: Color,
    /// Labels, units, disabled text.
    pub text_muted: Color,
    /// Selection / active / focus accent (`#C1440E`).
    pub accent: Color,
    /// Hover / secondary state of the accent.
    pub accent_dim: Color,
}

impl Theme {
    /// The editor's dark theme.
    pub fn dark() -> Self {
        Self {
            surface_base: Color::new(0.07, 0.07, 0.08, 1.0),
            surface_raised: Color::new(0.12, 0.12, 0.14, 1.0),
            surface_overlay: Color::new(0.16, 0.16, 0.19, 1.0),
            text_primary: Color::new(0.90, 0.90, 0.92, 1.0),
            text_muted: Color::new(0.55, 0.55, 0.58, 1.0),
            // #C1440E = rgb(193, 68, 14).
            accent: Color::new(193.0 / 255.0, 68.0 / 255.0, 14.0 / 255.0, 1.0),
            accent_dim: Color::new(0.50, 0.18, 0.04, 1.0),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_ramp_is_distinct_and_accent_is_c1440e() {
        let t = Theme::dark();
        // The accent decodes #C1440E.
        assert!((t.accent.r - 193.0 / 255.0).abs() < 1e-6);
        assert!((t.accent.g - 68.0 / 255.0).abs() < 1e-6);
        assert!((t.accent.b - 14.0 / 255.0).abs() < 1e-6);
        // Surfaces ascend in lightness; text is brighter than any surface.
        assert!(t.surface_base.r < t.surface_raised.r);
        assert!(t.surface_raised.r < t.surface_overlay.r);
        assert!(t.text_muted.r < t.text_primary.r);
        assert!(t.surface_overlay.r < t.text_primary.r);
        // Not pure black (cuts halation).
        assert!(t.surface_base.r > 0.0);
    }
}
