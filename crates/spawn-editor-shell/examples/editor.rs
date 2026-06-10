//! A minimal Spawn editor scene: three reflected cubes a user can select, move
//! with the gizmo, edit in the inspector, and preview with Play/Stop.

use spawn_asset::AssetId;
use spawn_core::{Transform3D, Vec3};
use spawn_editor_shell::{EditorApp, EditorConfig, Renderable, ShellResult};
use spawn_render::{
    CompareFn, CullMode, Material, MaterialUniform, Mesh, RenderStateKey, ShaderHandle, Topology,
    Vertex,
};

fn cube_id() -> AssetId {
    AssetId::from_raw(1)
}
fn material_id() -> AssetId {
    AssetId::from_raw(2)
}

fn main() -> ShellResult<()> {
    EditorApp::run(
        EditorConfig::new()
            .with_world_setup(|world| {
                // Reflect Transform3D so the inspector can edit it.
                world.register_reflect::<Transform3D>();
                for x in [-2.5, 0.0, 2.5] {
                    world.spawn_with((
                        Transform3D::from_translation(Vec3::new(x, 0.0, 0.0)),
                        Renderable {
                            mesh: cube_id(),
                            material: material_id(),
                        },
                    ));
                }
            })
            .with_render_setup(|renderer, resources| {
                let (vertices, indices) = cube();
                let mesh = Mesh::new(renderer.device(), &vertices, &indices)?;
                // The lit pass uses the built-in lit pipeline; the material only
                // supplies group 1, so the placeholder shader/state are unused.
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
                    ShaderHandle::from_id(AssetId::from_raw(100)),
                    MaterialUniform {
                        base_color: [0.7, 0.7, 0.75, 1.0],
                        params: [0.0; 4],
                    },
                    None,
                    state,
                )?;
                resources.insert_mesh(cube_id(), mesh);
                resources.insert_material(material_id(), material);
                Ok(())
            }),
    )
}

/// A unit cube with per-face normals (24 vertices, 36 indices).
fn cube() -> (Vec<Vertex>, Vec<u32>) {
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        (
            [0.0, 0.0, 1.0],
            [
                [-0.5, -0.5, 0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5],
                [-0.5, 0.5, 0.5],
            ],
        ),
        (
            [0.0, 0.0, -1.0],
            [
                [0.5, -0.5, -0.5],
                [-0.5, -0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [0.5, 0.5, -0.5],
            ],
        ),
        (
            [1.0, 0.0, 0.0],
            [
                [0.5, -0.5, 0.5],
                [0.5, -0.5, -0.5],
                [0.5, 0.5, -0.5],
                [0.5, 0.5, 0.5],
            ],
        ),
        (
            [-1.0, 0.0, 0.0],
            [
                [-0.5, -0.5, -0.5],
                [-0.5, -0.5, 0.5],
                [-0.5, 0.5, 0.5],
                [-0.5, 0.5, -0.5],
            ],
        ),
        (
            [0.0, 1.0, 0.0],
            [
                [-0.5, 0.5, 0.5],
                [0.5, 0.5, 0.5],
                [0.5, 0.5, -0.5],
                [-0.5, 0.5, -0.5],
            ],
        ),
        (
            [0.0, -1.0, 0.0],
            [
                [-0.5, -0.5, -0.5],
                [0.5, -0.5, -0.5],
                [0.5, -0.5, 0.5],
                [-0.5, -0.5, 0.5],
            ],
        ),
    ];
    let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (normal, corners) in faces {
        let base = vertices.len() as u32;
        for (i, position) in corners.iter().enumerate() {
            vertices.push(Vertex {
                position: *position,
                normal,
                uv: uvs[i],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (vertices, indices)
}
