//! The 2D overlay pass: rasterizes a `spawn_ui` draw list (rects, borders, text,
//! scissors) plus editor line geometry (gizmo handles, selection outlines) onto
//! the surface after the lit scene. No depth; alpha-blended; loads (composites
//! over) the existing color.
//!
//! UI positions are converted to clip space (NDC) on the CPU from the surface
//! size, so the UI pipeline needs no screen uniform — only a texture group (the
//! 1×1 white texture for solid rects/borders, or a font atlas for text). Lines
//! are world-space and projected by the scene camera. Geometry is built into
//! reused buffers (cleared, not freed), so a steady overlay allocates nothing.
//!
//! `DrawCommand::Image` is not rendered yet (a documented follow-on); the editor
//! MVP draws panels, labels, numeric values, and gizmos.

use std::collections::HashMap;
use std::ops::Range;

use spawn_core::{Color, Vec3};
use spawn_ui::{DrawCommand, DrawList, FontId, UiTree};

use crate::asset_handle::ShaderHandle;
use crate::error::RenderResult;
use crate::format::{CompareFn, CullMode, DepthFormat, SurfaceSize, TextureFormat, Topology};
use crate::mesh::{LineVertex, UiVertex};
use crate::pipeline::{
    BindGroupLayouts, PipelineCache, PipelineKey, RenderStateKey, ShaderStore, VertexLayoutId,
};
use crate::renderer::Renderer;
use crate::shaders::{OVERLAY_LINE_WGSL, OVERLAY_UI_WGSL};
use crate::text::Font;
use crate::texture::Texture;

use spawn_asset::AssetId;

const BUILTIN_OVERLAY_UI_SHADER_ID: u64 = u64::MAX - 2;
const BUILTIN_OVERLAY_LINE_SHADER_ID: u64 = u64::MAX - 3;
const INITIAL_UI_VERTS: u32 = 1024;
const INITIAL_LINE_VERTS: u32 = 256;
/// Text has no color in the Phase 1 `spawn_ui` draw list, so the overlay renders
/// glyphs in white (editor panels are dark). Per-glyph color is a follow-on when
/// `spawn_ui` adds a text color.
const TEXT_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// A world-space line segment with a color, projected by the scene camera. Gizmo
/// handles and selection outlines are emitted as these.
#[derive(Debug, Clone, Copy)]
pub struct LineSegment {
    pub start: Vec3,
    pub end: Vec3,
    pub color: Color,
}

/// Maps `spawn_ui::FontId`s to their [`Font`] and a pre-built overlay texture
/// bind group (group 0 of the UI pipeline). Bind groups are created at
/// registration, never per frame.
pub struct FontRegistry {
    fonts: HashMap<u64, (Font, wgpu::BindGroup)>,
}

impl Default for FontRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FontRegistry {
    pub fn new() -> Self {
        Self {
            fonts: HashMap::new(),
        }
    }

    /// Registers `font` under `id`, building its atlas bind group once. Overwrites
    /// any prior registration for `id`.
    pub fn insert(&mut self, renderer: &Renderer, id: FontId, font: Font) {
        let bind_group = renderer.create_overlay_texture_bind_group(font.atlas());
        self.fonts.insert(id.0, (font, bind_group));
    }

    /// The font registered under `id`, if any.
    pub fn font(&self, id: FontId) -> Option<&Font> {
        self.fonts.get(&id.0).map(|(f, _)| f)
    }

    fn resolve(&self, id: FontId) -> Option<&(Font, wgpu::BindGroup)> {
        self.fonts.get(&id.0)
    }

    /// Measures `text` in the font registered under `id`, or `None` if `id` is
    /// unregistered. Convenience for callers that route layout text metrics
    /// through a registry rather than a single [`Font`].
    pub fn measure(&self, id: FontId, text: &str) -> Option<spawn_core::Vec2> {
        self.font(id).map(|f| f.measure(text))
    }
}

/// What a UI batch samples: the 1×1 white texture (solid rects/borders) or a
/// font atlas (text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchTexture {
    White,
    Font(u64),
}

/// A contiguous run of UI vertices sharing a texture and scissor.
struct UiBatch {
    texture: BatchTexture,
    scissor: Option<[u32; 4]>,
    range: Range<u32>,
}

/// Renderer-owned overlay GPU state and reused CPU geometry. Built once at
/// renderer construction; the buffers grow on demand (never per frame) and the
/// CPU vectors are cleared-not-freed each frame.
pub(crate) struct OverlayState {
    ui_key: PipelineKey,
    line_key: PipelineKey,
    white_bind_group: wgpu::BindGroup,
    ui_buffer: wgpu::Buffer,
    ui_capacity: u32,
    line_buffer: wgpu::Buffer,
    line_capacity: u32,
    quads: Vec<UiVertex>,
    lines: Vec<LineVertex>,
    batches: Vec<UiBatch>,
    scissor_stack: Vec<[u32; 4]>,
}

impl OverlayState {
    /// Builds the overlay pipelines (UI quad + line) into the cache, the white
    /// texture bind group, and the initial vertex buffers. Pipelines are built
    /// once here, never per frame.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        device: &wgpu::Device,
        layouts: &BindGroupLayouts,
        cache: &mut PipelineCache,
        shaders: &mut ShaderStore,
        surface_format: TextureFormat,
        depth_format: DepthFormat,
        white_texture: &Texture,
    ) -> RenderResult<Self> {
        let ui_shader = ShaderHandle::from_id(AssetId::from_raw(BUILTIN_OVERLAY_UI_SHADER_ID));
        let line_shader = ShaderHandle::from_id(AssetId::from_raw(BUILTIN_OVERLAY_LINE_SHADER_ID));
        shaders.load(device, ui_shader, OVERLAY_UI_WGSL)?;
        shaders.load(device, line_shader, OVERLAY_LINE_WGSL)?;
        // Depth fields are part of the cache key but unused by the overlay
        // (Overlay2D builds no depth-stencil state); pick stable placeholders.
        let ui_key = PipelineKey {
            shader: ui_shader,
            vertex_layout: VertexLayoutId::UiQuad,
            render_state: RenderStateKey {
                color_format: surface_format,
                depth_format,
                depth_compare: CompareFn::Always,
                depth_write: false,
                cull: CullMode::None,
                topology: Topology::TriangleList,
            },
            pass: PassKind::Overlay2D,
            instanced: false,
        };
        let line_key = PipelineKey {
            shader: line_shader,
            vertex_layout: VertexLayoutId::OverlayLine,
            render_state: RenderStateKey {
                color_format: surface_format,
                depth_format,
                depth_compare: CompareFn::Always,
                depth_write: false,
                cull: CullMode::None,
                topology: Topology::LineList,
            },
            pass: PassKind::Overlay2D,
            instanced: false,
        };
        cache.get_or_create(device, layouts, ui_key, shaders)?;
        cache.get_or_create(device, layouts, line_key, shaders)?;

        let white_bind_group =
            make_texture_bind_group(device, &layouts.overlay_texture, white_texture);
        let ui_buffer = make_vertex_buffer(
            device,
            "spawn-overlay-ui",
            INITIAL_UI_VERTS as u64 * std::mem::size_of::<UiVertex>() as u64,
        );
        let line_buffer = make_vertex_buffer(
            device,
            "spawn-overlay-line",
            INITIAL_LINE_VERTS as u64 * std::mem::size_of::<LineVertex>() as u64,
        );

        Ok(Self {
            ui_key,
            line_key,
            white_bind_group,
            ui_buffer,
            ui_capacity: INITIAL_UI_VERTS,
            line_buffer,
            line_capacity: INITIAL_LINE_VERTS,
            quads: Vec::new(),
            lines: Vec::new(),
            batches: Vec::new(),
            scissor_stack: Vec::new(),
        })
    }

    fn ensure_ui_capacity(&mut self, device: &wgpu::Device, count: u32) {
        if count <= self.ui_capacity {
            return;
        }
        let new_capacity = count.next_power_of_two().max(INITIAL_UI_VERTS);
        self.ui_buffer = make_vertex_buffer(
            device,
            "spawn-overlay-ui",
            new_capacity as u64 * std::mem::size_of::<UiVertex>() as u64,
        );
        self.ui_capacity = new_capacity;
    }

    fn ensure_line_capacity(&mut self, device: &wgpu::Device, count: u32) {
        if count <= self.line_capacity {
            return;
        }
        let new_capacity = count.next_power_of_two().max(INITIAL_LINE_VERTS);
        self.line_buffer = make_vertex_buffer(
            device,
            "spawn-overlay-line",
            new_capacity as u64 * std::mem::size_of::<LineVertex>() as u64,
        );
        self.line_capacity = new_capacity;
    }
}

fn make_vertex_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub(crate) fn make_texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &Texture,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("spawn-overlay-texture-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(texture.view()),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(texture.sampler()),
            },
        ],
    })
}

use crate::graph::PassKind;

/// The overlay scene: the UI tree (resolves text strings), its draw list, the
/// font registry, and the editor line geometry.
pub struct Overlay<'a> {
    pub tree: &'a UiTree,
    pub draw_list: &'a DrawList,
    pub fonts: &'a FontRegistry,
    pub lines: &'a [LineSegment],
}

/// Records the overlay pass: builds UI quad + line geometry into the renderer's
/// reused buffers, uploads, and draws lines then UI batches over `color_view`. No
/// depth attachment. `camera_offset` is this pass's camera slot (the scene
/// camera), used to project the world-space lines.
pub(crate) fn record(
    renderer: &mut Renderer,
    encoder: &mut wgpu::CommandEncoder,
    color_view: &wgpu::TextureView,
    overlay: &Overlay,
    camera_offset: u32,
) -> RenderResult<()> {
    let size = renderer.size();
    // Take the reused CPU buffers out so the rest of the function does not hold a
    // borrow of `renderer.overlay`; they are returned (capacity retained) at the
    // end. mem::take leaves empty Vecs (no allocation).
    let mut quads = std::mem::take(&mut renderer.overlay.quads);
    let mut lines = std::mem::take(&mut renderer.overlay.lines);
    let mut batches = std::mem::take(&mut renderer.overlay.batches);
    let mut scissor_stack = std::mem::take(&mut renderer.overlay.scissor_stack);
    quads.clear();
    lines.clear();
    batches.clear();
    scissor_stack.clear();

    build_ui(overlay, size, &mut quads, &mut batches, &mut scissor_stack);
    for seg in overlay.lines {
        lines.push(LineVertex {
            position: [seg.start.x, seg.start.y, seg.start.z],
            color: [seg.color.r, seg.color.g, seg.color.b, seg.color.a],
        });
        lines.push(LineVertex {
            position: [seg.end.x, seg.end.y, seg.end.z],
            color: [seg.color.r, seg.color.g, seg.color.b, seg.color.a],
        });
    }

    renderer
        .overlay
        .ensure_ui_capacity(&renderer.device, quads.len() as u32);
    renderer
        .overlay
        .ensure_line_capacity(&renderer.device, lines.len() as u32);
    if !quads.is_empty() {
        renderer
            .queue
            .write_buffer(&renderer.overlay.ui_buffer, 0, bytemuck::cast_slice(&quads));
    }
    if !lines.is_empty() {
        renderer.queue.write_buffer(
            &renderer.overlay.line_buffer,
            0,
            bytemuck::cast_slice(&lines),
        );
    }

    {
        let cache = &renderer.cache;
        let overlay_state = &renderer.overlay;
        let camera_bind_group = &renderer.camera_bind_group;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("spawn-overlay-2d"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Composite over the lit frame; never clears.
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        if !lines.is_empty() {
            let pipeline = cache.get(&overlay_state.line_key)?;
            pass.set_pipeline(pipeline);
            // The camera group carries a model dynamic binding the line shader does
            // not use; offset 0 is a valid slot.
            pass.set_bind_group(0, camera_bind_group, &[camera_offset, 0]);
            pass.set_vertex_buffer(0, overlay_state.line_buffer.slice(..));
            pass.draw(0..lines.len() as u32, 0..1);
        }

        if !batches.is_empty() {
            let pipeline = cache.get(&overlay_state.ui_key)?;
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, overlay_state.ui_buffer.slice(..));
            for batch in &batches {
                let bind_group = match batch.texture {
                    BatchTexture::White => &overlay_state.white_bind_group,
                    BatchTexture::Font(id) => match overlay.fonts.resolve(FontId(id)) {
                        Some((_, bg)) => bg,
                        None => continue,
                    },
                };
                if let Some([x, y, w, h]) = batch.scissor {
                    pass.set_scissor_rect(x, y, w, h);
                } else {
                    pass.set_scissor_rect(0, 0, size.width, size.height);
                }
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(batch.range.clone(), 0..1);
            }
        }
    }

    renderer.overlay.quads = quads;
    renderer.overlay.lines = lines;
    renderer.overlay.batches = batches;
    renderer.overlay.scissor_stack = scissor_stack;
    Ok(())
}

/// Walks the draw list, appending UI quads and batches. Maintains a scissor stack
/// (each push intersects the current clip).
fn build_ui(
    overlay: &Overlay,
    size: SurfaceSize,
    quads: &mut Vec<UiVertex>,
    batches: &mut Vec<UiBatch>,
    scissor_stack: &mut Vec<[u32; 4]>,
) {
    let sw = size.width.max(1) as f32;
    let sh = size.height.max(1) as f32;
    scissor_stack.clear();
    for command in overlay.draw_list.commands() {
        let scissor = scissor_stack.last().copied();
        match command {
            DrawCommand::Rect { rect, color, .. } => {
                let start = quads.len() as u32;
                push_rect(quads, *rect, *color, [0.0, 0.0], [1.0, 1.0], sw, sh);
                extend_batch(
                    batches,
                    BatchTexture::White,
                    scissor,
                    start,
                    quads.len() as u32,
                );
            }
            DrawCommand::Border {
                rect, width, color, ..
            } => {
                let start = quads.len() as u32;
                push_border(quads, *rect, *width, *color, sw, sh);
                extend_batch(
                    batches,
                    BatchTexture::White,
                    scissor,
                    start,
                    quads.len() as u32,
                );
            }
            DrawCommand::Text {
                rect,
                font,
                text_node,
            } => {
                let Some((font_obj, _)) = overlay.fonts.resolve(*font) else {
                    continue;
                };
                let Some(text) = overlay.tree.text(*text_node) else {
                    continue;
                };
                let start = quads.len() as u32;
                push_text(quads, font_obj, text, *rect, sw, sh);
                extend_batch(
                    batches,
                    BatchTexture::Font(font.0),
                    scissor,
                    start,
                    quads.len() as u32,
                );
            }
            DrawCommand::ScissorPush(rect) => {
                let pushed = rect_to_scissor(*rect, size);
                let clipped = match scissor_stack.last() {
                    Some(top) => intersect_scissor(*top, pushed),
                    None => pushed,
                };
                scissor_stack.push(clipped);
            }
            DrawCommand::ScissorPop => {
                scissor_stack.pop();
            }
            // Image rendering is a documented follow-on (the MVP draws
            // rects/borders/text); skip it without affecting the frame.
            DrawCommand::Image { .. } => {}
        }
    }
}

fn extend_batch(
    batches: &mut Vec<UiBatch>,
    texture: BatchTexture,
    scissor: Option<[u32; 4]>,
    start: u32,
    end: u32,
) {
    if end == start {
        return;
    }
    if let Some(last) = batches.last_mut() {
        if last.texture == texture && last.scissor == scissor && last.range.end == start {
            last.range.end = end;
            return;
        }
    }
    batches.push(UiBatch {
        texture,
        scissor,
        range: start..end,
    });
}

fn ndc(x: f32, y: f32, sw: f32, sh: f32) -> [f32; 2] {
    [x / sw * 2.0 - 1.0, 1.0 - y / sh * 2.0]
}

#[allow(clippy::too_many_arguments)]
fn push_quad(
    quads: &mut Vec<UiVertex>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    color: [f32; 4],
    sw: f32,
    sh: f32,
) {
    let p00 = ndc(x0, y0, sw, sh);
    let p10 = ndc(x1, y0, sw, sh);
    let p11 = ndc(x1, y1, sw, sh);
    let p01 = ndc(x0, y1, sw, sh);
    let v = |p: [f32; 2], uv: [f32; 2]| UiVertex {
        position: p,
        uv,
        color,
    };
    quads.push(v(p00, [uv_min[0], uv_min[1]]));
    quads.push(v(p10, [uv_max[0], uv_min[1]]));
    quads.push(v(p11, [uv_max[0], uv_max[1]]));
    quads.push(v(p00, [uv_min[0], uv_min[1]]));
    quads.push(v(p11, [uv_max[0], uv_max[1]]));
    quads.push(v(p01, [uv_min[0], uv_max[1]]));
}

fn push_rect(
    quads: &mut Vec<UiVertex>,
    rect: spawn_core::Rect,
    color: Color,
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    sw: f32,
    sh: f32,
) {
    if color.a <= 0.0 {
        return;
    }
    push_quad(
        quads,
        rect.min.x,
        rect.min.y,
        rect.max.x,
        rect.max.y,
        uv_min,
        uv_max,
        [color.r, color.g, color.b, color.a],
        sw,
        sh,
    );
}

fn push_border(
    quads: &mut Vec<UiVertex>,
    rect: spawn_core::Rect,
    width: f32,
    color: Color,
    sw: f32,
    sh: f32,
) {
    if width <= 0.0 || color.a <= 0.0 {
        return;
    }
    let c = [color.r, color.g, color.b, color.a];
    let (x0, y0, x1, y1) = (rect.min.x, rect.min.y, rect.max.x, rect.max.y);
    let w = width.min((x1 - x0).max(0.0)).min((y1 - y0).max(0.0));
    let uv = ([0.0, 0.0], [1.0, 1.0]);
    // top, bottom, left, right insets.
    push_quad(quads, x0, y0, x1, y0 + w, uv.0, uv.1, c, sw, sh);
    push_quad(quads, x0, y1 - w, x1, y1, uv.0, uv.1, c, sw, sh);
    push_quad(quads, x0, y0 + w, x0 + w, y1 - w, uv.0, uv.1, c, sw, sh);
    push_quad(quads, x1 - w, y0 + w, x1, y1 - w, uv.0, uv.1, c, sw, sh);
}

fn push_text(
    quads: &mut Vec<UiVertex>,
    font: &Font,
    text: &str,
    rect: spawn_core::Rect,
    sw: f32,
    sh: f32,
) {
    let advance = font.advance();
    let line_height = font.line_height();
    let mut x = rect.min.x;
    let mut y = rect.min.y;
    for ch in text.chars() {
        if ch == '\n' {
            x = rect.min.x;
            y += line_height;
            continue;
        }
        if let Some(glyph) = font.glyph(ch) {
            push_quad(
                quads,
                x,
                y,
                x + advance,
                y + line_height,
                [glyph.uv_min.x, glyph.uv_min.y],
                [glyph.uv_max.x, glyph.uv_max.y],
                TEXT_COLOR,
                sw,
                sh,
            );
        }
        x += advance;
    }
}

fn rect_to_scissor(rect: spawn_core::Rect, size: SurfaceSize) -> [u32; 4] {
    let x0 = rect.min.x.max(0.0).floor() as u32;
    let y0 = rect.min.y.max(0.0).floor() as u32;
    let x1 = rect.max.x.max(0.0).ceil() as u32;
    let y1 = rect.max.y.max(0.0).ceil() as u32;
    let x0 = x0.min(size.width);
    let y0 = y0.min(size.height);
    let w = x1.min(size.width).saturating_sub(x0);
    let h = y1.min(size.height).saturating_sub(y0);
    [x0, y0, w, h]
}

fn intersect_scissor(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let ax1 = a[0] + a[2];
    let ay1 = a[1] + a[3];
    let bx1 = b[0] + b[2];
    let by1 = b[1] + b[3];
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = ax1.min(bx1);
    let y1 = ay1.min(by1);
    [x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::{Rect, Vec2};
    use spawn_ui::{Dimension, Size, Style, TextMeasure};

    struct NoText;
    impl TextMeasure for NoText {
        fn measure(&mut self, _t: &str, _w: Option<f32>) -> Vec2 {
            Vec2::ZERO
        }
    }

    #[test]
    fn scissor_intersection_and_conversion() {
        assert_eq!(
            intersect_scissor([0, 0, 100, 100], [10, 10, 50, 50]),
            [10, 10, 50, 50]
        );
        // Disjoint rects intersect to zero area.
        assert_eq!(intersect_scissor([0, 0, 10, 10], [50, 50, 10, 10])[2], 0);
        let s = rect_to_scissor(
            Rect::new(Vec2::new(10.0, 20.0), Vec2::new(40.0, 60.0)),
            SurfaceSize::new(100, 100),
        );
        assert_eq!(s, [10, 20, 30, 40]);
        // Clamped to the surface.
        let clamped = rect_to_scissor(
            Rect::new(Vec2::new(0.0, 0.0), Vec2::new(200.0, 200.0)),
            SurfaceSize::new(64, 48),
        );
        assert_eq!(clamped, [0, 0, 64, 48]);
    }

    #[test]
    fn batches_coalesce_then_split_on_change() {
        let mut b = Vec::new();
        extend_batch(&mut b, BatchTexture::White, None, 0, 6);
        extend_batch(&mut b, BatchTexture::White, None, 6, 12);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].range, 0..12);
        // A different texture starts a new batch.
        extend_batch(&mut b, BatchTexture::Font(7), None, 12, 18);
        assert_eq!(b.len(), 2);
        // A different scissor starts a new batch.
        extend_batch(&mut b, BatchTexture::Font(7), Some([0, 0, 1, 1]), 18, 24);
        assert_eq!(b.len(), 3);
        // An empty range is a no-op.
        extend_batch(&mut b, BatchTexture::White, None, 24, 24);
        assert_eq!(b.len(), 3);
    }

    fn px(w: f32, h: f32) -> Size {
        Size {
            width: Dimension::Px(w),
            height: Dimension::Px(h),
        }
    }

    #[test]
    fn background_becomes_one_white_quad_batch() {
        let mut tree = UiTree::new(Style {
            background: Color::new(0.1, 0.2, 0.3, 1.0),
            size: px(100.0, 100.0),
            ..Default::default()
        });
        let mut measure = NoText;
        tree.compute_layout(Vec2::new(100.0, 100.0), &mut measure)
            .unwrap();
        let mut dl = DrawList::default();
        tree.build_draw_list(&mut dl).unwrap();

        let fonts = FontRegistry::new();
        let overlay = Overlay {
            tree: &tree,
            draw_list: &dl,
            fonts: &fonts,
            lines: &[],
        };
        let mut quads = Vec::new();
        let mut batches = Vec::new();
        let mut scissor_stack = Vec::new();
        build_ui(
            &overlay,
            SurfaceSize::new(100, 100),
            &mut quads,
            &mut batches,
            &mut scissor_stack,
        );
        // One opaque background rect → one quad (6 vertices), one White batch.
        assert_eq!(quads.len(), 6);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].texture, BatchTexture::White);
        assert_eq!(batches[0].range, 0..6);
    }

    #[test]
    fn ndc_maps_corners() {
        // Top-left pixel → NDC (-1, 1); bottom-right → (1, -1).
        assert_eq!(ndc(0.0, 0.0, 100.0, 100.0), [-1.0, 1.0]);
        assert_eq!(ndc(100.0, 100.0, 100.0, 100.0), [1.0, -1.0]);
    }
}
