//! GPU-gated shadow-correctness test (spec §12): renders a ground plane shadowed
//! by an elevated occluder, then reads the rendered pixels back and asserts a
//! point under the occluder is darker than one in open light. This exercises the
//! built-in lit + shadow WGSL, the light uniform layout, and the comparison
//! sampler end to end. **A wgpu adapter is required**; the test skips cleanly with
//! a logged note when none is available, like the other GPU tests (§13 gate).

#![cfg(test)]

use spawn_asset::AssetId;
use spawn_core::{Color, Mat4, Vec3, Vec4};

use crate::asset_handle::ShaderHandle;
use crate::camera::Camera;
use crate::format::{CompareFn, CullMode, DepthFormat, TextureFormat, Topology};
use crate::graph::PassKind;
use crate::light::{DirectionalLight, ShadowConfig};
use crate::material::MaterialUniform;
use crate::mesh::{Mesh, Vertex};
use crate::pipeline::{
    BindGroupLayouts, ModelUniform, PipelineCache, PipelineKey, RenderStateKey, ShaderStore,
    VertexLayoutId,
};
use crate::shaders::{LIT_WGSL, SHADOW_WGSL};

const W: u32 = 64;
const H: u32 = 64;
const SHADOW_RES: u32 = 1024;

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
            label: Some("spawn-shadow-test-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    ))
    .ok()
}

fn columns(m: Mat4) -> [[f32; 4]; 4] {
    let c = |v: Vec4| [v.x, v.y, v.z, v.w];
    [c(m.cols[0]), c(m.cols[1]), c(m.cols[2]), c(m.cols[3])]
}

fn align_up(v: u64, a: u64) -> u64 {
    if a <= 1 {
        v
    } else {
        v.div_ceil(a) * a
    }
}

fn ground_quad(device: &wgpu::Device, half: f32) -> Mesh {
    let n = [0.0, 1.0, 0.0];
    let v = [
        Vertex {
            position: [-half, 0.0, -half],
            normal: n,
            uv: [0.0, 0.0],
        },
        Vertex {
            position: [half, 0.0, -half],
            normal: n,
            uv: [1.0, 0.0],
        },
        Vertex {
            position: [half, 0.0, half],
            normal: n,
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [-half, 0.0, half],
            normal: n,
            uv: [0.0, 1.0],
        },
    ];
    Mesh::new(device, &v, &[0, 1, 2, 0, 2, 3]).expect("quad mesh")
}

#[test]
fn occluder_casts_shadow_on_ground() {
    let Some((device, queue)) = try_device() else {
        eprintln!("gpu_test: no adapter; skipping shadow-correctness test (spec §13 gate)");
        return;
    };

    let layouts = BindGroupLayouts::new(&device);
    let mut shaders = ShaderStore::new();
    let mut cache = PipelineCache::new();
    let lit_shader = ShaderHandle::from_id(AssetId::from_raw(0xA1));
    let shadow_shader = ShaderHandle::from_id(AssetId::from_raw(0xA2));
    shaders
        .load(&device, lit_shader, LIT_WGSL)
        .expect("lit wgsl");
    shaders
        .load(&device, shadow_shader, SHADOW_WGSL)
        .expect("shadow wgsl");

    let color_format = TextureFormat::Rgba8Unorm;
    let depth_format = DepthFormat::Depth32Float;
    let lit_key = PipelineKey {
        shader: lit_shader,
        vertex_layout: VertexLayoutId::PositionNormalUv,
        render_state: RenderStateKey {
            color_format,
            depth_format,
            depth_compare: CompareFn::Less,
            depth_write: true,
            cull: CullMode::None,
            topology: Topology::TriangleList,
        },
        pass: PassKind::ForwardLit,
    };
    let shadow_key = PipelineKey {
        shader: shadow_shader,
        vertex_layout: VertexLayoutId::PositionNormalUv,
        render_state: RenderStateKey {
            color_format,
            depth_format,
            depth_compare: CompareFn::Less,
            depth_write: true,
            cull: CullMode::None,
            topology: Topology::TriangleList,
        },
        pass: PassKind::ShadowDepth,
    };
    cache
        .get_or_create(&device, &layouts, lit_key, &shaders)
        .expect("lit pipeline");
    cache
        .get_or_create(&device, &layouts, shadow_key, &shaders)
        .expect("shadow pipeline");

    // Scene: a wide ground at y=0 and a small occluder hovering at y=2, both
    // horizontal. The light points straight down, so the occluder shadows the
    // ground directly beneath it (world xz within +/-1.5).
    let ground = ground_quad(&device, 5.0);
    let occluder = ground_quad(&device, 1.5);
    let occluder_model = Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0));

    let light = DirectionalLight {
        direction: Vec3::new(0.0, -1.0, 0.0),
        color: Color::WHITE,
        intensity: 1.0,
        ambient: Color::new(0.05, 0.05, 0.05, 1.0),
        shadow: ShadowConfig {
            center: Vec3::ZERO,
            extent: 5.0,
            near: 0.1,
            far: 12.0,
            resolution: SHADOW_RES,
            depth_bias: 0.003,
        },
    };
    let light_cam = light.shadow_camera().expect("shadow camera");
    let light_vp = light_cam.view_projection();

    // Lit camera: top-down orthographic, so image center maps to world origin
    // (under the occluder, shadowed) and the corners map to the open ground (lit).
    let lit_cam = Camera::orthographic(
        Vec3::new(0.0, 10.0, 0.0),
        Vec3::ZERO,
        Vec3::Z,
        -5.0,
        5.0,
        -5.0,
        5.0,
        0.1,
        20.0,
    )
    .expect("lit ortho camera");

    let align = device.limits().min_uniform_buffer_offset_alignment as u64;
    let camera_stride = align_up(
        std::mem::size_of::<crate::camera::CameraUniform>() as u64,
        align,
    );
    let model_stride = align_up(std::mem::size_of::<ModelUniform>() as u64, align);

    let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test-camera"),
        size: camera_stride * 2,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test-model"),
        size: model_stride * 2,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // slot 0 = light view-proj (shadow pass), slot 1 = lit camera.
    queue.write_buffer(&camera_buffer, 0, bytemuck::bytes_of(&light_cam.uniform()));
    queue.write_buffer(
        &camera_buffer,
        camera_stride,
        bytemuck::bytes_of(&lit_cam.uniform()),
    );
    // index 0 = occluder, index 1 = ground.
    let occluder_uniform = ModelUniform {
        model: columns(occluder_model),
    };
    let ground_uniform = ModelUniform {
        model: columns(Mat4::IDENTITY),
    };
    queue.write_buffer(&model_buffer, 0, bytemuck::bytes_of(&occluder_uniform));
    queue.write_buffer(
        &model_buffer,
        model_stride,
        bytemuck::bytes_of(&ground_uniform),
    );

    let camera_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("test-camera-bg"),
        layout: &layouts.camera,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &camera_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(
                        std::mem::size_of::<crate::camera::CameraUniform>() as u64,
                    ),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &model_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ModelUniform>() as u64),
                }),
            },
        ],
    });

    // Material: white, 1x1 white texture + sampler.
    let mat_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test-material"),
        size: std::mem::size_of::<MaterialUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &mat_buffer,
        0,
        bytemuck::bytes_of(&MaterialUniform::default()),
    );
    let white = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-white"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &white,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255, 255, 255, 255],
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let white_view = white.create_view(&wgpu::TextureViewDescriptor::default());
    let white_sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let material_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("test-material-bg"),
        layout: &layouts.material,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: mat_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&white_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&white_sampler),
            },
        ],
    });

    // Light bind group: uniform + shadow map + comparison sampler.
    let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test-light"),
        size: std::mem::size_of::<crate::light::LightUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &light_buffer,
        0,
        bytemuck::bytes_of(&light.light_uniform(light_vp)),
    );
    let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-shadow"),
        size: wgpu::Extent3d {
            width: SHADOW_RES,
            height: SHADOW_RES,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: depth_format.to_wgpu(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let shadow_view = shadow_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("test-shadow-sampler"),
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    let light_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("test-light-bg"),
        layout: &layouts.light,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&shadow_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&shadow_sampler),
            },
        ],
    });

    let color_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-color"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: color_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-depth"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: depth_format.to_wgpu(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let shadow_pipeline = cache.get(&shadow_key).expect("shadow pipeline cached");
    let lit_pipeline = cache.get(&lit_key).expect("lit pipeline cached");

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

    // Shadow pass: render the occluder depth-only from the light camera.
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("test-shadow-pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &shadow_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(shadow_pipeline);
        pass.set_bind_group(0, &camera_bg, &[0, 0]);
        pass.set_vertex_buffer(0, occluder.vertex_buffer().slice(..));
        pass.set_index_buffer(occluder.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..occluder.index_count(), 0, 0..1);
    }

    // Lit pass: render the ground from the top-down camera, shadowed by the map.
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("test-lit-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(lit_pipeline);
        pass.set_bind_group(2, &light_bg, &[]);
        pass.set_bind_group(0, &camera_bg, &[camera_stride as u32, model_stride as u32]);
        pass.set_bind_group(1, &material_bg, &[]);
        pass.set_vertex_buffer(0, ground.vertex_buffer().slice(..));
        pass.set_index_buffer(ground.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..ground.index_count(), 0, 0..1);
    }

    // Read the color target back. W*4 = 256, already a multiple of 256.
    let bytes_per_row = W * 4;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test-readback"),
        size: (bytes_per_row * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &color_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &readback,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().expect("map channel").expect("map readback");
    let data = readback.slice(..).get_mapped_range();

    let red_at = |col: u32, row: u32| -> u8 {
        let i = (row * bytes_per_row + col * 4) as usize;
        data[i]
    };
    let center = red_at(W / 2, H / 2);
    let corner = red_at(6, 6);

    eprintln!("gpu_test: shadowed center red={center}, lit corner red={corner}");
    assert!(
        (corner as i32) - (center as i32) > 40,
        "ground under the occluder ({center}) must be clearly darker than open ground ({corner})"
    );
}
