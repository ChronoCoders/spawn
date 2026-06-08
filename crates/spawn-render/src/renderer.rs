//! The `Renderer`: owns all GPU state for one window/surface.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use wgpu::util::DeviceExt;

use crate::camera::CameraUniform;
use crate::error::{RenderError, RenderResult};
use crate::format::{DepthFormat, PowerPreference, PresentMode, SurfaceSize, TextureFormat};
use crate::pipeline::{BindGroupLayouts, ModelUniform, PipelineCache, ShaderStore};
use crate::texture::{SamplerConfig, Texture};

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
    pub(crate) model_buffer: wgpu::Buffer,
    pub(crate) model_stride: u64,
    pub(crate) model_capacity: u32,
    pub(crate) fallback_texture: Texture,
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

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spawn-camera-uniform"),
            contents: bytemuck::bytes_of(&CameraUniform {
                view_proj: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
                view_pos: [0.0, 0.0, 0.0, 1.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
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

        let fallback_texture = Texture::build(
            &device,
            &queue,
            &[255, 255, 255, 255],
            SurfaceSize::new(1, 1),
            true,
            SamplerConfig::default(),
        )?;

        Ok(Self {
            device,
            queue,
            device_lost,
            cache: PipelineCache::new(),
            shaders: ShaderStore::new(),
            layouts,
            camera_buffer,
            camera_bind_group,
            model_buffer,
            model_stride,
            model_capacity,
            fallback_texture,
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

    pub(crate) fn fallback_texture(&self) -> &Texture {
        &self.fallback_texture
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

    /// Uploads `uniform` into the renderer-owned camera buffer in place; no
    /// reallocation. Called once per frame from the forward pass.
    ///
    /// Invariant: the camera and per-draw model buffers are
    /// singletons submitted once at `end_frame`. A second surface pass in the
    /// same frame would overwrite this buffer before submission, clobbering the
    /// first pass. [`crate::graph::RenderGraph::validate`] enforces exactly one
    /// surface-color pass per frame, so this write happens at most once per
    /// frame and the clobber cannot occur. Per-pass uniforms are Phase 2.
    pub(crate) fn write_camera(&self, uniform: &CameraUniform) {
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(uniform));
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
                resource: camera_buffer.as_entire_binding(),
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
