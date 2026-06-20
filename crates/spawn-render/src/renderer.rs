//! The `Renderer`: owns all GPU state for one window/surface.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use spawn_asset::AssetId;

use crate::asset_handle::ShaderHandle;
use crate::camera::CameraUniform;
use crate::error::{RenderError, RenderResult};
use crate::format::{
    CompareFn, CullMode, DepthFormat, PowerPreference, PresentMode, SurfaceSize, TextureFormat,
    Topology,
};
use crate::graph::PassKind;
use crate::light::LightUniform;
use crate::passes::overlay::{make_texture_bind_group, OverlayState};
use crate::pipeline::{
    BindGroupLayouts, ModelUniform, PipelineCache, PipelineKey, RenderStateKey, ShaderStore,
    VertexLayoutId,
};
use crate::shaders::{LIT_WGSL, PBR_WGSL, SHADOW_WGSL, TONEMAP_WGSL};
use crate::texture::{SamplerConfig, Texture};

/// The linear HDR format the `ForwardPbr`/scene passes render into before the
/// tonemap reduces it to the LDR surface. Callers building a PBR graph size the
/// scene-color transient with this format (see [`Renderer::hdr_format`]).
const HDR_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// The `raw-window-handle` bound a surface source must satisfy. `Send + Sync` is
/// required by wgpu's surface target. spawn-platform's `Window` implements it.
pub trait HasWindowHandleSet: HasWindowHandle + HasDisplayHandle + Send + Sync {}
impl<T: HasWindowHandle + HasDisplayHandle + Send + Sync> HasWindowHandleSet for T {}

/// Renderer construction parameters.
pub struct RendererConfig {
    pub power_preference: PowerPreference,
    pub present_mode: PresentMode,
    /// `None` selects the first sRGB-capable supported surface format.
    pub surface_format: Option<TextureFormat>,
    pub depth_format: DepthFormat,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            power_preference: PowerPreference::HighPerformance,
            present_mode: PresentMode::Fifo,
            surface_format: None,
            depth_format: DepthFormat::Depth32Float,
        }
    }
}

/// Owns the wgpu instance/adapter/device/queue, the surface and its config, the
/// depth target, the pipeline cache, shader store, shared bind-group layouts,
/// the camera uniform + group-0 bind group, the per-draw model uniform buffer,
/// and a fallback texture. The `'w` lifetime ties the surface to the window
/// handle it was created from.
///
/// Drop order follows field order: fallback texture, camera/model resources,
/// cache/shaders/layouts, depth view/texture, surface config, surface, queue,
/// device, adapter, instance. Engine wrappers do no manual GPU teardown — wgpu
/// frees on drop.
pub struct Renderer<'w> {
    pub(crate) device: Arc<wgpu::Device>,
    pub(crate) queue: Arc<wgpu::Queue>,
    pub(crate) device_lost: Arc<AtomicBool>,
    pub(crate) cache: PipelineCache,
    pub(crate) shaders: ShaderStore,
    pub(crate) layouts: BindGroupLayouts,
    pub(crate) camera_buffer: wgpu::Buffer,
    pub(crate) camera_bind_group: wgpu::BindGroup,
    pub(crate) camera_stride: u64,
    pub(crate) camera_capacity: u32,
    pub(crate) model_buffer: wgpu::Buffer,
    pub(crate) model_stride: u64,
    pub(crate) model_capacity: u32,
    light_buffer: wgpu::Buffer,
    shadow_sampler: wgpu::Sampler,
    fullscreen_sampler: wgpu::Sampler,
    lit_pipeline_key: PipelineKey,
    shadow_pipeline_key: PipelineKey,
    pbr_pipeline_key: PipelineKey,
    tonemap_pipeline_key: PipelineKey,
    pub(crate) overlay: OverlayState,
    pub(crate) fallback_texture: Texture,
    pub(crate) fallback_normal: Texture,
    pub(crate) fallback_black: Texture,
    pub(crate) depth_view: wgpu::TextureView,
    depth_texture: wgpu::Texture,
    pub(crate) surface: wgpu::Surface<'w>,
    pub(crate) surface_config: wgpu::SurfaceConfiguration,
    depth_format: DepthFormat,
    size: SurfaceSize,
    _adapter: wgpu::Adapter,
    _instance: wgpu::Instance,
}

fn pick_surface_format(
    caps: &wgpu::SurfaceCapabilities,
    requested: Option<TextureFormat>,
) -> RenderResult<TextureFormat> {
    if let Some(fmt) = requested {
        if caps.formats.contains(&fmt) {
            return Ok(fmt);
        }
        return Err(RenderError::InvalidArgument {
            context: "requested surface format unsupported",
        });
    }
    if let Some(srgb) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
        return Ok(srgb);
    }
    caps.formats
        .first()
        .copied()
        .ok_or(RenderError::InvalidArgument {
            context: "surface exposes no formats",
        })
}

fn pick_present_mode(caps: &wgpu::SurfaceCapabilities, requested: PresentMode) -> PresentMode {
    if caps.present_modes.contains(&requested) {
        requested
    } else {
        PresentMode::Fifo
    }
}

fn create_depth(
    device: &wgpu::Device,
    format: DepthFormat,
    size: SurfaceSize,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("spawn-depth"),
        size: wgpu::Extent3d {
            width: size.width.max(1),
            height: size.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: format.to_wgpu(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

impl<'w> Renderer<'w> {
    /// Initializes the GPU: requests instance/adapter/device/queue, creates and
    /// configures the surface, and allocates the depth target.
    ///
    /// `Err(NoAdapter)` if no compatible adapter exists (the headless-skip gate),
    /// `Err(DeviceRequest)` if device creation fails, `Err(InvalidArgument)` for
    /// a zero `size` or an unsupported requested surface format, `Err(Surface)`
    /// if surface creation fails.
    pub fn new(
        window: &'w (impl HasWindowHandleSet + 'w),
        size: SurfaceSize,
        config: RendererConfig,
    ) -> RenderResult<Self> {
        if size.is_zero() {
            return Err(RenderError::InvalidArgument {
                context: "initial surface size is zero",
            });
        }

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Window(Box::new(WindowRef(window))))
            .map_err(|_| RenderError::Surface)?;

        Self::from_instance_surface(instance, surface, size, config)
    }

    /// Initializes the GPU against an already-created instance and surface, the
    /// shared tail of [`new`](Renderer::new) and
    /// [`from_owned`](Renderer::from_owned). The `'w` lifetime is the surface's;
    /// for an owned window source it is `'static`.
    fn from_instance_surface(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'w>,
        size: SurfaceSize,
        config: RendererConfig,
    ) -> RenderResult<Self> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: config.power_preference,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .ok_or(RenderError::NoAdapter)?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("spawn-device"),
                required_features: wgpu::Features::empty(),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .map_err(|err| RenderError::DeviceRequest {
            message: err.to_string(),
        })?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        // Device-lost detection contract: wgpu 22 `Queue::submit` returns a
        // `SubmissionIndex` (not a `Result`), so submission itself never reports
        // device loss. Instead wgpu invokes this callback once when the device is
        // lost (driver reset, removal, or explicit destroy). The callback only
        // sets a shared atomic flag; `begin_frame`/`end_frame` read the flag (via
        // `device_lost_error`) and surface `RenderError::DeviceLost`. The flag is
        // sticky: once lost the device never recovers, so every subsequent frame
        // fails fast. The callback runs on a wgpu-internal thread, hence `Send`.
        let device_lost = Arc::new(AtomicBool::new(false));
        {
            let flag = Arc::clone(&device_lost);
            device.set_device_lost_callback(move |_reason, _message| {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            });
        }

        let caps = surface.get_capabilities(&adapter);
        let surface_format = pick_surface_format(&caps, config.surface_format)?;
        let present_mode = pick_present_mode(&caps, config.present_mode);
        let alpha_mode = caps
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let (depth_texture, depth_view) = create_depth(&device, config.depth_format, size);

        let layouts = BindGroupLayouts::new(&device);

        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let camera_stride = align_up(std::mem::size_of::<CameraUniform>() as u64, align.max(1));
        let camera_capacity = INITIAL_CAMERA_CAPACITY;
        // Dynamic-offset camera buffer: one slot per pass, so a multi-pass graph
        // (shadow view vs camera view) never clobbers a singleton.
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spawn-camera-uniform"),
            size: camera_stride * camera_capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_stride = align_up(std::mem::size_of::<ModelUniform>() as u64, align.max(1));
        let model_capacity = INITIAL_MODEL_CAPACITY;
        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spawn-model-uniform"),
            size: model_stride * model_capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_bind_group =
            make_camera_bind_group(&device, &layouts.camera, &camera_buffer, &model_buffer);

        // The lit and shadow shaders are engine built-ins, compiled once and their
        // pipelines built here at construction — never per frame. The lit pass
        // shades every draw with one pipeline (materials supply group 1); the
        // shadow pass renders depth-only into the shadow map.
        let mut shaders = ShaderStore::new();
        let mut cache = PipelineCache::new();
        let lit_shader = ShaderHandle::from_id(AssetId::from_raw(BUILTIN_LIT_SHADER_ID));
        let shadow_shader = ShaderHandle::from_id(AssetId::from_raw(BUILTIN_SHADOW_SHADER_ID));
        shaders.load(&device, lit_shader, LIT_WGSL)?;
        shaders.load(&device, shadow_shader, SHADOW_WGSL)?;
        let lit_pipeline_key = PipelineKey {
            shader: lit_shader,
            vertex_layout: VertexLayoutId::PositionNormalUv,
            render_state: RenderStateKey {
                color_format: surface_format,
                depth_format: config.depth_format,
                depth_compare: CompareFn::Less,
                depth_write: true,
                cull: CullMode::Back,
                topology: Topology::TriangleList,
            },
            pass: PassKind::ForwardLit,
        };
        let shadow_pipeline_key = PipelineKey {
            shader: shadow_shader,
            vertex_layout: VertexLayoutId::PositionNormalUv,
            render_state: RenderStateKey {
                // No color target; the color format is part of the cache key only.
                color_format: surface_format,
                depth_format: config.depth_format,
                depth_compare: CompareFn::Less,
                depth_write: true,
                // Render all caster faces so thin/open geometry still casts.
                cull: CullMode::None,
                topology: Topology::TriangleList,
            },
            pass: PassKind::ShadowDepth,
        };
        cache.get_or_create(&device, &layouts, lit_pipeline_key, &shaders)?;
        cache.get_or_create(&device, &layouts, shadow_pipeline_key, &shaders)?;

        // The PBR forward pass renders into the HDR scene transient; the tonemap
        // fullscreen pass reduces that to the LDR surface. Both shaders are
        // built-ins compiled and built once here, never per frame.
        let pbr_shader = ShaderHandle::from_id(AssetId::from_raw(BUILTIN_PBR_SHADER_ID));
        let tonemap_shader = ShaderHandle::from_id(AssetId::from_raw(BUILTIN_TONEMAP_SHADER_ID));
        shaders.load(&device, pbr_shader, PBR_WGSL)?;
        shaders.load(&device, tonemap_shader, TONEMAP_WGSL)?;
        let pbr_pipeline_key = PipelineKey {
            shader: pbr_shader,
            vertex_layout: VertexLayoutId::PositionNormalUv,
            render_state: RenderStateKey {
                color_format: HDR_FORMAT,
                depth_format: config.depth_format,
                depth_compare: CompareFn::Less,
                depth_write: true,
                cull: CullMode::Back,
                topology: Topology::TriangleList,
            },
            pass: PassKind::ForwardPbr,
        };
        let tonemap_pipeline_key = PipelineKey {
            shader: tonemap_shader,
            // The fullscreen triangle is generated from the vertex index; the
            // layout field is part of the key but no vertex buffer is bound.
            vertex_layout: VertexLayoutId::PositionNormalUv,
            render_state: RenderStateKey {
                color_format: surface_format,
                depth_format: config.depth_format,
                depth_compare: CompareFn::Always,
                depth_write: false,
                cull: CullMode::None,
                topology: Topology::TriangleList,
            },
            pass: PassKind::Tonemap,
        };
        cache.get_or_create(&device, &layouts, pbr_pipeline_key, &shaders)?;
        cache.get_or_create(&device, &layouts, tonemap_pipeline_key, &shaders)?;

        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spawn-light-uniform"),
            size: std::mem::size_of::<LightUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("spawn-shadow-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let fullscreen_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("spawn-fullscreen-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let fallback_texture = Texture::build(
            &device,
            &queue,
            &[255, 255, 255, 255],
            SurfaceSize::new(1, 1),
            true,
            SamplerConfig::default(),
        )?;
        // Typed PBR fallbacks for absent metallic-roughness maps: a flat
        // tangent-space normal (+Z) and opaque black emissive, both linear (data,
        // not color). Sampling is gated by the material's texture flags, so these
        // satisfy the shared layout without affecting shading.
        let fallback_normal = Texture::build(
            &device,
            &queue,
            &[128, 128, 255, 255],
            SurfaceSize::new(1, 1),
            false,
            SamplerConfig::default(),
        )?;
        let fallback_black = Texture::build(
            &device,
            &queue,
            &[0, 0, 0, 255],
            SurfaceSize::new(1, 1),
            false,
            SamplerConfig::default(),
        )?;

        // Overlay pipelines (UI quad + line) and reused geometry buffers, built
        // once here. The 1×1 white fallback texture backs solid rects/borders.
        let overlay = OverlayState::new(
            &device,
            &layouts,
            &mut cache,
            &mut shaders,
            surface_format,
            config.depth_format,
            &fallback_texture,
        )?;

        Ok(Self {
            device,
            queue,
            device_lost,
            cache,
            shaders,
            layouts,
            camera_buffer,
            camera_bind_group,
            camera_stride,
            camera_capacity,
            model_buffer,
            model_stride,
            model_capacity,
            light_buffer,
            shadow_sampler,
            fullscreen_sampler,
            lit_pipeline_key,
            shadow_pipeline_key,
            pbr_pipeline_key,
            tonemap_pipeline_key,
            overlay,
            fallback_texture,
            fallback_normal,
            fallback_black,
            depth_view,
            depth_texture,
            surface,
            surface_config,
            depth_format: config.depth_format,
            size,
            _adapter: adapter,
            _instance: instance,
        })
    }

    pub fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }

    pub fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }

    pub fn surface_format(&self) -> TextureFormat {
        self.surface_config.format
    }

    pub fn depth_format(&self) -> DepthFormat {
        self.depth_format
    }

    pub fn size(&self) -> SurfaceSize {
        self.size
    }

    pub fn pipeline_cache(&self) -> &PipelineCache {
        &self.cache
    }

    /// Mutable pipeline cache, for the caller's startup/asset-load pipeline
    /// build (`PipelineCache::get_or_create`). Combine with [`Renderer::device`]
    /// (clone the `Arc` first to avoid borrowing `self` for the build),
    /// [`Renderer::shaders`], and [`Renderer::bind_group_layouts`]. Never called
    /// per frame.
    pub fn pipeline_cache_mut(&mut self) -> &mut PipelineCache {
        &mut self.cache
    }

    pub fn shaders(&self) -> &ShaderStore {
        &self.shaders
    }

    /// Mutable shader store, for compiling WGSL at startup/asset-load
    /// (`ShaderStore::load`) before building the pipelines that reference it.
    pub fn shaders_mut(&mut self) -> &mut ShaderStore {
        &mut self.shaders
    }

    /// The shared bind-group layouts (group 0 camera/model, group 1 material)
    /// that every Phase 1 pipeline and material must be built against so bind
    /// groups and pipelines are layout-compatible.
    pub fn bind_group_layouts(&self) -> &BindGroupLayouts {
        &self.layouts
    }

    /// Compiles WGSL under `handle` into the shader store (setup/load path; never
    /// per frame). Composes the device and the store internally so callers do not
    /// have to juggle the disjoint borrows the bare accessors would require.
    pub fn load_shader(&mut self, handle: ShaderHandle, source: &str) -> RenderResult<()> {
        self.shaders.load(&self.device, handle, source)?;
        Ok(())
    }

    /// Builds and caches the pipeline for `key` (its shader must already be loaded
    /// via [`load_shader`](Renderer::load_shader)). Setup/load path; never per
    /// frame. Composes the cache, layouts, and shader store internally.
    pub fn build_pipeline(&mut self, key: PipelineKey) -> RenderResult<()> {
        self.cache
            .get_or_create(&self.device, &self.layouts, key, &self.shaders)?;
        Ok(())
    }

    pub(crate) fn fallback_texture(&self) -> &Texture {
        &self.fallback_texture
    }

    pub(crate) fn fallback_normal_texture(&self) -> &Texture {
        &self.fallback_normal
    }

    pub(crate) fn fallback_black_texture(&self) -> &Texture {
        &self.fallback_black
    }

    /// The linear HDR format the PBR/scene passes render into. Callers building a
    /// PBR graph size the scene-color transient with this format; the tonemap pass
    /// resolves it to the LDR surface.
    pub fn hdr_format(&self) -> TextureFormat {
        HDR_FORMAT
    }

    /// Reads the device-lost flag set by the wgpu device-lost callback. `true`
    /// once the GPU device has been lost (sticky). The frame lifecycle maps this
    /// to [`RenderError::DeviceLost`] via [`crate::frame::device_lost_error`].
    pub(crate) fn is_device_lost(&self) -> bool {
        self.device_lost.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Reconfigures the surface and recreates the depth target at the current
    /// size. No-op for a zero size. Reused by [`Renderer::resize`] and
    /// surface-loss recovery in the frame lifecycle.
    pub(crate) fn reconfigure(&mut self) {
        if self.size.is_zero() {
            return;
        }
        self.surface_config.width = self.size.width;
        self.surface_config.height = self.size.height;
        self.surface.configure(&self.device, &self.surface_config);
        let (texture, view) = create_depth(&self.device, self.depth_format, self.size);
        self.depth_texture = texture;
        self.depth_view = view;
    }

    /// Reconfigures the surface and depth target for `size`. A zero width or
    /// height is a no-op returning `Ok(())` (minimized window); presentation
    /// stays suppressed until a non-zero size arrives.
    pub fn resize(&mut self, size: SurfaceSize) -> RenderResult<()> {
        self.size = size;
        if size.is_zero() {
            return Ok(());
        }
        self.reconfigure();
        Ok(())
    }

    /// Writes `uniform` into the camera slot for pass `slot` (dynamic-offset
    /// `slot * camera_stride`) in place; no reallocation. Each pass binds its own
    /// slot, so multiple passes in one frame never clobber a shared camera buffer.
    /// Caller guarantees capacity via [`Renderer::ensure_camera_capacity`].
    pub(crate) fn write_camera_slot(&self, slot: u32, uniform: &CameraUniform) {
        self.queue.write_buffer(
            &self.camera_buffer,
            u64::from(slot) * self.camera_stride,
            bytemuck::bytes_of(uniform),
        );
    }

    /// Ensures the per-pass camera buffer holds at least `count` slots,
    /// reallocating (and rebuilding the camera bind group) only on growth — never
    /// in steady state once capacity covers the largest graph's pass count.
    pub(crate) fn ensure_camera_capacity(&mut self, count: u32) {
        if count <= self.camera_capacity {
            return;
        }
        let new_capacity = count.next_power_of_two().max(INITIAL_CAMERA_CAPACITY);
        self.camera_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spawn-camera-uniform"),
            size: self.camera_stride * new_capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.camera_capacity = new_capacity;
        self.camera_bind_group = make_camera_bind_group(
            &self.device,
            &self.layouts.camera,
            &self.camera_buffer,
            &self.model_buffer,
        );
    }

    pub(crate) fn camera_stride(&self) -> u64 {
        self.camera_stride
    }

    /// Ensures the per-draw model buffer holds at least `count` entries,
    /// reallocating (and rebuilding the camera bind group) only on growth. Growth
    /// happens at most a logarithmic number of times and never in steady state —
    /// once capacity covers the largest frame it is retained.
    pub(crate) fn ensure_model_capacity(&mut self, count: u32) {
        if count <= self.model_capacity {
            return;
        }
        let new_capacity = count.next_power_of_two().max(INITIAL_MODEL_CAPACITY);
        self.model_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spawn-model-uniform"),
            size: self.model_stride * new_capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.model_capacity = new_capacity;
        self.camera_bind_group = make_camera_bind_group(
            &self.device,
            &self.layouts.camera,
            &self.camera_buffer,
            &self.model_buffer,
        );
    }

    /// Writes `model` at draw index `index` (dynamic offset `index *
    /// model_stride`) in place; no reallocation. Caller guarantees capacity via
    /// [`Renderer::ensure_model_capacity`].
    pub(crate) fn write_model(&self, index: u32, model: &ModelUniform) {
        self.queue.write_buffer(
            &self.model_buffer,
            index as u64 * self.model_stride,
            bytemuck::bytes_of(model),
        );
    }

    pub(crate) fn model_stride(&self) -> u64 {
        self.model_stride
    }

    /// Uploads the per-frame light block into the renderer-owned light buffer in
    /// place; no reallocation. The group-2 light bind group references this buffer
    /// and is unaffected.
    pub(crate) fn write_light(&self, uniform: &LightUniform) {
        self.queue
            .write_buffer(&self.light_buffer, 0, bytemuck::bytes_of(uniform));
    }

    /// Builds the group-2 light bind group from the renderer-owned light buffer
    /// and comparison sampler plus `shadow_view` (the compiled shadow map). Called
    /// at graph compile/resize, never per frame.
    pub(crate) fn create_light_bind_group(
        &self,
        shadow_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spawn-light-bg"),
            layout: &self.layouts.light,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                },
            ],
        })
    }

    /// The cache key of the built-in lit pipeline (the lit pass uses it for every
    /// draw; materials supply group 1).
    pub(crate) fn lit_pipeline_key(&self) -> PipelineKey {
        self.lit_pipeline_key
    }

    /// The cache key of the built-in depth-only shadow pipeline.
    pub(crate) fn shadow_pipeline_key(&self) -> PipelineKey {
        self.shadow_pipeline_key
    }

    /// The cache key of the built-in PBR pipeline (the `ForwardPbr` pass uses it
    /// for every draw; PBR materials supply group 1).
    pub(crate) fn pbr_pipeline_key(&self) -> PipelineKey {
        self.pbr_pipeline_key
    }

    /// The cache key of the built-in fullscreen tonemap pipeline.
    pub(crate) fn tonemap_pipeline_key(&self) -> PipelineKey {
        self.tonemap_pipeline_key
    }

    /// Builds a fullscreen-pass group-0 bind group sampling `input_view` (a scene
    /// transient) with the renderer's clamp/linear sampler. Built at graph
    /// compile/resize, never per frame.
    pub(crate) fn create_fullscreen_bind_group(
        &self,
        input_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spawn-fullscreen-bg"),
            layout: &self.layouts.fullscreen,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.fullscreen_sampler),
                },
            ],
        })
    }

    /// Builds an overlay UI texture bind group (group 0 of the UI pipeline) for
    /// `texture`. Used by [`FontRegistry`](crate::passes::overlay::FontRegistry)
    /// to bind a font atlas; built at registration, never per frame.
    pub(crate) fn create_overlay_texture_bind_group(&self, texture: &Texture) -> wgpu::BindGroup {
        make_texture_bind_group(&self.device, &self.layouts.overlay_texture, texture)
    }
}

impl Renderer<'static> {
    /// Creates a renderer that *owns* its window handle through an `Arc`, so the
    /// surface lifetime is `'static` and the renderer can be stored without a
    /// borrow tying it to the window. Used by long-lived engine wrappers (the
    /// surface keeps the window alive for its own lifetime). Same error contract
    /// as [`new`](Renderer::new).
    pub fn from_owned<W: HasWindowHandleSet + 'static>(
        window: Arc<W>,
        size: SurfaceSize,
        config: RendererConfig,
    ) -> RenderResult<Self> {
        if size.is_zero() {
            return Err(RenderError::InvalidArgument {
                context: "initial surface size is zero",
            });
        }

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Window(Box::new(OwnedWindow(window))))
            .map_err(|_| RenderError::Surface)?;

        Self::from_instance_surface(instance, surface, size, config)
    }
}

const INITIAL_MODEL_CAPACITY: u32 = 256;
const INITIAL_CAMERA_CAPACITY: u32 = 8;

// Reserved `AssetId` raw values for the engine's built-in shaders. Picked at the
// top of the id space so an app's content ids do not collide. `MAX-2`/`MAX-3` are
// the overlay UI/line shaders (see `passes::overlay`), so the PBR and tonemap
// shaders take `MAX-4`/`MAX-5` to avoid aliasing in the shared `ShaderStore`.
const BUILTIN_LIT_SHADER_ID: u64 = u64::MAX;
const BUILTIN_SHADOW_SHADER_ID: u64 = u64::MAX - 1;
const BUILTIN_PBR_SHADER_ID: u64 = u64::MAX - 4;
const BUILTIN_TONEMAP_SHADER_ID: u64 = u64::MAX - 5;

fn align_up(value: u64, align: u64) -> u64 {
    if align <= 1 {
        return value;
    }
    value.div_ceil(align) * align
}

fn make_camera_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera_buffer: &wgpu::Buffer,
    model_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("spawn-camera-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: camera_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<CameraUniform>() as u64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: model_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ModelUniform>() as u64),
                }),
            },
        ],
    })
}

/// Adapter so a borrowed `&impl HasWindowHandleSet` satisfies the boxed
/// `WindowHandle` trait object wgpu's safe `create_surface` expects. Borrows the
/// window for the surface's lifetime `'w`; no `unsafe`.
struct WindowRef<'w, W: HasWindowHandleSet + 'w>(&'w W);

impl<W: HasWindowHandleSet> HasWindowHandle for WindowRef<'_, W> {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.0.window_handle()
    }
}

impl<W: HasWindowHandleSet> HasDisplayHandle for WindowRef<'_, W> {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.0.display_handle()
    }
}

/// Owned counterpart to [`WindowRef`]: holds an `Arc` to the window so the
/// surface owns it and lives `'static`. Delegates both handle accessors to the
/// wrapped window; no `unsafe`.
struct OwnedWindow<W: HasWindowHandleSet + 'static>(Arc<W>);

impl<W: HasWindowHandleSet> HasWindowHandle for OwnedWindow<W> {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.0.window_handle()
    }
}

impl<W: HasWindowHandleSet> HasDisplayHandle for OwnedWindow<W> {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.0.display_handle()
    }
}
