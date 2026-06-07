//! Renderer-agnostic draw-list generation with z-ordering and scissor clipping.

use spawn_core::{Color, Rect};

use crate::error::{UiError, UiResult};
use crate::style::Display;
use crate::tree::{NodeId, UiTree};

/// Opaque texture handle; spawn-ui never interprets it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureId(pub u64);

/// Opaque font handle; spawn-ui never interprets it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontId(pub u64);

/// Image content attached to a node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiImage {
    pub texture: TextureId,
    pub uv: Rect,
    pub tint: Color,
}

/// A single renderer-agnostic draw command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawCommand {
    Rect {
        rect: Rect,
        color: Color,
        corner_radius: f32,
    },
    Border {
        rect: Rect,
        width: f32,
        color: Color,
        corner_radius: f32,
    },
    Image {
        rect: Rect,
        texture: TextureId,
        uv: Rect,
        tint: Color,
    },
    /// A layout box only in Phase 1: it carries the node's measured rect and
    /// font; no glyph runs are emitted. `text_node` lets the renderer fetch the
    /// string.
    Text {
        rect: Rect,
        font: FontId,
        text_node: NodeId,
    },
    ScissorPush(Rect),
    ScissorPop,
}

/// Reusable command buffer. Capacity is retained across `clear` so steady-state
/// rebuilds allocate nothing.
#[derive(Debug, Default)]
pub struct DrawList {
    commands: Vec<DrawCommand>,
}

impl DrawList {
    /// Clears the commands while retaining capacity.
    pub fn clear(&mut self) {
        self.commands.clear();
    }

    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl UiTree {
    /// Clears `out` and appends commands in pre-order tree z-order. The reused
    /// buffer keeps capacity, so a steady-state rebuild allocates nothing.
    /// `Err(InvalidState)` if layout is uncomputed/dirty.
    pub fn build_draw_list(&self, out: &mut DrawList) -> UiResult<()> {
        if self.layout_dirty {
            return Err(UiError::InvalidState {
                context: "build_draw_list requires up-to-date layout",
            });
        }
        out.clear();
        let root = self.root();
        self.emit_node(root, out);
        Ok(())
    }

    fn emit_node(&self, node: NodeId, out: &mut DrawList) {
        let n = match self.resolve(node) {
            Some(n) => n,
            None => return,
        };
        if n.style.display == Display::None {
            return;
        }
        let rect = match n.rect {
            Some(r) => r,
            None => return,
        };
        let style = n.style;

        if style.background.a > 0.0 {
            out.commands.push(DrawCommand::Rect {
                rect,
                color: style.background,
                corner_radius: style.corner_radius,
            });
        }
        if let Some(img) = n.image {
            out.commands.push(DrawCommand::Image {
                rect,
                texture: img.texture,
                uv: img.uv,
                tint: img.tint,
            });
        }
        if style.border.width > 0.0 {
            out.commands.push(DrawCommand::Border {
                rect,
                width: style.border.width,
                color: style.border.color,
                corner_radius: style.corner_radius,
            });
        }
        if n.text.is_some() {
            let font = n.font.unwrap_or(FontId(0));
            out.commands.push(DrawCommand::Text {
                rect,
                font,
                text_node: node,
            });
        }

        let clip = style.overflow_clip;
        if clip {
            let cr = n.content_rect.unwrap_or(rect);
            out.commands.push(DrawCommand::ScissorPush(cr));
        }
        let child_count = n.children.len();
        for i in 0..child_count {
            // Re-resolve each iteration so no borrow is held across recursion;
            // child ids are `Copy`, so this reads without allocating.
            let child = match self.resolve(node) {
                Some(node_ref) => node_ref.children[i],
                None => break,
            };
            self.emit_node(child, out);
        }
        if clip {
            out.commands.push(DrawCommand::ScissorPop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TextMeasure;
    use crate::style::{AlignItems, Border, Dimension, Size, Style};
    use spawn_core::Vec2;

    struct ZeroMeasure;
    impl TextMeasure for ZeroMeasure {
        fn measure(&mut self, _t: &str, _w: Option<f32>) -> Vec2 {
            Vec2::ZERO
        }
    }

    fn px(w: f32, h: f32) -> Size {
        Size {
            width: Dimension::Px(w),
            height: Dimension::Px(h),
        }
    }

    #[test]
    fn requires_layout() {
        let tree = UiTree::new(Style::default());
        let mut dl = DrawList::default();
        assert!(tree.build_draw_list(&mut dl).is_err());
    }

    #[test]
    fn z_order_is_preorder() {
        let mut tree = UiTree::new(Style {
            background: Color::WHITE,
            size: px(100.0, 100.0),
            ..Default::default()
        });
        let root = tree.root();
        let a = tree
            .create_node(
                Style {
                    background: Color::RED,
                    size: px(20.0, 20.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let b = tree
            .create_node(
                Style {
                    background: Color::BLUE,
                    size: px(20.0, 20.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let _ = (a, b);
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(100.0, 100.0), &mut m)
            .unwrap();
        let mut dl = DrawList::default();
        tree.build_draw_list(&mut dl).unwrap();
        let colors: Vec<Color> = dl
            .commands()
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Rect { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        assert_eq!(colors, vec![Color::WHITE, Color::RED, Color::BLUE]);
    }

    #[test]
    fn per_node_command_ordering() {
        let mut tree = UiTree::new(Style {
            background: Color::WHITE,
            border: Border {
                width: 2.0,
                color: Color::BLACK,
            },
            size: px(50.0, 50.0),
            ..Default::default()
        });
        let root = tree.root();
        tree.set_image(
            root,
            Some(UiImage {
                texture: TextureId(7),
                uv: Rect::new(Vec2::ZERO, Vec2::ONE),
                tint: Color::WHITE,
            }),
        )
        .unwrap();
        tree.set_text(root, Some("x".to_string())).unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(50.0, 50.0), &mut m).unwrap();
        let mut dl = DrawList::default();
        tree.build_draw_list(&mut dl).unwrap();
        let kinds: Vec<u8> = dl
            .commands()
            .iter()
            .map(|c| match c {
                DrawCommand::Rect { .. } => 0,
                DrawCommand::Image { .. } => 1,
                DrawCommand::Border { .. } => 2,
                DrawCommand::Text { .. } => 3,
                DrawCommand::ScissorPush(_) => 4,
                DrawCommand::ScissorPop => 5,
            })
            .collect();
        assert_eq!(kinds, vec![0, 1, 2, 3]);
    }

    #[test]
    fn transparent_and_zero_border_emit_nothing() {
        let mut tree = UiTree::new(Style {
            background: Color::TRANSPARENT,
            border: Border {
                width: 0.0,
                color: Color::BLACK,
            },
            size: px(20.0, 20.0),
            ..Default::default()
        });
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(20.0, 20.0), &mut m).unwrap();
        let mut dl = DrawList::default();
        tree.build_draw_list(&mut dl).unwrap();
        assert!(dl.is_empty());
    }

    #[test]
    fn scissor_balanced_and_nested() {
        let mut tree = UiTree::new(Style {
            size: px(100.0, 100.0),
            ..Default::default()
        });
        let root = tree.root();
        let outer = tree
            .create_node(
                Style {
                    overflow_clip: true,
                    size: px(80.0, 80.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let _inner = tree
            .create_node(
                Style {
                    overflow_clip: true,
                    size: px(40.0, 40.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                outer,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(100.0, 100.0), &mut m)
            .unwrap();
        let mut dl = DrawList::default();
        tree.build_draw_list(&mut dl).unwrap();
        let mut depth = 0i32;
        let mut max_depth = 0i32;
        for c in dl.commands() {
            match c {
                DrawCommand::ScissorPush(_) => {
                    depth += 1;
                    max_depth = max_depth.max(depth);
                }
                DrawCommand::ScissorPop => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0);
        }
        assert_eq!(depth, 0);
        assert_eq!(max_depth, 2);
    }
}
