//! Flexbox-subset style model: [`Style`] and its component types.

use spawn_core::Color;

/// A length along one axis.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Dimension {
    /// Size is determined by content/flex rules rather than a fixed value.
    #[default]
    Auto,
    /// A fixed length in logical pixels.
    Px(f32),
    /// A fraction in `[0.0, 1.0]` of the parent's resolved content size along
    /// the relevant axis. Against an `Auto`/indefinite parent it resolves to
    /// `0` (see the layout module).
    Percent(f32),
}

/// Per-edge pixel lengths (margin or padding). No `Percent` edges in Phase 1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl Edges {
    pub const fn all(v: f32) -> Self {
        Self {
            left: v,
            right: v,
            top: v,
            bottom: v,
        }
    }

    pub const fn axis(horizontal: f32, vertical: f32) -> Self {
        Self {
            left: horizontal,
            right: horizontal,
            top: vertical,
            bottom: vertical,
        }
    }

    pub fn horizontal(self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(self) -> f32 {
        self.top + self.bottom
    }
}

/// Whether a node participates in layout/draw/hit-test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Display {
    Flex,
    /// Removes the node and its entire subtree from layout, the draw list, and
    /// hit testing.
    None,
}

/// Main-axis flow direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlexDirection {
    Row,
    Column,
}

/// Main-axis distribution of children.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JustifyContent {
    Start,
    Center,
    End,
    SpaceBetween,
}

/// Cross-axis alignment of children.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlignItems {
    Start,
    Center,
    End,
    Stretch,
}

/// Border drawn inset within the node's border box. It does not affect layout
/// size in Phase 1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Border {
    pub width: f32,
    pub color: Color,
}

/// A two-axis size constraint.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: Dimension,
    pub height: Dimension,
}

/// The full style of a node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub display: Display,
    pub flex_direction: FlexDirection,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Dimension,
    pub size: Size,
    /// `Auto` means "no minimum" (resolves to `0`); `min` wins over `max` on
    /// conflict.
    pub min_size: Size,
    /// `Auto` means "no maximum" (resolves to `+inf`).
    pub max_size: Size,
    pub margin: Edges,
    pub padding: Edges,
    /// Fixed pixels inserted between in-flow children along the main axis only.
    pub gap: f32,
    pub background: Color,
    pub border: Border,
    pub corner_radius: f32,
    /// When `true`, descendants are clipped to this node's content rect in both
    /// the draw list and hit testing.
    pub overflow_clip: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Start,
            align_items: AlignItems::Stretch,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Dimension::Auto,
            size: Size::default(),
            min_size: Size::default(),
            max_size: Size::default(),
            margin: Edges::default(),
            padding: Edges::default(),
            gap: 0.0,
            background: Color::TRANSPARENT,
            border: Border::default(),
            corner_radius: 0.0,
            overflow_clip: false,
        }
    }
}

const _: () = assert!(std::mem::size_of::<Dimension>() == 8);

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::traits::ApproxEq;

    #[test]
    fn edges_helpers() {
        let e = Edges::all(2.0);
        assert!(e.horizontal().approx_eq_default(4.0));
        assert!(e.vertical().approx_eq_default(4.0));
        let a = Edges::axis(3.0, 5.0);
        assert!(a.horizontal().approx_eq_default(6.0));
        assert!(a.vertical().approx_eq_default(10.0));
    }

    #[test]
    fn dimension_default_is_auto() {
        assert_eq!(Dimension::default(), Dimension::Auto);
    }

    #[test]
    fn style_default_matches_spec() {
        let s = Style::default();
        assert_eq!(s.display, Display::Flex);
        assert_eq!(s.flex_direction, FlexDirection::Row);
        assert_eq!(s.justify_content, JustifyContent::Start);
        assert_eq!(s.align_items, AlignItems::Stretch);
        assert!(s.flex_grow.approx_eq_default(0.0));
        assert!(s.flex_shrink.approx_eq_default(1.0));
        assert_eq!(s.flex_basis, Dimension::Auto);
        assert_eq!(s.size, Size::default());
        assert_eq!(s.background, Color::TRANSPARENT);
        assert!(s.border.width.approx_eq_default(0.0));
        assert!(!s.overflow_clip);
    }
}
