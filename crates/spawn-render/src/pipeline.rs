//! Pipeline cache and shader store.
//!
//! `wgpu::RenderPipeline` is created exactly once per [`PipelineKey`], in
//! [`PipelineCache::get_or_create`], at startup or asset load — never per frame.
//! `wgpu::ShaderModule` is compiled exactly once per [`ShaderHandle`], in
//! [`ShaderStore::load`]. Per-frame code only reads via [`PipelineCache::get`];
//! a draw against an uncached pipeline is [`RenderError::PipelineNotCached`].

use std::collections::HashMap;

use crate::asset_handle::ShaderHandle;
use crate::camera::CameraUniform;
use crate::error::{RenderError, RenderResult};
use crate::format::{CompareFn, CullMode, DepthFormat, TextureFormat, Topology};
use crate::graph::PassKind;
use crate::light::LightUniform;
use crate::material::{MaterialUniform, PbrMaterialUniform};
use crate::mesh::{LineVertex, UiVertex, Vertex};

/// The vertex layout a pipeline consumes. Part of the cache key so pipelines with
/// different vertex inputs are distinct entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VertexLayoutId {
    /// The 3D mesh vertex (position/normal/uv) — forward and shadow passes.
    PositionNormalUv,
    /// The 2D overlay UI vertex (clip position / uv / color).
    UiQuad,
    /// The overlay line vertex (world position / color).
    OverlayLine,
}

/// The render-state half of a [`PipelineKey`]. All fields are `Copy + Eq + Hash`
/// so two materials with identical state share one pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderStateKey {
    pub color_format: TextureFormat,
    pub depth_format: DepthFormat,
    pub depth_compare: CompareFn,
    pub depth_write: bool,
    pub cull: CullMode,
    pub topology: Topology,
}

/// Cache identity of a render pipeline. Equal keys ⇒ same cached pipeline.
/// `shader` identity is the source [`ShaderHandle`]; equal handles reuse the
/// same compiled module. `pass` selects the pipeline layout (group set) and
/// whether a fragment stage runs, so the unlit, lit, and shadow pipelines for one
/// shader/state are distinct cache entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub shader: ShaderHandle,
    pub vertex_layout: VertexLayoutId,
    pub render_state: RenderStateKey,
    pub pass: PassKind,
}

/// Compiled WGSL modules keyed by [`ShaderHandle`]. Modules live here for the
/// store's lifetime; pipelines reference them during creation only.
pub struct ShaderStore {
    modules: HashMap<ShaderHandle, wgpu::ShaderModule>,
}

impl Default for ShaderStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ShaderStore {
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
        }
    }

    /// Compiles `source` (WGSL) into a module under `handle`, or returns the
    /// already-compiled module on a repeat call.
    ///
    /// Compilation never happens mid-frame. A WGSL error surfaces as
    /// [`RenderError::ShaderCompile`]. This pushes an error scope so the failure
    /// is captured rather than only logged by wgpu.
    pub fn load(
        &mut self,
        device: &wgpu::Device,
        handle: ShaderHandle,
        source: &str,
    ) -> RenderResult<&wgpu::ShaderModule> {
        use std::collections::hash_map::Entry;
        match self.modules.entry(handle) {
            Entry::Occupied(e) => Ok(e.into_mut()),
            Entry::Vacant(e) => {
                device.push_error_scope(wgpu::ErrorFilter::Validation);
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("spawn-shader"),
                    source: wgpu::ShaderSource::Wgsl(source.into()),
                });
                if let Some(err) = pollster::block_on(device.pop_error_scope()) {
                    let info = pollster::block_on(module.get_compilation_info());
                    return Err(RenderError::ShaderCompile {
                        handle,
                        message: err.to_string(),
                        location: first_error_location(&info),
                    });
                }
                Ok(e.insert(module))
            }
        }
    }

    pub(crate) fn get(&self, handle: &ShaderHandle) -> Option<&wgpu::ShaderModule> {
        self.modules.get(handle)
    }
}

/// Extracts the source position of the first error-level message wgpu attaches
/// to a failed compilation, when one is present. wgpu does not always populate a
/// location (some backends report message-only diagnostics), so this returns
/// `None` in that case and the error carries the message alone.
fn first_error_location(info: &wgpu::CompilationInfo) -> Option<crate::error::SourceLocation> {
    info.messages
        .iter()
        .filter(|m| m.message_type == wgpu::CompilationMessageType::Error)
        .find_map(|m| m.location)
        .map(|loc| crate::error::SourceLocation {
            line: loc.line_number,
            column: loc.line_position,
        })
}

/// Per-draw model transform uploaded into a renderer-owned dynamic-offset
/// uniform buffer. `#[repr(C)]` + `Pod`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ModelUniform {
    pub model: [[f32; 4]; 4],
}

const _: () = assert!(std::mem::size_of::<ModelUniform>() == 64);

/// Bind-group layouts shared by every pipeline: group 0 is camera (binding 0,
/// dynamic offset) and model (binding 1, dynamic offset); group 1 is material
/// (uniform, texture, sampler); group 2 is light (uniform, shadow depth texture,
/// comparison sampler), used by the lit pass. Owned by the renderer so all
/// pipelines and materials reference identical layouts.
pub struct BindGroupLayouts {
    pub camera: wgpu::BindGroupLayout,
    pub material: wgpu::BindGroupLayout,
    pub light: wgpu::BindGroupLayout,
    /// Group 1 of the `ForwardPbr` pass: the [`PbrMaterialUniform`] at binding 0
    /// plus the five metallic-roughness `(texture, sampler)` pairs (base-color,
    /// metallic-roughness, normal, emissive, occlusion) at bindings 1–10. Absent
    /// maps bind the renderer's typed fallbacks so the layout is always satisfied.
    pub pbr_material: wgpu::BindGroupLayout,
    /// Group 0 of a fullscreen post pass (tonemap): a float input texture at
    /// binding 0 and a filtering sampler at binding 1.
    pub fullscreen: wgpu::BindGroupLayout,
    /// Overlay UI texture group (group 0 of the UI pipeline): a float texture at
    /// binding 0 and a filtering sampler at binding 1. Bound to the 1×1 white
    /// texture for solid rects/borders, or a font atlas for text.
    pub overlay_texture: wgpu::BindGroupLayout,
}

impl BindGroupLayouts {
    pub fn new(device: &wgpu::Device) -> Self {
        let camera = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spawn-camera-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        // Per-pass camera slot via dynamic offset (multi-pass graphs).
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<CameraUniform>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<ModelUniform>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });
        let material = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spawn-material-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<MaterialUniform>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let light = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spawn-light-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<LightUniform>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let pbr_material = {
            let mut entries = vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<PbrMaterialUniform>() as u64,
                    ),
                },
                count: None,
            }];
            for slot in 0..5u32 {
                let base = 1 + slot * 2;
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: base,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                });
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: base + 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                });
            }
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("spawn-pbr-material-bgl"),
                entries: &entries,
            })
        };
        let fullscreen = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spawn-fullscreen-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let overlay_texture = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spawn-overlay-texture-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        Self {
            camera,
            material,
            light,
            pbr_material,
            fullscreen,
            overlay_texture,
        }
    }
}

fn cull_to_wgpu(cull: CullMode) -> Option<wgpu::Face> {
    match cull {
        CullMode::None => None,
        CullMode::Front => Some(wgpu::Face::Front),
        CullMode::Back => Some(wgpu::Face::Back),
    }
}

/// Owns every `wgpu::RenderPipeline` keyed by [`PipelineKey`]. Pipelines live
/// here exclusively; materials and meshes carry the *key*, not the pipeline.
pub struct PipelineCache {
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
}

impl Default for PipelineCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PipelineCache {
    pub fn new() -> Self {
        Self {
            pipelines: HashMap::new(),
        }
    }

    /// Returns the pipeline for `key`, building and caching it on a miss. The
    /// only constructor of `wgpu::RenderPipeline`. Call at startup/asset-load,
    /// not per frame. `Err(ShaderCompile)`-shaped failures cannot occur here —
    /// the module must already be in `shaders` (compiled via
    /// [`ShaderStore::load`]); a missing module yields
    /// [`RenderError::PipelineNotCached`] for `key`.
    pub fn get_or_create(
        &mut self,
        device: &wgpu::Device,
        layouts: &BindGroupLayouts,
        key: PipelineKey,
        shaders: &ShaderStore,
    ) -> RenderResult<&wgpu::RenderPipeline> {
        use std::collections::hash_map::Entry;
        if let Entry::Vacant(slot) = self.pipelines.entry(key) {
            let module = shaders
                .get(&key.shader)
                .ok_or(RenderError::PipelineNotCached(key))?;

            // The group set and fragment stage depend on the pass: shadow is
            // depth-only (group 0, no fragment, no color target); lit binds the
            // light group; opaque is unlit; the overlay's UI pipeline binds only a
            // texture group (group 0) while its line pipeline reuses the camera
            // group. wgpu inserts all barriers — the layout here only declares the
            // resource interface.
            let bind_group_layouts: &[&wgpu::BindGroupLayout] = match key.pass {
                PassKind::ForwardOpaque => &[&layouts.camera, &layouts.material],
                PassKind::ForwardLit => &[&layouts.camera, &layouts.material, &layouts.light],
                PassKind::ForwardPbr => &[&layouts.camera, &layouts.pbr_material, &layouts.light],
                PassKind::Tonemap => &[&layouts.fullscreen],
                PassKind::ShadowDepth => &[&layouts.camera],
                PassKind::Overlay2D => match key.vertex_layout {
                    VertexLayoutId::UiQuad => &[&layouts.overlay_texture],
                    VertexLayoutId::OverlayLine => &[&layouts.camera],
                    VertexLayoutId::PositionNormalUv => {
                        return Err(RenderError::InvalidArgument {
                            context: "overlay pipeline needs a UiQuad or OverlayLine vertex layout",
                        })
                    }
                },
            };
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("spawn-pipeline-layout"),
                bind_group_layouts,
                push_constant_ranges: &[],
            });

            // The overlay composites on top of the lit frame and the tonemap is a
            // fullscreen resolve: both run without depth. The 3D passes are opaque
            // with depth.
            let depth_stencil = match key.pass {
                PassKind::Overlay2D | PassKind::Tonemap => None,
                _ => Some(wgpu::DepthStencilState {
                    format: key.render_state.depth_format.to_wgpu(),
                    depth_write_enabled: key.render_state.depth_write,
                    depth_compare: key.render_state.depth_compare,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
            };
            let blend = match key.pass {
                PassKind::Overlay2D => Some(wgpu::BlendState::ALPHA_BLENDING),
                _ => None,
            };

            let color_targets = [Some(wgpu::ColorTargetState {
                format: key.render_state.color_format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })];
            // The shadow pass writes depth only — no fragment stage, no color
            // attachment.
            let fragment = match key.pass {
                PassKind::ShadowDepth => None,
                PassKind::ForwardOpaque
                | PassKind::ForwardLit
                | PassKind::ForwardPbr
                | PassKind::Tonemap
                | PassKind::Overlay2D => Some(wgpu::FragmentState {
                    module,
                    entry_point: "fs_main",
                    targets: &color_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
            };

            // Fullscreen passes generate their triangle from the vertex index and
            // bind no vertex buffer; every other pass consumes one vertex layout.
            let mesh_buffer = [match key.vertex_layout {
                VertexLayoutId::PositionNormalUv => Vertex::layout(),
                VertexLayoutId::UiQuad => UiVertex::layout(),
                VertexLayoutId::OverlayLine => LineVertex::layout(),
            }];
            let vertex_buffers: &[wgpu::VertexBufferLayout] = match key.pass {
                PassKind::Tonemap => &[],
                _ => &mesh_buffer,
            };

            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("spawn-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: "vs_main",
                    buffers: vertex_buffers,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment,
                primitive: wgpu::PrimitiveState {
                    topology: key.render_state.topology,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull_to_wgpu(key.render_state.cull),
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
            slot.insert(pipeline);
        }
        self.pipelines
            .get(&key)
            .ok_or(RenderError::PipelineNotCached(key))
    }

    /// Looks up a built pipeline without building. Per-frame draw path.
    /// `Err(PipelineNotCached)` if it was never built at load time.
    pub fn get(&self, key: &PipelineKey) -> RenderResult<&wgpu::RenderPipeline> {
        self.pipelines
            .get(key)
            .ok_or(RenderError::PipelineNotCached(*key))
    }

    pub fn contains(&self, key: &PipelineKey) -> bool {
        self.pipelines.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.pipelines.len()
    }

    // Kept to satisfy clippy::len_without_is_empty: a public `len` requires a
    // companion `is_empty`. Trivial, allocation-free, no per-frame use.
    pub fn is_empty(&self) -> bool {
        self.pipelines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_asset::AssetId;

    fn key(shader_id: u64, write: bool) -> PipelineKey {
        PipelineKey {
            shader: ShaderHandle::from_id(AssetId::from_raw(shader_id)),
            vertex_layout: VertexLayoutId::PositionNormalUv,
            render_state: RenderStateKey {
                color_format: TextureFormat::Rgba8UnormSrgb,
                depth_format: DepthFormat::Depth32Float,
                depth_compare: CompareFn::Less,
                depth_write: write,
                cull: CullMode::Back,
                topology: Topology::TriangleList,
            },
            pass: PassKind::ForwardOpaque,
        }
    }

    #[test]
    fn equal_keys_hash_and_compare_equal() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let a = key(1, true);
        let b = key(1, true);
        assert_eq!(a, b);
        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.hash(&mut ha);
        b.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
    }

    #[test]
    fn differing_fields_distinguish_keys() {
        assert_ne!(key(1, true), key(2, true));
        assert_ne!(key(1, true), key(1, false));
    }

    #[test]
    fn pass_discriminator_distinguishes_keys() {
        // The lit and shadow pipelines for one shader/state must be distinct cache
        // entries from the unlit one (different layouts / fragment stage).
        let mut lit = key(1, true);
        lit.pass = PassKind::ForwardLit;
        let mut shadow = key(1, true);
        shadow.pass = PassKind::ShadowDepth;
        assert_ne!(key(1, true), lit);
        assert_ne!(key(1, true), shadow);
        assert_ne!(lit, shadow);
    }

    #[test]
    fn empty_cache_reports_not_cached() {
        let cache = PipelineCache::new();
        assert!(cache.is_empty());
        assert!(!cache.contains(&key(1, true)));
        assert!(matches!(
            cache.get(&key(1, true)),
            Err(RenderError::PipelineNotCached(_))
        ));
    }

    const TEST_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32>, view_pos: vec4<f32> };
struct Model { model: mat4x4<f32> };
struct Material { base_color: vec4<f32>, params: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> model: Model;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * model.model * vec4<f32>(position, 1.0);
    out.uv = uv;
    return out;
}
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return material.base_color * textureSample(tex, samp, in.uv);
}
"#;

    fn try_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: true,
            compatible_surface: None,
        }))?;
        pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("spawn-test-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .ok()
    }

    #[test]
    fn shader_and_pipeline_build_once_and_cache() {
        let Some((device, _queue)) = try_device() else {
            eprintln!("SKIP shader_and_pipeline_build_once_and_cache: no GPU adapter");
            return;
        };
        let shader = ShaderHandle::from_id(AssetId::from_raw(99));
        let mut store = ShaderStore::new();
        assert!(store.load(&device, shader, TEST_WGSL).is_ok());
        // Second load reuses the module (no recompile, still Ok).
        assert!(store.load(&device, shader, TEST_WGSL).is_ok());

        let layouts = BindGroupLayouts::new(&device);
        let k = PipelineKey {
            shader,
            vertex_layout: VertexLayoutId::PositionNormalUv,
            render_state: RenderStateKey {
                color_format: TextureFormat::Rgba8UnormSrgb,
                depth_format: DepthFormat::Depth32Float,
                depth_compare: CompareFn::Less,
                depth_write: true,
                cull: CullMode::Back,
                topology: Topology::TriangleList,
            },
            pass: PassKind::ForwardOpaque,
        };
        let mut cache = PipelineCache::new();
        assert!(cache.get_or_create(&device, &layouts, k, &store).is_ok());
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&k));
        // Cache hit: no new entry.
        assert!(cache.get_or_create(&device, &layouts, k, &store).is_ok());
        assert_eq!(cache.len(), 1);
        // A draw-time lookup succeeds for the built key.
        assert!(cache.get(&k).is_ok());
    }

    #[test]
    fn shader_compile_failure_surfaces_error() {
        let Some((device, _queue)) = try_device() else {
            eprintln!("SKIP shader_compile_failure_surfaces_error: no GPU adapter");
            return;
        };
        let shader = ShaderHandle::from_id(AssetId::from_raw(100));
        let mut store = ShaderStore::new();
        // A WGSL with a known error on a known line: an undefined identifier in a
        // function body. wgpu's front-end reports this with a source location.
        let bad = "@vertex\nfn vs_main() -> @builtin(position) vec4<f32> {\n    return nope;\n}\n";
        let err = store.load(&device, shader, bad);
        let Err(RenderError::ShaderCompile {
            message, location, ..
        }) = err
        else {
            panic!("expected ShaderCompile error");
        };
        assert!(!message.is_empty(), "compile error must carry a message");
        // §13: the location must be captured when wgpu provides one. The Mesa /
        // naga front-end reports a location for this class of error; if a backend
        // ever omits it, the contract permits None, so only assert correctness of
        // a provided location rather than its mere presence.
        if let Some(loc) = location {
            assert!(loc.line >= 1, "reported line is 1-based");
        }
    }
}
