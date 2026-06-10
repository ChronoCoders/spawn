//! Scene rendering glue: the `Renderable` marker component and extraction of the
//! lit pass's draw list from the world (the 2b draw-proxy model).

use spawn_asset::AssetId;
use spawn_core::{Mat4, Transform3D};
use spawn_ecs::{Component, World};
use spawn_render::{DrawItem, RenderResources};

/// Marks an entity as drawable in the editor scene view: the GPU mesh/material to
/// resolve through the [`RenderResources`] registry. The host registers the
/// resources and adds this component; an unregistered id is skipped at draw time.
#[derive(Debug, Clone, Copy)]
pub struct Renderable {
    pub mesh: AssetId,
    pub material: AssetId,
}

impl Component for Renderable {}

/// Clears `out` and fills it with one [`DrawItem`] per `(Transform3D, Renderable)`
/// entity whose mesh+material resolve, building the model matrix from the
/// transform. `out` borrows `resources` (the draws reference its GPU resources);
/// it is cleared-not-freed so a steady draw count is allocation-free.
pub fn extract_draws<'a>(
    world: &World,
    resources: &'a RenderResources,
    out: &mut Vec<DrawItem<'a>>,
) {
    out.clear();
    for (transform, renderable) in world.query::<(&Transform3D, &Renderable)>().iter() {
        if let Some((mesh, material)) = resources.resolve(renderable.mesh, renderable.material) {
            out.push(DrawItem {
                mesh,
                material,
                model: Mat4::from_scale_rotation_translation(
                    transform.scale,
                    transform.rotation,
                    transform.translation,
                ),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::Vec3;

    #[test]
    fn extract_skips_unregistered_and_non_renderable() {
        let mut world = World::new();
        world.register::<Transform3D>();
        world.register::<Renderable>();
        // One renderable with unregistered ids → skipped; one bare transform → skipped.
        world.spawn_with((
            Transform3D::from_translation(Vec3::ZERO),
            Renderable {
                mesh: AssetId::from_raw(1),
                material: AssetId::from_raw(2),
            },
        ));
        world.spawn_with((Transform3D::IDENTITY,));
        let resources = RenderResources::new();
        let mut out = Vec::new();
        extract_draws(&world, &resources, &mut out);
        assert!(
            out.is_empty(),
            "unregistered ids and non-renderables are skipped"
        );
    }
}
