//! Windowed example: the falling-body simulation rendered by the wgpu backend's
//! lit graph. A render-setup hook builds a ground plane and a small occluder
//! (each a lit material); the extract routine emits a perspective camera, one
//! directional light, and a proxy per drawable. The shadow pass renders the
//! occluder into the shadow map and the lit pass shades the ground with Lambert +
//! ambient + PCF shadowing, so the falling body casts a moving shadow. Closes the
//! 2a clear-only path on-target.
//!
//! Run with `cargo run -p spawn-engine --example windowed` on a desktop session
//! (it needs a display server).

use std::error::Error;

use spawn_asset::AssetId;
use spawn_core::{Color, Mat4, Transform3D, Vec3};
use spawn_ecs::{Commands, Query, Res, World};
use spawn_engine::{
    App, DirectionalLight, EngineConfig, EngineResult, Lighting, RenderProxies, RenderProxy,
    RenderResources, Renderer, ScheduleLabel, ShadowConfig, Time, WindowConfig,
};
use spawn_physics::ecs::{run_physics_fixed_update, Collider, PhysicsSyncState, RigidBody};
use spawn_physics::physics3d::{ColliderDesc, PhysicsConfig, PhysicsWorld, RigidBodyDesc, Shape};
use spawn_physics::CollisionEvent;
use spawn_render::{
    CompareFn, CullMode, Material, MaterialUniform, Mesh, RenderStateKey, ShaderHandle, Topology,
    Vertex,
};

fn ground_mesh_id() -> AssetId {
    AssetId::from_canonical_path("ground-mesh")
}

fn occluder_mesh_id() -> AssetId {
    AssetId::from_canonical_path("occluder-mesh")
}

fn ground_material_id() -> AssetId {
    AssetId::from_canonical_path("ground-material")
}

fn occluder_material_id() -> AssetId {
    AssetId::from_canonical_path("occluder-material")
}

/// A horizontal quad of half-size `half` centered at the origin (y = 0), facing
/// up. Used for both the ground and the occluder (the occluder is positioned by
/// its draw's model matrix).
fn horizontal_quad(renderer: &Renderer, half: f32) -> EngineResult<Mesh> {
    let n = [0.0, 1.0, 0.0];
    let verts = [
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
    let mesh = Mesh::new(renderer.device(), &verts, &[0, 1, 2, 0, 2, 3])?;
    Ok(mesh)
}

/// Builds a lit material with `base_color`. In the lit graph the material's
/// recorded pipeline key is unused (the lit pass uses the renderer's built-in lit
/// pipeline); the material only supplies group 1 (color uniform + fallback
/// texture), so the placeholder shader/state here never reach the pipeline cache.
fn lit_material(renderer: &Renderer, base_color: [f32; 4]) -> EngineResult<Material> {
    let shader = ShaderHandle::from_id(AssetId::from_canonical_path("lit-material-placeholder"));
    let state = RenderStateKey {
        color_format: renderer.surface_format(),
        depth_format: renderer.depth_format(),
        depth_compare: CompareFn::Less,
        depth_write: true,
        cull: CullMode::Back,
        topology: Topology::TriangleList,
    };
    let material = Material::new(
        renderer,
        shader,
        MaterialUniform {
            base_color,
            params: [0.0; 4],
        },
        None,
        state,
    )?;
    Ok(material)
}

fn register_scene(renderer: &mut Renderer, resources: &mut RenderResources) -> EngineResult<()> {
    let ground = horizontal_quad(renderer, 20.0)?;
    let occluder = horizontal_quad(renderer, 1.5)?;
    resources.insert_mesh(ground_mesh_id(), ground);
    resources.insert_mesh(occluder_mesh_id(), occluder);
    resources.insert_material(
        ground_material_id(),
        lit_material(renderer, [0.6, 0.6, 0.62, 1.0])?,
    );
    resources.insert_material(
        occluder_material_id(),
        lit_material(renderer, [0.3, 0.7, 1.0, 1.0])?,
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let fixed_dt = 1.0 / 60.0;

    let mut app = App::new();
    app.set_config(EngineConfig {
        fixed_timestep: fixed_dt,
        render_thread: true,
        window: WindowConfig::default().with_title("Spawn Engine 3c"),
        ..Default::default()
    });

    let start = Transform3D::from_translation(Vec3::new(0.0, 6.0, 0.0));
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

    app.add_render_setup(register_scene);

    let view = Mat4::look_at_rh(
        Vec3::new(7.0, 7.0, 11.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    )
    .unwrap_or(Mat4::IDENTITY);
    let projection = Mat4::perspective_rh(1.0, 16.0 / 9.0, 0.1, 200.0).unwrap_or(Mat4::IDENTITY);
    let lighting = Lighting {
        directional: DirectionalLight {
            direction: Vec3::new(-0.4, -1.0, -0.3),
            color: Color::WHITE,
            intensity: 1.0,
            ambient: Color::new(0.15, 0.15, 0.18, 1.0),
            shadow: ShadowConfig {
                center: Vec3::ZERO,
                extent: 24.0,
                near: 0.1,
                far: 80.0,
                resolution: 2048,
                depth_bias: 0.003,
            },
        },
    };
    app.add_extract(move |world: &World, proxies: &mut RenderProxies| {
        proxies.camera.view = view;
        proxies.camera.projection = projection;
        proxies.lighting = lighting;
        // The ground sits at the origin; its mesh is already world-sized.
        proxies.draws.push(RenderProxy {
            model: Mat4::IDENTITY,
            mesh: ground_mesh_id(),
            material: ground_material_id(),
        });
        for transform in world.query::<&Transform3D>().iter() {
            proxies.draws.push(RenderProxy {
                model: transform.to_mat4(),
                mesh: occluder_mesh_id(),
                material: occluder_material_id(),
            });
        }
    });

    app.run()?;
    Ok(())
}
