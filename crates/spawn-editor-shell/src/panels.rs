//! The four-region docked layout: toolbar / outliner / viewport / inspector,
//! plus a status strip. Built with the step-b widgets over one `UiTree`. The
//! viewport panel is a transparent spacer whose laid-out rect drives the scene
//! viewport (camera aspect, picking, the overlay's scene region).

use spawn_core::{Rect, Vec2};
use spawn_ui::{
    AlignItems, Dimension, FlexDirection, NodeId, Panel, Size, Style, UiResult, UiTree,
};

use crate::theme::Theme;

const TOOLBAR_HEIGHT: f32 = 28.0;
const STATUS_HEIGHT: f32 = 18.0;
const OUTLINER_WIDTH: f32 = 170.0;
const INSPECTOR_WIDTH: f32 = 250.0;

/// The editor's panel node ids, built once and reused; their content is rebuilt
/// in place.
pub struct Panels {
    pub root: NodeId,
    pub toolbar: NodeId,
    pub outliner: NodeId,
    pub viewport: NodeId,
    pub inspector: NodeId,
    pub status: NodeId,
}

impl Panels {
    /// Builds the four-region layout under the tree root.
    pub fn build(tree: &mut UiTree, theme: &Theme) -> UiResult<Self> {
        let root = tree.root();
        tree.set_style(
            root,
            Style {
                flex_direction: FlexDirection::Column,
                background: theme.surface_base,
                ..Default::default()
            },
        )?;

        let toolbar = Panel::new(tree, root, bar_style(theme, TOOLBAR_HEIGHT))?;

        let middle = Panel::new(
            tree,
            root,
            Style {
                flex_direction: FlexDirection::Row,
                flex_grow: 1.0,
                align_items: AlignItems::Stretch,
                ..Default::default()
            },
        )?;
        let outliner = Panel::new(tree, middle, side_style(theme, OUTLINER_WIDTH))?;
        let viewport = Panel::new(
            tree,
            middle,
            Style {
                flex_grow: 1.0,
                // Transparent: the scene shows through; only its rect matters.
                ..Default::default()
            },
        )?;
        let inspector = Panel::new(tree, middle, side_style(theme, INSPECTOR_WIDTH))?;

        let status = Panel::new(tree, root, bar_style(theme, STATUS_HEIGHT))?;

        Ok(Self {
            root,
            toolbar,
            outliner,
            viewport,
            inspector,
            status,
        })
    }

    /// The scene viewport rectangle after layout (the viewport panel's border
    /// box). Falls back to the full `[0, fallback]` rect if layout is unavailable.
    pub fn viewport_rect(&self, tree: &UiTree, fallback: Vec2) -> Rect {
        tree.layout(self.viewport)
            .unwrap_or_else(|| Rect::new(Vec2::ZERO, fallback))
    }
}

fn bar_style(theme: &Theme, height: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        gap: 6.0,
        padding: spawn_ui::Edges::axis(6.0, 2.0),
        size: Size {
            width: Dimension::Auto,
            height: Dimension::Px(height),
        },
        background: theme.surface_raised,
        ..Default::default()
    }
}

fn side_style(theme: &Theme, width: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        padding: spawn_ui::Edges::all(4.0),
        size: Size {
            width: Dimension::Px(width),
            height: Dimension::Auto,
        },
        background: theme.surface_base,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct ZeroText;
    impl spawn_ui::layout::TextMeasure for ZeroText {
        fn measure(&mut self, _t: &str, _w: Option<f32>) -> Vec2 {
            Vec2::ZERO
        }
    }

    #[test]
    fn four_regions_partition_the_surface_with_viewport_largest() {
        let mut tree = UiTree::new(Style::default());
        let theme = Theme::dark();
        let panels = Panels::build(&mut tree, &theme).unwrap();
        let size = Vec2::new(1280.0, 720.0);
        let mut m = ZeroText;
        tree.compute_layout(size, &mut m).unwrap();

        let toolbar = tree.layout(panels.toolbar).unwrap();
        let outliner = tree.layout(panels.outliner).unwrap();
        let viewport = panels.viewport_rect(&tree, size);
        let inspector = tree.layout(panels.inspector).unwrap();
        let status = tree.layout(panels.status).unwrap();

        // Toolbar on top, status on the bottom, fixed heights.
        assert!((toolbar.height() - TOOLBAR_HEIGHT).abs() < 0.5);
        assert!((status.height() - STATUS_HEIGHT).abs() < 0.5);
        assert!(toolbar.min.y < viewport.min.y);
        assert!(status.min.y > viewport.min.y);
        // Outliner left, inspector right, fixed widths; viewport between them.
        assert!((outliner.width() - OUTLINER_WIDTH).abs() < 0.5);
        assert!((inspector.width() - INSPECTOR_WIDTH).abs() < 0.5);
        assert!(outliner.max.x <= viewport.min.x + 0.5);
        assert!(inspector.min.x >= viewport.max.x - 0.5);
        // The viewport is the largest region by area.
        let area = |r: Rect| r.width() * r.height();
        assert!(area(viewport) > area(outliner));
        assert!(area(viewport) > area(inspector));
    }
}
