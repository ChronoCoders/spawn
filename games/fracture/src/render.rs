use std::f32::consts::PI;

use spawn_asset::AssetId;
use spawn_core::{Color, Mat4, Transform3D, Vec3};
use spawn_ecs::{Commands, EcsResult, World};
use spawn_engine::{
    DirectionalLight, EngineResult, Lighting, RenderProxies, RenderProxy, RenderResources,
    Renderer, ShadowConfig,
};
use spawn_render::{
    CompareFn, CullMode, Material, MaterialUniform, Mesh, RenderStateKey, ShaderHandle, Topology,
    Vertex,
};

use crate::components::{Brick, BrickKind, PowerUpKind, Renderable};
use crate::field;

pub fn cuboid_mesh() -> AssetId {
    AssetId::from_canonical_path("fracture/mesh/cuboid")
}

pub fn ball_mesh() -> AssetId {
    AssetId::from_canonical_path("fracture/mesh/ball")
}

pub fn quad_mesh() -> AssetId {
    AssetId::from_canonical_path("fracture/mesh/quad")
}

pub fn paddle_material() -> AssetId {
    AssetId::from_canonical_path("fracture/mat/paddle")
}

pub fn ball_material() -> AssetId {
    AssetId::from_canonical_path("fracture/mat/ball")
}

pub fn wall_material() -> AssetId {
    AssetId::from_canonical_path("fracture/mat/wall")
}

pub fn back_plane_material() -> AssetId {
    AssetId::from_canonical_path("fracture/mat/back-plane")
}

pub fn powerup_material(kind: PowerUpKind) -> AssetId {
    AssetId::from_canonical_path(match kind {
        PowerUpKind::WidenPaddle => "fracture/mat/powerup/widen",
        PowerUpKind::MultiBall => "fracture/mat/powerup/multi",
        PowerUpKind::SlowBall => "fracture/mat/powerup/slow",
        PowerUpKind::ExtraLife => "fracture/mat/powerup/life",
    })
}

pub fn brick_material(kind: BrickKind, health: u8) -> AssetId {
    AssetId::from_canonical_path(match (kind, health) {
        (BrickKind::Normal, _) => "fracture/mat/brick/normal",
        (BrickKind::Tough, 2) => "fracture/mat/brick/tough-2",
        (BrickKind::Tough, _) => "fracture/mat/brick/tough-1",
        (BrickKind::Reinforced, 3) => "fracture/mat/brick/reinforced-3",
        (BrickKind::Reinforced, 2) => "fracture/mat/brick/reinforced-2",
        (BrickKind::Reinforced, _) => "fracture/mat/brick/reinforced-1",
        (BrickKind::Solid, _) => "fracture/mat/brick/solid",
    })
}

pub fn back_plane_renderable() -> Renderable {
    Renderable {
        mesh: quad_mesh(),
        material: back_plane_material(),
    }
}

pub fn wall_renderable() -> Renderable {
    Renderable {
        mesh: cuboid_mesh(),
        material: wall_material(),
    }
}

pub fn paddle_renderable() -> Renderable {
    Renderable {
        mesh: cuboid_mesh(),
        material: paddle_material(),
    }
}

pub fn ball_renderable() -> Renderable {
    Renderable {
        mesh: ball_mesh(),
        material: ball_material(),
    }
}

pub fn powerup_renderable(kind: PowerUpKind) -> Renderable {
    Renderable {
        mesh: cuboid_mesh(),
        material: powerup_material(kind),
    }
}

fn face(out: &mut Vec<Vertex>, indices: &mut Vec<u32>, corners: [[f32; 3]; 4], normal: [f32; 3]) {
    let base = out.len() as u32;
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    for (position, uv) in corners.into_iter().zip(uvs) {
        out.push(Vertex {
            position,
            normal,
            uv,
        });
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn cuboid_geometry() -> (Vec<Vertex>, Vec<u32>) {
    let h = 0.5;
    let mut verts = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    face(
        &mut verts,
        &mut indices,
        [[-h, -h, h], [h, -h, h], [h, h, h], [-h, h, h]],
        [0.0, 0.0, 1.0],
    );
    face(
        &mut verts,
        &mut indices,
        [[h, -h, -h], [-h, -h, -h], [-h, h, -h], [h, h, -h]],
        [0.0, 0.0, -1.0],
    );
    face(
        &mut verts,
        &mut indices,
        [[h, -h, h], [h, -h, -h], [h, h, -h], [h, h, h]],
        [1.0, 0.0, 0.0],
    );
    face(
        &mut verts,
        &mut indices,
        [[-h, -h, -h], [-h, -h, h], [-h, h, h], [-h, h, -h]],
        [-1.0, 0.0, 0.0],
    );
    face(
        &mut verts,
        &mut indices,
        [[-h, h, h], [h, h, h], [h, h, -h], [-h, h, -h]],
        [0.0, 1.0, 0.0],
    );
    face(
        &mut verts,
        &mut indices,
        [[-h, -h, -h], [h, -h, -h], [h, -h, h], [-h, -h, h]],
        [0.0, -1.0, 0.0],
    );
    (verts, indices)
}

fn quad_geometry() -> (Vec<Vertex>, Vec<u32>) {
    let h = 0.5;
    let normal = [0.0, 0.0, 1.0];
    let verts = vec![
        Vertex {
            position: [-h, -h, 0.0],
            normal,
            uv: [0.0, 0.0],
        },
        Vertex {
            position: [h, -h, 0.0],
            normal,
            uv: [1.0, 0.0],
        },
        Vertex {
            position: [h, h, 0.0],
            normal,
            uv: [1.0, 1.0],
        },
        Vertex {
            position: [-h, h, 0.0],
            normal,
            uv: [0.0, 1.0],
        },
    ];
    (verts, vec![0, 1, 2, 0, 2, 3])
}

fn sphere_geometry() -> (Vec<Vertex>, Vec<u32>) {
    let radius = 0.5;
    let stacks = 12u32;
    let slices = 16u32;
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for i in 0..=stacks {
        let phi = PI * (i as f32) / (stacks as f32);
        let (sin_phi, cos_phi) = phi.sin_cos();
        for j in 0..=slices {
            let theta = 2.0 * PI * (j as f32) / (slices as f32);
            let (sin_theta, cos_theta) = theta.sin_cos();
            let normal = [sin_phi * cos_theta, cos_phi, sin_phi * sin_theta];
            verts.push(Vertex {
                position: [normal[0] * radius, normal[1] * radius, normal[2] * radius],
                normal,
                uv: [(j as f32) / (slices as f32), (i as f32) / (stacks as f32)],
            });
        }
    }
    let ring = slices + 1;
    for i in 0..stacks {
        for j in 0..slices {
            let first = i * ring + j;
            let second = first + ring;
            indices.extend_from_slice(&[first, first + 1, second, first + 1, second + 1, second]);
        }
    }
    (verts, indices)
}

fn lit_material(renderer: &Renderer, color: Color) -> EngineResult<Material> {
    let shader = ShaderHandle::from_id(AssetId::from_canonical_path("fracture/lit-placeholder"));
    let state = RenderStateKey {
        color_format: renderer.surface_format(),
        depth_format: renderer.depth_format(),
        depth_compare: CompareFn::Less,
        depth_write: true,
        cull: CullMode::Back,
        topology: Topology::TriangleList,
    };
    Ok(Material::new(
        renderer,
        shader,
        MaterialUniform {
            base_color: [color.r, color.g, color.b, color.a],
            params: [0.0; 4],
        },
        None,
        state,
    )?)
}

pub fn setup(renderer: &mut Renderer, resources: &mut RenderResources) -> EngineResult<()> {
    let (cv, ci) = cuboid_geometry();
    resources.insert_mesh(cuboid_mesh(), Mesh::new(renderer.device(), &cv, &ci)?);
    let (bv, bi) = sphere_geometry();
    resources.insert_mesh(ball_mesh(), Mesh::new(renderer.device(), &bv, &bi)?);
    let (qv, qi) = quad_geometry();
    resources.insert_mesh(quad_mesh(), Mesh::new(renderer.device(), &qv, &qi)?);

    let materials = [
        (paddle_material(), Color::rgb(0.2, 0.9, 0.9)),
        (ball_material(), Color::rgb(0.95, 0.95, 1.0)),
        (wall_material(), Color::rgb(0.3, 0.3, 0.35)),
        (back_plane_material(), Color::rgb(0.08, 0.08, 0.13)),
        (
            powerup_material(PowerUpKind::WidenPaddle),
            Color::rgb(0.2, 0.9, 0.3),
        ),
        (
            powerup_material(PowerUpKind::MultiBall),
            Color::rgb(0.95, 0.85, 0.2),
        ),
        (
            powerup_material(PowerUpKind::SlowBall),
            Color::rgb(0.3, 0.6, 0.95),
        ),
        (
            powerup_material(PowerUpKind::ExtraLife),
            Color::rgb(0.95, 0.4, 0.6),
        ),
        (
            brick_material(BrickKind::Normal, 1),
            Color::rgb(1.0, 0.5, 0.2),
        ),
        (
            brick_material(BrickKind::Tough, 2),
            Color::rgb(0.2, 0.45, 0.9),
        ),
        (
            brick_material(BrickKind::Tough, 1),
            Color::rgb(0.55, 0.7, 1.0),
        ),
        (
            brick_material(BrickKind::Reinforced, 3),
            Color::rgb(0.55, 0.2, 0.7),
        ),
        (
            brick_material(BrickKind::Reinforced, 2),
            Color::rgb(0.72, 0.4, 0.82),
        ),
        (
            brick_material(BrickKind::Reinforced, 1),
            Color::rgb(0.88, 0.62, 0.93),
        ),
        (
            brick_material(BrickKind::Solid, 0),
            Color::rgb(0.45, 0.45, 0.5),
        ),
    ];
    for (id, color) in materials {
        resources.insert_material(id, lit_material(renderer, color)?);
    }
    Ok(())
}

fn view() -> Mat4 {
    Mat4::look_at_rh(
        Vec3::new(0.0, 0.0, 40.0),
        Vec3::ZERO,
        Vec3::new(0.0, 1.0, 0.0),
    )
    .unwrap_or(Mat4::IDENTITY)
}

fn projection() -> Mat4 {
    let half_w = field::HALF_WIDTH * 1.1;
    let half_h = field::HALF_HEIGHT * 1.1;
    Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, 1.0, 100.0).unwrap_or(Mat4::IDENTITY)
}

fn lighting() -> Lighting {
    Lighting {
        directional: DirectionalLight {
            direction: Vec3::new(-0.3, -0.4, -1.0),
            color: Color::WHITE,
            intensity: 1.0,
            ambient: Color::new(0.22, 0.22, 0.26, 1.0),
            shadow: ShadowConfig {
                center: Vec3::ZERO,
                extent: field::FIELD_HEIGHT,
                near: 0.1,
                far: 120.0,
                resolution: 2048,
                depth_bias: 0.003,
            },
        },
    }
}

pub fn extract(world: &World, proxies: &mut RenderProxies) {
    proxies.camera.view = view();
    proxies.camera.projection = projection();
    proxies.lighting = lighting();
    for (transform, renderable) in world.query::<(&Transform3D, &Renderable)>().iter() {
        proxies.draws.push(RenderProxy {
            model: transform.to_mat4(),
            mesh: renderable.mesh,
            material: renderable.material,
        });
    }
    for (transform, brick) in world.query::<(&Transform3D, &Brick)>().iter() {
        proxies.draws.push(RenderProxy {
            model: transform.to_mat4(),
            mesh: cuboid_mesh(),
            material: brick_material(brick.kind, brick.health),
        });
    }
}

pub fn spawn_back_plane(commands: &mut Commands<'_>) -> EcsResult<()> {
    let transform = Transform3D {
        translation: Vec3::new(0.0, 0.0, field::BACK_PLANE_Z),
        rotation: spawn_core::Quat::IDENTITY,
        scale: Vec3::new(field::FIELD_WIDTH * 2.0, field::FIELD_HEIGHT * 2.0, 1.0),
    };
    commands.spawn_with((
        transform,
        crate::components::BackPlane,
        back_plane_renderable(),
    ));
    Ok(())
}
