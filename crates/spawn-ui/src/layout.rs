//! Layout: `compute_layout`, the [`TextMeasure`] hook, and dirty-driven
//! incremental relayout.

use spawn_core::{Rect, Vec2};

use crate::error::{UiError, UiResult};
use crate::style::{AlignItems, Dimension, Display, FlexDirection, JustifyContent};
use crate::tree::{NodeId, UiTree};

/// Intrinsic text sizing hook — the only text-measurement injection point in
/// Phase 1.
pub trait TextMeasure {
    /// Returns the intrinsic size of `text`. `max_width` is `Some` when the
    /// cross constraint is resolved (so wrapping may apply), else `None`.
    fn measure(&mut self, text: &str, max_width: Option<f32>) -> Vec2;
}

#[derive(Clone, Copy)]
struct Axis2 {
    main: f32,
    cross: f32,
}

fn axis_of(dir: FlexDirection, v: Vec2) -> Axis2 {
    match dir {
        FlexDirection::Row => Axis2 {
            main: v.x,
            cross: v.y,
        },
        FlexDirection::Column => Axis2 {
            main: v.y,
            cross: v.x,
        },
    }
}

fn vec_of(dir: FlexDirection, a: Axis2) -> Vec2 {
    match dir {
        FlexDirection::Row => Vec2::new(a.main, a.cross),
        FlexDirection::Column => Vec2::new(a.cross, a.main),
    }
}

fn dim_main(dir: FlexDirection, size: crate::style::Size) -> Dimension {
    match dir {
        FlexDirection::Row => size.width,
        FlexDirection::Column => size.height,
    }
}

fn dim_cross(dir: FlexDirection, size: crate::style::Size) -> Dimension {
    match dir {
        FlexDirection::Row => size.height,
        FlexDirection::Column => size.width,
    }
}

/// Resolves a `min`/`max` `Dimension` against `parent` (which may be `None` when
/// indefinite). `min` defaults to `0`, `max` to `+inf`.
fn resolve_bound(dim: Dimension, parent: Option<f32>, default: f32) -> f32 {
    match dim {
        Dimension::Auto => default,
        Dimension::Px(v) => v,
        Dimension::Percent(p) => parent.map(|pp| pp * p).unwrap_or(default),
    }
}

/// Applies `[min, max]` clamping with `min` winning ties (clamp to min last).
fn clamp_bounds(value: f32, min: f32, max: f32) -> f32 {
    value.min(max).max(min)
}

impl UiTree {
    /// Lays out every dirty subtree against `root_size`, caches each node's
    /// root-space border-box rect, and clears dirty flags.
    ///
    /// A `root_size` differing from the previous call relays the whole tree.
    /// Components `< 0` or non-finite yield `Err(InvalidArgument)`.
    pub fn compute_layout(
        &mut self,
        root_size: Vec2,
        measure: &mut dyn TextMeasure,
    ) -> UiResult<()> {
        if !root_size.is_finite() || root_size.x < 0.0 || root_size.y < 0.0 {
            return Err(UiError::InvalidArgument {
                context: "root_size must be finite and non-negative",
            });
        }
        if self.last_root_size != Some(root_size) {
            let root = self.root();
            self.mark_dirty(root);
        }
        #[cfg(test)]
        {
            self.layout_visits = 0;
        }
        if !self.layout_dirty {
            // Steady state: clean tree, same root_size, cached rects valid.
            return Ok(());
        }
        let root = self.root();
        let outer = Rect::new(Vec2::ZERO, root_size);
        self.layout_node(root, outer, root_size, measure);
        self.clear_dirty(root);
        self.last_root_size = Some(root_size);
        self.layout_dirty = false;
        Ok(())
    }

    /// Returns the cached root-space border-box rect; `None` if stale,
    /// `Display::None`, or before the first `compute_layout`.
    pub fn layout(&self, node: NodeId) -> Option<Rect> {
        self.resolve(node).and_then(|n| n.rect)
    }

    /// Clears dirty flags over the dirty region only. A clean node already has a
    /// clean subtree (a dirty mark always propagates fully downward, §6), so the
    /// walk stops there — keeping the clear pass O(dirty), not O(total).
    fn clear_dirty(&mut self, node: NodeId) {
        let children = match self.resolve_mut(node) {
            Some(n) if n.dirty => {
                n.dirty = false;
                n.children.clone()
            }
            _ => return,
        };
        for c in children {
            self.clear_dirty(c);
        }
    }

    /// Bottom-up intrinsic *content* size (padding included) of `node` along its
    /// parent's relevant axes. `Percent` self-sizes contribute `0` here.
    ///
    /// A clean node (no dirty mark since the last layout) returns its cached
    /// intrinsic without descending — any change to this node, its style/text,
    /// or any descendant would have propagated a dirty mark up to it (§6), so a
    /// surviving clean cache is guaranteed current.
    fn measure_intrinsic(&mut self, node: NodeId, measure: &mut dyn TextMeasure) -> Vec2 {
        let (style, children, has_text, dirty, cached) = match self.resolve(node) {
            Some(n) => (
                n.style,
                n.children.clone(),
                n.text.is_some(),
                n.dirty,
                n.intrinsic,
            ),
            None => return Vec2::ZERO,
        };
        if let (false, Some(size)) = (dirty, cached) {
            return size;
        }
        if style.display == Display::None {
            return Vec2::ZERO;
        }

        let explicit_w = match style.size.width {
            Dimension::Px(v) => Some(v),
            _ => None,
        };
        let explicit_h = match style.size.height {
            Dimension::Px(v) => Some(v),
            _ => None,
        };

        let mut content = Vec2::ZERO;

        if has_text {
            let text_size = match self.resolve(node).and_then(|n| n.text.clone()) {
                Some(t) => measure.measure(&t, None),
                None => Vec2::ZERO,
            };
            content = content.max(text_size);
        }

        if !children.is_empty() {
            let dir = style.flex_direction;
            let mut main = 0.0_f32;
            let mut cross = 0.0_f32;
            let mut count = 0u32;
            for c in &children {
                if !self.is_displayed(*c) {
                    continue;
                }
                let cs = self.measure_intrinsic(*c, measure);
                let cstyle = match self.resolve(*c) {
                    Some(n) => n.style,
                    None => continue,
                };
                let ca = axis_of(dir, cs);
                let margin_main = match dir {
                    FlexDirection::Row => cstyle.margin.horizontal(),
                    FlexDirection::Column => cstyle.margin.vertical(),
                };
                let margin_cross = match dir {
                    FlexDirection::Row => cstyle.margin.vertical(),
                    FlexDirection::Column => cstyle.margin.horizontal(),
                };
                main += ca.main + margin_main;
                cross = cross.max(ca.cross + margin_cross);
                count += 1;
            }
            if count > 1 {
                main += style.gap * (count - 1) as f32;
            }
            let children_box = vec_of(dir, Axis2 { main, cross });
            content = content.max(children_box);
        }

        // Padding adds to content extents on both axes.
        let padded = content + Vec2::new(style.padding.horizontal(), style.padding.vertical());

        let intrinsic = Vec2::new(
            explicit_w.unwrap_or(padded.x),
            explicit_h.unwrap_or(padded.y),
        );
        if let Some(n) = self.resolve_mut(node) {
            n.intrinsic = Some(intrinsic);
        }
        intrinsic
    }

    /// Top-down arrangement. `border_box` is this node's resolved root-space
    /// border box; `parent_content` is the parent's content size used for
    /// `Percent` resolution of descendants.
    fn layout_node(
        &mut self,
        node: NodeId,
        border_box: Rect,
        _parent_content: Vec2,
        measure: &mut dyn TextMeasure,
    ) {
        #[cfg(test)]
        {
            self.layout_visits += 1;
        }

        let (style, children) = match self.resolve(node) {
            Some(n) => (n.style, n.children.clone()),
            None => return,
        };
        if style.display == Display::None {
            if let Some(n) = self.resolve_mut(node) {
                n.rect = None;
                n.content_rect = None;
            }
            return;
        }

        let content_min = border_box.min + Vec2::new(style.padding.left, style.padding.top);
        let content_max = border_box.max - Vec2::new(style.padding.right, style.padding.bottom);
        let content_rect = Rect::new(content_min, content_max.max(content_min));
        let content_size = content_rect.size();

        if let Some(n) = self.resolve_mut(node) {
            n.rect = Some(border_box);
            n.content_rect = Some(content_rect);
        }

        let in_flow: Vec<NodeId> = children
            .iter()
            .copied()
            .filter(|c| self.is_displayed(*c))
            .collect();
        if in_flow.is_empty() {
            return;
        }

        let dir = style.flex_direction;
        let avail = axis_of(dir, content_size);

        struct Item {
            id: NodeId,
            basis: f32,
            grow: f32,
            shrink: f32,
            min_main: f32,
            max_main: f32,
            margin_main: f32,
            margin_main_lead: f32,
            cross_size: f32,
            margin_cross_lead: f32,
            margin_cross: f32,
        }

        let mut items: Vec<Item> = Vec::with_capacity(in_flow.len());
        for c in &in_flow {
            let cs = match self.resolve(*c) {
                Some(n) => n.style,
                None => continue,
            };
            let intrinsic = self.measure_intrinsic(*c, measure);
            let intrinsic_axis = axis_of(dir, intrinsic);

            let main_dim = dim_main(dir, cs.size);
            let basis_dim = match cs.flex_basis {
                Dimension::Auto => main_dim,
                other => other,
            };
            let basis = match basis_dim {
                Dimension::Auto => intrinsic_axis.main,
                Dimension::Px(v) => v,
                Dimension::Percent(p) => avail.main * p,
            };

            let min_main = resolve_bound(dim_main(dir, cs.min_size), Some(avail.main), 0.0);
            let max_main =
                resolve_bound(dim_main(dir, cs.max_size), Some(avail.main), f32::INFINITY);

            let margin_main = match dir {
                FlexDirection::Row => cs.margin.horizontal(),
                FlexDirection::Column => cs.margin.vertical(),
            };
            let margin_main_lead = match dir {
                FlexDirection::Row => cs.margin.left,
                FlexDirection::Column => cs.margin.top,
            };
            let margin_cross_lead = match dir {
                FlexDirection::Row => cs.margin.top,
                FlexDirection::Column => cs.margin.left,
            };
            let margin_cross = match dir {
                FlexDirection::Row => cs.margin.vertical(),
                FlexDirection::Column => cs.margin.horizontal(),
            };

            // Cross sizing.
            let cross_dim = dim_cross(dir, cs.size);
            let min_cross = resolve_bound(dim_cross(dir, cs.min_size), Some(avail.cross), 0.0);
            let max_cross = resolve_bound(
                dim_cross(dir, cs.max_size),
                Some(avail.cross),
                f32::INFINITY,
            );
            let cross_avail = (avail.cross - margin_cross).max(0.0);
            let cross_base = match cross_dim {
                Dimension::Auto => {
                    if style.align_items == AlignItems::Stretch {
                        cross_avail
                    } else {
                        intrinsic_axis.cross
                    }
                }
                Dimension::Px(v) => v,
                Dimension::Percent(p) => avail.cross * p,
            };
            let cross_size = clamp_bounds(cross_base, min_cross, max_cross);

            items.push(Item {
                id: *c,
                basis,
                grow: cs.flex_grow,
                shrink: cs.flex_shrink,
                min_main,
                max_main,
                margin_main,
                margin_main_lead,
                cross_size,
                margin_cross_lead,
                margin_cross,
            });
        }

        let count = items.len();
        let gap_total = if count > 1 {
            style.gap * (count - 1) as f32
        } else {
            0.0
        };
        let basis_total: f32 = items.iter().map(|i| i.basis + i.margin_main).sum();
        let free = avail.main - basis_total - gap_total;

        let mut sizes: Vec<f32> = Vec::with_capacity(count);
        if free > 0.0 {
            let grow_total: f32 = items.iter().map(|i| i.grow).sum();
            for i in &items {
                let extra = if grow_total > 0.0 {
                    free * (i.grow / grow_total)
                } else {
                    0.0
                };
                sizes.push(clamp_bounds(i.basis + extra, i.min_main, i.max_main));
            }
        } else if free < 0.0 {
            let shrink_total: f32 = items.iter().map(|i| i.shrink * i.basis).sum();
            for i in &items {
                let reduce = if shrink_total > 0.0 {
                    -free * (i.shrink * i.basis / shrink_total)
                } else {
                    0.0
                };
                sizes.push(clamp_bounds(i.basis - reduce, i.min_main, i.max_main));
            }
        } else {
            for i in &items {
                sizes.push(clamp_bounds(i.basis, i.min_main, i.max_main));
            }
        }

        let used: f32 = sizes
            .iter()
            .zip(items.iter())
            .map(|(s, i)| s + i.margin_main)
            .sum::<f32>()
            + gap_total;
        let remaining = (avail.main - used).max(0.0);

        let (mut cursor, between) = match style.justify_content {
            JustifyContent::Start => (0.0, 0.0),
            JustifyContent::Center => (remaining * 0.5, 0.0),
            JustifyContent::End => (remaining, 0.0),
            JustifyContent::SpaceBetween => {
                if count > 1 {
                    (0.0, remaining / (count - 1) as f32)
                } else {
                    (0.0, 0.0)
                }
            }
        };

        let arranged: Vec<(NodeId, Rect)> = {
            let mut out = Vec::with_capacity(count);
            for (idx, (item, &main_size)) in items.iter().zip(sizes.iter()).enumerate() {
                let main_start = cursor + item.margin_main_lead;
                let cross_free = (avail.cross - item.cross_size - item.margin_cross).max(0.0);
                let cross_offset = match style.align_items {
                    AlignItems::Start | AlignItems::Stretch => 0.0,
                    AlignItems::Center => cross_free * 0.5,
                    AlignItems::End => cross_free,
                } + item.margin_cross_lead;

                let pos_axis = Axis2 {
                    main: main_start,
                    cross: cross_offset,
                };
                let size_axis = Axis2 {
                    main: main_size,
                    cross: item.cross_size,
                };
                let pos = content_rect.min + vec_of(dir, pos_axis);
                let size = vec_of(dir, size_axis);
                let child_box = Rect::new(pos, pos + size);
                out.push((item.id, child_box));

                cursor += item.margin_main + main_size;
                if idx + 1 < count {
                    cursor += style.gap + between;
                }
            }
            out
        };

        for (id, child_box) in arranged {
            // Skip a clean child whose input box is unchanged: its cached rect
            // and entire subtree remain valid (§6). A dirty child, or one whose
            // resolved border box moved/resized, must be re-arranged so the
            // change ripples through its descendants. Layout is deterministic
            // and unrounded (§4.3), so an unchanged input reproduces a
            // bit-identical box — exact equality is the correct skip predicate.
            let skip = match self.resolve(id) {
                Some(n) => !n.dirty && n.rect == Some(child_box),
                None => true,
            };
            if skip {
                continue;
            }
            self.layout_node(id, child_box, content_size, measure);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{Border, Dimension, Edges, Size, Style};
    use spawn_core::traits::ApproxEq;
    use spawn_core::Color;

    struct ZeroMeasure;
    impl TextMeasure for ZeroMeasure {
        fn measure(&mut self, _text: &str, _max_width: Option<f32>) -> Vec2 {
            Vec2::ZERO
        }
    }

    struct FixedMeasure {
        per_char: Vec2,
        saw_max: std::cell::Cell<Option<Option<f32>>>,
    }
    impl TextMeasure for FixedMeasure {
        fn measure(&mut self, text: &str, max_width: Option<f32>) -> Vec2 {
            self.saw_max.set(Some(max_width));
            Vec2::new(self.per_char.x * text.len() as f32, self.per_char.y)
        }
    }

    fn px(w: f32, h: f32) -> Size {
        Size {
            width: Dimension::Px(w),
            height: Dimension::Px(h),
        }
    }

    fn approx(a: Rect, min: Vec2, max: Vec2) {
        assert!(
            a.min.approx_eq(min, 1e-4) && a.max.approx_eq(max, 1e-4),
            "got {a:?}, want min {min:?} max {max:?}"
        );
    }

    #[test]
    fn row_with_gap_padding_margin() {
        let mut tree = UiTree::new(Style {
            padding: Edges::all(10.0),
            gap: 5.0,
            ..Default::default()
        });
        let root = tree.root();
        let mk = |w: f32| Style {
            size: px(w, 20.0),
            align_items: AlignItems::Start,
            ..Default::default()
        };
        let a = tree.create_node(mk(30.0), root).unwrap();
        let b = tree
            .create_node(
                Style {
                    margin: Edges::all(4.0),
                    ..mk(40.0)
                },
                root,
            )
            .unwrap();
        let c = tree.create_node(mk(50.0), root).unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(300.0, 100.0), &mut m)
            .unwrap();
        // content origin at (10,10).
        approx(
            tree.layout(a).unwrap(),
            Vec2::new(10.0, 10.0),
            Vec2::new(40.0, 30.0),
        );
        // after a: cursor 30 + gap 5 = 35; b lead margin 4 -> x 10+35+4=49.
        approx(
            tree.layout(b).unwrap(),
            Vec2::new(49.0, 14.0),
            Vec2::new(89.0, 34.0),
        );
        // after b: 35 + margin(8) + width(40) + gap(5) = 88; c x = 10+88=98.
        approx(
            tree.layout(c).unwrap(),
            Vec2::new(98.0, 10.0),
            Vec2::new(148.0, 30.0),
        );
    }

    #[test]
    fn column_flow() {
        let mut tree = UiTree::new(Style {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Start,
            gap: 10.0,
            ..Default::default()
        });
        let root = tree.root();
        let s = Style {
            size: px(20.0, 30.0),
            ..Default::default()
        };
        let a = tree.create_node(s, root).unwrap();
        let b = tree.create_node(s, root).unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(100.0, 200.0), &mut m)
            .unwrap();
        approx(
            tree.layout(a).unwrap(),
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 30.0),
        );
        approx(
            tree.layout(b).unwrap(),
            Vec2::new(0.0, 40.0),
            Vec2::new(20.0, 70.0),
        );
    }

    #[test]
    fn grow_distribution_1_2_1() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let mk = |g: f32| Style {
            flex_grow: g,
            flex_basis: Dimension::Px(0.0),
            align_items: AlignItems::Start,
            size: Size {
                height: Dimension::Px(10.0),
                ..Default::default()
            },
            ..Default::default()
        };
        let a = tree.create_node(mk(1.0), root).unwrap();
        let b = tree.create_node(mk(2.0), root).unwrap();
        let c = tree.create_node(mk(1.0), root).unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(400.0, 10.0), &mut m).unwrap();
        approx(
            tree.layout(a).unwrap(),
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 10.0),
        );
        approx(
            tree.layout(b).unwrap(),
            Vec2::new(100.0, 0.0),
            Vec2::new(300.0, 10.0),
        );
        approx(
            tree.layout(c).unwrap(),
            Vec2::new(300.0, 0.0),
            Vec2::new(400.0, 10.0),
        );
    }

    #[test]
    fn shrink_distribution_clamped_at_min() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let mk = |w: f32, min: f32| Style {
            flex_shrink: 1.0,
            flex_basis: Dimension::Px(w),
            min_size: Size {
                width: Dimension::Px(min),
                ..Default::default()
            },
            ..Default::default()
        };
        let a = tree.create_node(mk(100.0, 80.0), root).unwrap();
        let b = tree.create_node(mk(100.0, 0.0), root).unwrap();
        let mut m = ZeroMeasure;
        // free = 100 - 200 = -100; equal basis*shrink -> -50 each.
        // a clamps to 80 (min), b becomes 50 (single-pass, no redistribution).
        tree.compute_layout(Vec2::new(100.0, 10.0), &mut m).unwrap();
        assert!(tree.layout(a).unwrap().width().approx_eq(80.0, 1e-4));
        assert!(tree.layout(b).unwrap().width().approx_eq(50.0, 1e-4));
    }

    #[test]
    fn percent_against_definite_parent() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let child = tree
            .create_node(
                Style {
                    size: Size {
                        width: Dimension::Percent(0.5),
                        height: Dimension::Percent(0.25),
                    },
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(200.0, 80.0), &mut m).unwrap();
        approx(
            tree.layout(child).unwrap(),
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 20.0),
        );
    }

    #[test]
    fn percent_against_indefinite_parent_is_zero() {
        // Root is Auto-sized container (size Auto) with no fixed extent on main.
        let mut tree = UiTree::new(Style {
            size: Size {
                width: Dimension::Auto,
                height: Dimension::Px(50.0),
            },
            ..Default::default()
        });
        let root = tree.root();
        // intermediate Auto container whose main size comes from children.
        let mid = tree
            .create_node(
                Style {
                    size: Size {
                        width: Dimension::Auto,
                        height: Dimension::Px(50.0),
                    },
                    flex_grow: 0.0,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let child = tree
            .create_node(
                Style {
                    flex_basis: Dimension::Percent(0.5),
                    flex_grow: 0.0,
                    align_items: AlignItems::Start,
                    size: Size {
                        height: Dimension::Px(10.0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                mid,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        // mid has Auto main width; its content main = sum of children intrinsic = 0
        // (percent basis contributes 0 bottom-up). So child resolves to 0 width.
        tree.compute_layout(Vec2::new(300.0, 50.0), &mut m).unwrap();
        assert!(tree.layout(child).unwrap().width().approx_eq(0.0, 1e-4));
    }

    #[test]
    fn min_wins_over_max() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let child = tree
            .create_node(
                Style {
                    size: Size {
                        width: Dimension::Px(40.0),
                        height: Dimension::Px(10.0),
                    },
                    min_size: Size {
                        width: Dimension::Px(80.0),
                        ..Default::default()
                    },
                    max_size: Size {
                        width: Dimension::Px(50.0),
                        ..Default::default()
                    },
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(300.0, 10.0), &mut m).unwrap();
        // clamp(40, min 80, max 50): min(50) then max(80) => 80; min wins.
        assert!(tree.layout(child).unwrap().width().approx_eq(80.0, 1e-4));
    }

    #[test]
    fn justify_variants() {
        let make = |jc: JustifyContent| {
            let mut tree = UiTree::new(Style {
                justify_content: jc,
                ..Default::default()
            });
            let root = tree.root();
            let s = Style {
                size: px(20.0, 10.0),
                align_items: AlignItems::Start,
                ..Default::default()
            };
            let a = tree.create_node(s, root).unwrap();
            let b = tree.create_node(s, root).unwrap();
            let mut m = ZeroMeasure;
            tree.compute_layout(Vec2::new(100.0, 10.0), &mut m).unwrap();
            (tree.layout(a).unwrap().min.x, tree.layout(b).unwrap().min.x)
        };
        let (a, b) = make(JustifyContent::Start);
        assert!(a.approx_eq(0.0, 1e-4) && b.approx_eq(20.0, 1e-4));
        let (a, b) = make(JustifyContent::End);
        assert!(a.approx_eq(60.0, 1e-4) && b.approx_eq(80.0, 1e-4));
        let (a, b) = make(JustifyContent::Center);
        assert!(a.approx_eq(30.0, 1e-4) && b.approx_eq(50.0, 1e-4));
        let (a, b) = make(JustifyContent::SpaceBetween);
        assert!(a.approx_eq(0.0, 1e-4) && b.approx_eq(80.0, 1e-4));
    }

    #[test]
    fn space_between_single_child_is_start() {
        let mut tree = UiTree::new(Style {
            justify_content: JustifyContent::SpaceBetween,
            ..Default::default()
        });
        let root = tree.root();
        let a = tree
            .create_node(
                Style {
                    size: px(20.0, 10.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(100.0, 10.0), &mut m).unwrap();
        assert!(tree.layout(a).unwrap().min.x.approx_eq(0.0, 1e-4));
    }

    #[test]
    fn align_items_variants() {
        let make = |ai: AlignItems| {
            let mut tree = UiTree::new(Style {
                align_items: ai,
                ..Default::default()
            });
            let root = tree.root();
            let a = tree
                .create_node(
                    Style {
                        size: Size {
                            width: Dimension::Px(20.0),
                            height: if ai == AlignItems::Stretch {
                                Dimension::Auto
                            } else {
                                Dimension::Px(10.0)
                            },
                        },
                        ..Default::default()
                    },
                    root,
                )
                .unwrap();
            let mut m = ZeroMeasure;
            tree.compute_layout(Vec2::new(100.0, 100.0), &mut m)
                .unwrap();
            tree.layout(a).unwrap()
        };
        approx(
            make(AlignItems::Start),
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 10.0),
        );
        approx(
            make(AlignItems::Center),
            Vec2::new(0.0, 45.0),
            Vec2::new(20.0, 55.0),
        );
        approx(
            make(AlignItems::End),
            Vec2::new(0.0, 90.0),
            Vec2::new(20.0, 100.0),
        );
        approx(
            make(AlignItems::Stretch),
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 100.0),
        );
    }

    #[test]
    fn auto_container_sizing_nested() {
        // Outer column auto-sized from inner row auto-sized from leaves.
        let mut tree = UiTree::new(Style {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Start,
            size: Size {
                width: Dimension::Auto,
                height: Dimension::Auto,
            },
            ..Default::default()
        });
        let root = tree.root();
        let row = tree
            .create_node(
                Style {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Start,
                    gap: 5.0,
                    size: Size {
                        width: Dimension::Auto,
                        height: Dimension::Auto,
                    },
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let _a = tree
            .create_node(
                Style {
                    size: px(30.0, 20.0),
                    ..Default::default()
                },
                row,
            )
            .unwrap();
        let _b = tree
            .create_node(
                Style {
                    size: px(40.0, 25.0),
                    ..Default::default()
                },
                row,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(500.0, 500.0), &mut m)
            .unwrap();
        // row main = 30 + 5 + 40 = 75; cross = max(20,25)=25.
        let rb = tree.layout(row).unwrap();
        assert!(rb.width().approx_eq(75.0, 1e-4));
        assert!(rb.height().approx_eq(25.0, 1e-4));
    }

    #[test]
    fn display_none_skipped() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let hidden = tree
            .create_node(
                Style {
                    display: Display::None,
                    size: px(50.0, 10.0),
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let visible = tree
            .create_node(
                Style {
                    size: px(20.0, 10.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(200.0, 10.0), &mut m).unwrap();
        assert_eq!(tree.layout(hidden), None);
        // visible starts at 0 (hidden contributed no slot/gap).
        approx(
            tree.layout(visible).unwrap(),
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 10.0),
        );
    }

    #[test]
    fn text_auto_size_and_max_width() {
        let mut tree = UiTree::new(Style {
            align_items: AlignItems::Start,
            ..Default::default()
        });
        let root = tree.root();
        let t = tree.create_node(Style::default(), root).unwrap();
        tree.set_text(t, Some("abcd".to_string())).unwrap();
        let mut m = FixedMeasure {
            per_char: Vec2::new(7.0, 12.0),
            saw_max: std::cell::Cell::new(None),
        };
        tree.compute_layout(Vec2::new(300.0, 50.0), &mut m).unwrap();
        let r = tree.layout(t).unwrap();
        assert!(r.width().approx_eq(28.0, 1e-4));
        assert!(r.height().approx_eq(12.0, 1e-4));
        // measure was invoked during the bottom-up pass (indefinite max_width).
        assert_eq!(m.saw_max.get(), Some(None));
    }

    #[test]
    fn invalid_root_size_errors() {
        let mut tree = UiTree::new(Style::default());
        let mut m = ZeroMeasure;
        assert!(tree.compute_layout(Vec2::new(-1.0, 10.0), &mut m).is_err());
        assert!(tree
            .compute_layout(Vec2::new(f32::NAN, 10.0), &mut m)
            .is_err());
    }

    #[test]
    fn incremental_relayout_skips_clean_subtrees() {
        // Tree:
        //   root [Column]
        //   ├── left [Column]      (a wide, clean subtree)
        //   │   ├── l0 .. l3 (leaves)
        //   └── right [Column]
        //       └── target_parent
        //           └── target (deep leaf we restyle)
        let mut tree = UiTree::new(Style {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Start,
            size: px(400.0, 400.0),
            ..Default::default()
        });
        let root = tree.root();
        let col = |w: f32, h: f32| Style {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Start,
            size: px(w, h),
            ..Default::default()
        };
        let left = tree.create_node(col(200.0, 200.0), root).unwrap();
        let mut left_leaves = Vec::new();
        for _ in 0..4 {
            left_leaves.push(
                tree.create_node(
                    Style {
                        size: px(20.0, 20.0),
                        align_items: AlignItems::Start,
                        ..Default::default()
                    },
                    left,
                )
                .unwrap(),
            );
        }
        let right = tree.create_node(col(200.0, 200.0), root).unwrap();
        let target_parent = tree.create_node(col(100.0, 100.0), right).unwrap();
        let target = tree
            .create_node(
                Style {
                    size: px(30.0, 30.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                target_parent,
            )
            .unwrap();

        let mut m = ZeroMeasure;
        let root_size = Vec2::new(400.0, 400.0);
        tree.compute_layout(root_size, &mut m).unwrap();
        // Warm pass arranges every node once: root, left, 4 leaves, right,
        // target_parent, target = 9.
        assert_eq!(tree.layout_visits, 9);

        // Restyle a deep leaf without changing its resolved size (Px(30,30)
        // stays Px(30,30)): its box is unchanged, so only the dirty chain is
        // re-arranged and no extra nodes ripple.
        tree.set_style(
            target,
            Style {
                size: px(30.0, 30.0),
                align_items: AlignItems::Start,
                background: Color::RED,
                ..Default::default()
            },
        )
        .unwrap();
        tree.compute_layout(root_size, &mut m).unwrap();
        // Dirty chain only: root -> right -> target_parent -> target = 4.
        // The whole `left` subtree (6 nodes) is skipped.
        assert_eq!(tree.layout_visits, 4);

        // Correctness oracle: a from-scratch full relayout of an identical tree
        // yields the same rects.
        let mut fresh = UiTree::new(Style {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Start,
            size: px(400.0, 400.0),
            ..Default::default()
        });
        let froot = fresh.root();
        let fleft = fresh.create_node(col(200.0, 200.0), froot).unwrap();
        let mut fleft_leaves = Vec::new();
        for _ in 0..4 {
            fleft_leaves.push(
                fresh
                    .create_node(
                        Style {
                            size: px(20.0, 20.0),
                            align_items: AlignItems::Start,
                            ..Default::default()
                        },
                        fleft,
                    )
                    .unwrap(),
            );
        }
        let fright = fresh.create_node(col(200.0, 200.0), froot).unwrap();
        let ftarget_parent = fresh.create_node(col(100.0, 100.0), fright).unwrap();
        let ftarget = fresh
            .create_node(
                Style {
                    size: px(30.0, 30.0),
                    align_items: AlignItems::Start,
                    background: Color::RED,
                    ..Default::default()
                },
                ftarget_parent,
            )
            .unwrap();
        fresh.compute_layout(root_size, &mut m).unwrap();

        let pairs = [
            (root, froot),
            (left, fleft),
            (right, fright),
            (target_parent, ftarget_parent),
            (target, ftarget),
        ];
        for (a, b) in pairs {
            let ra = tree.layout(a).unwrap();
            let rb = fresh.layout(b).unwrap();
            assert!(
                ra.min.approx_eq(rb.min, 1e-4) && ra.max.approx_eq(rb.max, 1e-4),
                "node mismatch: incremental {ra:?} vs full {rb:?}"
            );
        }
        for (a, b) in left_leaves.iter().zip(fleft_leaves.iter()) {
            let ra = tree.layout(*a).unwrap();
            let rb = fresh.layout(*b).unwrap();
            assert!(ra.min.approx_eq(rb.min, 1e-4) && ra.max.approx_eq(rb.max, 1e-4));
        }
    }

    #[test]
    fn steady_state_does_no_layout_work() {
        let mut tree = UiTree::new(Style {
            align_items: AlignItems::Start,
            size: px(100.0, 100.0),
            ..Default::default()
        });
        let root = tree.root();
        tree.create_node(
            Style {
                size: px(10.0, 10.0),
                align_items: AlignItems::Start,
                ..Default::default()
            },
            root,
        )
        .unwrap();
        let mut m = ZeroMeasure;
        let size = Vec2::new(100.0, 100.0);
        tree.compute_layout(size, &mut m).unwrap();
        // No mutation, same root_size: the short-circuit returns before any
        // arrangement.
        tree.compute_layout(size, &mut m).unwrap();
        assert_eq!(tree.layout_visits, 0);
    }

    #[test]
    fn border_does_not_affect_layout() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let a = tree
            .create_node(
                Style {
                    size: px(20.0, 10.0),
                    align_items: AlignItems::Start,
                    border: Border {
                        width: 4.0,
                        color: Color::RED,
                    },
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(200.0, 10.0), &mut m).unwrap();
        approx(
            tree.layout(a).unwrap(),
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 10.0),
        );
    }
}
