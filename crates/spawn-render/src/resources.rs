//! `AssetId`-keyed GPU resource registry: resolves the engine's draw proxies
//! (`mesh: AssetId`, `material: AssetId`) to the owned `Mesh`/`Material` GPU
//! resources to draw with. Holds already-built resources: the app constructs a
//! `Mesh`/`Material` (Phase 1 constructors) from its data and registers it.
//! Mesh/material *file-format* loading is out of scope (spawn-asset/spawn-build).

use std::collections::HashMap;

use spawn_asset::AssetId;

use crate::material::Material;
use crate::mesh::Mesh;

/// GPU resources keyed by `AssetId`, resolved per frame by the draw path.
pub struct RenderResources {
    meshes: HashMap<AssetId, Mesh>,
    materials: HashMap<AssetId, Material>,
}

impl Default for RenderResources {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderResources {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            meshes: HashMap::new(),
            materials: HashMap::new(),
        }
    }

    /// Registers (or overwrites) the mesh for `id`.
    pub fn insert_mesh(&mut self, id: AssetId, mesh: Mesh) {
        self.meshes.insert(id, mesh);
    }

    /// Registers (or overwrites) the material for `id`.
    pub fn insert_material(&mut self, id: AssetId, material: Material) {
        self.materials.insert(id, material);
    }

    /// The mesh registered for `id`, or `None`.
    pub fn mesh(&self, id: AssetId) -> Option<&Mesh> {
        self.meshes.get(&id)
    }

    /// The material registered for `id`, or `None`.
    pub fn material(&self, id: AssetId) -> Option<&Material> {
        self.materials.get(&id)
    }

    /// Resolves a `(mesh, material)` id pair, or `None` if either is unregistered
    /// (the caller skips that draw, content not yet uploaded is never fatal).
    pub fn resolve(&self, mesh: AssetId, material: AssetId) -> Option<(&Mesh, &Material)> {
        Some((self.meshes.get(&mesh)?, self.materials.get(&material)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_resolves_to_none() {
        let res = RenderResources::new();
        let a = AssetId::from_canonical_path("mesh");
        let b = AssetId::from_canonical_path("material");
        assert!(res.mesh(a).is_none());
        assert!(res.material(b).is_none());
        assert!(res.resolve(a, b).is_none());
    }
}
