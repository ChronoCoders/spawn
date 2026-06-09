//! Windowed example: the same simulation as the headless example, plus the wgpu
//! backend rasterizing a registered mesh. A render-setup hook builds a triangle
//! mesh + material + pipeline; the extract routine emits a proxy referencing them
//! and a perspective camera, so the falling body is drawn. Closes the 2a
//! clear-only path on-target.
//!
//! Run with `cargo run -p spawn-engine --example windowed` on a desktop session
//! (it needs a display server).

use std::error::Error;

use spawn_asset::AssetId;
use spawn_core::{Mat4, Transform3D, Vec3};
use spawn_ecs::{Commands, Query, Res, World};
use spawn_engine::{
    App, EngineConfig, EngineResult, RenderProxies, RenderProxy, RenderResources, Renderer,
    ScheduleLabel, Time, WindowConfig,
};
use spawn_physics::ecs::{run_physics_fixed_update, Collider, PhysicsSyncState, RigidBody};
use spawn_physics::physics3d::{ColliderDesc, PhysicsConfig, PhysicsWorld, RigidBodyDesc, Shape};
use spawn_physics::CollisionEvent;
use spawn_render::{
    CompareFn, CullMode, Material, MaterialUniform, Mesh, PipelineKey, RenderStateKey,
    ShaderHandle, Topology, Vertex, VertexLayoutId,
};

const UNLIT_WGSL: &str = r#"
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

fn mesh_id() -> AssetId {
    AssetId::from_canonical_path("mesh")
}

fn material_id() -> AssetId {
    AssetId::from_canonical_path("material")
}

fn register_triangle(renderer: &mut Renderer, resources: &mut RenderResources) -> EngineResult<()> {
    let shader = ShaderHandle::from_id(AssetId::from_canonical_path("shader"));
    renderer.load_shader(shader, UNLIT_WGSL)?;
    let state = RenderStateKey {
        color_format: renderer.surface_format(),
        depth_format: renderer.depth_format(),
        depth_compare: CompareFn::Less,
        depth_write: true,
        cull: CullMode::Back,
        topology: Topology::TriangleList,
    };
    renderer.build_pipeline(PipelineKey {
        shader,
        vertex_layout: VertexLayoutId::PositionNormalUv,
        render_state: state,
    })?;

    let verts = [
        Vertex {
            position: [-0.5, -0.5, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 1.0],
        },
        Vertex {
            position: [0.5, -0.5, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [0.0, 0.5, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [0.5, 0.0],
        },
    ];
    let mesh = Mesh::new(renderer.device(), &verts, &[0, 1, 2])?;
    let material = Material::new(
        renderer,
        shader,
        MaterialUniform {
            base_color: [0.3, 0.7, 1.0, 1.0],
            params: [0.0; 4],
        },
        None,
        state,
    )?;
    resources.insert_mesh(mesh_id(), mesh);
    resources.insert_material(material_id(), material);
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let fixed_dt = 1.0 / 60.0;

    let mut app = App::new();
    app.set_config(EngineConfig {
        fixed_timestep: fixed_dt,
        window: WindowConfig::default().with_title("Spawn Engine — 2b"),
        ..Default::default()
    });

    let start = Transform3D::from_translation(Vec3::new(0.0, 3.0, 0.0));
    app.world_mut().spawn_with((
        start,
        RigidBody(RigidBodyDesc::dynamic().with_transform(start)),
        Collider(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
    ));

    let mut physics = PhysicsWorld::new(PhysicsConfig {
        fixed_timestep: fixed_dt,
        ..Default::default()
    })?;
    let mut sync = PhysicsSyncState::new();
    let mut events: Vec<CollisionEvent> = Vec::new();
    app.add_fixed_hook(move |world, _time| {
        run_physics_fixed_update(world, &mut physics, &mut sync, &mut events);
        Ok(())
    });

    app.add_system(
        ScheduleLabel::Update,
        |_q: Query<'_, &Transform3D, ()>, _time: Res<'_, Time>, _c: &mut Commands<'_>| Ok(()),
    );

    app.add_render_setup(register_triangle);

    // A perspective camera looking at the scene; the falling body draws as a quad.
    let view = Mat4::look_at_rh(
        Vec3::new(0.0, 2.0, 8.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    )
    .unwrap_or(Mat4::IDENTITY);
    let projection = Mat4::perspective_rh(1.0, 16.0 / 9.0, 0.1, 100.0).unwrap_or(Mat4::IDENTITY);
    app.add_extract(move |world: &World, proxies: &mut RenderProxies| {
        proxies.camera.view = view;
        proxies.camera.projection = projection;
        for transform in world.query::<&Transform3D>().iter() {
            proxies.draws.push(RenderProxy {
                model: transform.to_mat4(),
                mesh: mesh_id(),
                material: material_id(),
            });
        }
    });

    app.run()?;
    Ok(())
}
