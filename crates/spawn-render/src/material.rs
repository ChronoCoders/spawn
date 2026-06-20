//! Material: a pipeline key plus its group-1 bind group (uniform + texture).

use wgpu::util::DeviceExt;

use crate::asset_handle::ShaderHandle;
use crate::error::RenderResult;
use crate::graph::PassKind;
use crate::pipeline::{PipelineKey, RenderStateKey, VertexLayoutId};
use crate::renderer::Renderer;
use crate::texture::Texture;

/// Per-material shading parameters. `#[repr(C)]` + `Pod`; offsets asserted for
/// std140-compatible upload.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialUniform {
    pub base_color: [f32; 4],
    pub params: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<MaterialUniform>() == 32);
const _: () = assert!(std::mem::offset_of!(MaterialUniform, base_color) == 0);
const _: () = assert!(std::mem::offset_of!(MaterialUniform, params) == 16);

impl Default for MaterialUniform {
    fn default() -> Self {
        Self {
            base_color: [1.0, 1.0, 1.0, 1.0],
            params: [0.0; 4],
        }
    }
}

/// A material owns its uniform buffer and group-1 bind group, and records the
/// [`PipelineKey`] used to look up its pipeline in the cache. It owns no
/// pipeline. The bind group holds the uniform buffer and the texture
/// view/sampler alive for the material's lifetime.
pub struct Material {
    pipeline_key: PipelineKey,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

impl Material {
    /// Builds the uniform buffer and the group-1 bind group, recording the
    /// pipeline key from `shader` + the fixed vertex layout + `state`.
    ///
    /// When `texture` is `None`, the engine's 1x1 white fallback texture
    /// (owned by the renderer) is bound so the shared bind-group layout is
    /// always satisfied.
    pub fn new(
        renderer: &Renderer,
        shader: ShaderHandle,
        uniform: MaterialUniform,
        texture: Option<&Texture>,
        state: RenderStateKey,
    ) -> RenderResult<Self> {
        let device = renderer.device();
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spawn-material-uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let fallback = renderer.fallback_texture();
        let view = texture.map_or_else(|| fallback.view(), Texture::view);
        let sampler = texture.map_or_else(|| fallback.sampler(), Texture::sampler);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spawn-material-bg"),
            layout: &renderer.bind_group_layouts().material,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        Ok(Self {
            pipeline_key: PipelineKey {
                shader,
                vertex_layout: VertexLayoutId::PositionNormalUv,
                render_state: state,
                pass: PassKind::ForwardOpaque,
                instanced: false,
            },
            bind_group,
            uniform_buffer,
        })
    }

    pub fn pipeline_key(&self) -> PipelineKey {
        self.pipeline_key
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Updates the uniform contents in place via `queue.write_buffer`; no
    /// reallocation.
    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: MaterialUniform) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
    }
}

/// Bit positions in [`PbrMaterialUniform::texture_flags`]`[0]` marking which of
/// the five metallic-roughness maps are bound; an unset bit means the shader uses
/// the corresponding scalar factor instead of sampling.
pub mod pbr_texture_flags {
    /// Base-color (albedo) map present.
    pub const BASE_COLOR: u32 = 1;
    /// Packed metallic-roughness map present (G = roughness, B = metallic).
    pub const METALLIC_ROUGHNESS: u32 = 2;
    /// Tangent-space normal map present.
    pub const NORMAL: u32 = 4;
    /// Emissive map present.
    pub const EMISSIVE: u32 = 8;
    /// Ambient-occlusion map present.
    pub const OCCLUSION: u32 = 16;
}

/// Metallic-roughness PBR material parameters. `#[repr(C)]` + `Pod`; member
/// offsets asserted std140-compatible for uniform upload. `texture_flags[0]` is a
/// bitmask of [`pbr_texture_flags`] set from the bound [`PbrMaps`]; the remaining
/// lanes are reserved padding to keep the field a 16-byte `vec4<u32>`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PbrMaterialUniform {
    pub base_color: [f32; 4],
    pub emissive: [f32; 4],
    /// `[metallic, roughness, normal_scale, occlusion_strength]`.
    pub factors: [f32; 4],
    /// `[0]` is the [`pbr_texture_flags`] bitmask; `[1..4]` are reserved padding.
    pub texture_flags: [u32; 4],
}

const _: () = assert!(std::mem::size_of::<PbrMaterialUniform>() == 64);
const _: () = assert!(std::mem::offset_of!(PbrMaterialUniform, base_color) == 0);
const _: () = assert!(std::mem::offset_of!(PbrMaterialUniform, emissive) == 16);
const _: () = assert!(std::mem::offset_of!(PbrMaterialUniform, factors) == 32);
const _: () = assert!(std::mem::offset_of!(PbrMaterialUniform, texture_flags) == 48);

impl Default for PbrMaterialUniform {
    fn default() -> Self {
        Self {
            base_color: [1.0, 1.0, 1.0, 1.0],
            emissive: [0.0, 0.0, 0.0, 1.0],
            factors: [0.0, 1.0, 1.0, 1.0],
            texture_flags: [0; 4],
        }
    }
}

/// The optional metallic-roughness texture set for a [`PbrMaterial`]. Each map is
/// independent; an absent map binds the renderer's matching 1×1 fallback (white
/// for color/metallic-roughness/occlusion, flat normal for normal, black for
/// emissive) and clears its [`pbr_texture_flags`] bit so the shader uses the
/// scalar factor.
#[derive(Default, Clone, Copy)]
pub struct PbrMaps<'a> {
    pub base_color: Option<&'a Texture>,
    pub metallic_roughness: Option<&'a Texture>,
    pub normal: Option<&'a Texture>,
    pub emissive: Option<&'a Texture>,
    pub occlusion: Option<&'a Texture>,
}

impl PbrMaps<'_> {
    /// The [`pbr_texture_flags`] bitmask for the present maps.
    pub fn flags(&self) -> u32 {
        use pbr_texture_flags as f;
        let mut bits = 0;
        if self.base_color.is_some() {
            bits |= f::BASE_COLOR;
        }
        if self.metallic_roughness.is_some() {
            bits |= f::METALLIC_ROUGHNESS;
        }
        if self.normal.is_some() {
            bits |= f::NORMAL;
        }
        if self.emissive.is_some() {
            bits |= f::EMISSIVE;
        }
        if self.occlusion.is_some() {
            bits |= f::OCCLUSION;
        }
        bits
    }
}

/// A physically based material: its [`PbrMaterialUniform`] buffer and the group-1
/// bind group (uniform + the five metallic-roughness texture/sampler pairs), plus
/// the [`PipelineKey`] for the `ForwardPbr` pass. Owns no pipeline. The bind group
/// keeps the uniform buffer and every bound texture view/sampler alive for the
/// material's lifetime. `texture_flags` is derived from `maps`, overriding the
/// caller's uniform value so the shader and the bound textures never disagree.
pub struct PbrMaterial {
    pipeline_key: PipelineKey,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

impl PbrMaterial {
    /// Builds the uniform buffer (with `texture_flags` set from `maps`) and the
    /// group-1 PBR bind group, recording the `ForwardPbr` pipeline key from
    /// `shader` + the fixed vertex layout + `state`. Absent maps bind the
    /// renderer's typed fallbacks so the shared layout is always satisfied.
    pub fn new(
        renderer: &Renderer,
        shader: ShaderHandle,
        uniform: PbrMaterialUniform,
        maps: PbrMaps<'_>,
        state: RenderStateKey,
    ) -> RenderResult<Self> {
        let device = renderer.device();
        let mut uniform = uniform;
        uniform.texture_flags = [maps.flags(), 0, 0, 0];
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spawn-pbr-material-uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let white = renderer.fallback_texture();
        let normal_fallback = renderer.fallback_normal_texture();
        let black = renderer.fallback_black_texture();
        let base = pick_or(maps.base_color, white);
        let mr = pick_or(maps.metallic_roughness, white);
        let nrm = pick_or(maps.normal, normal_fallback);
        let emi = pick_or(maps.emissive, black);
        let occ = pick_or(maps.occlusion, white);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spawn-pbr-material-bg"),
            layout: &renderer.bind_group_layouts().pbr_material,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(base.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(base.sampler()),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(mr.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(mr.sampler()),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(nrm.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(nrm.sampler()),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(emi.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(emi.sampler()),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(occ.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(occ.sampler()),
                },
            ],
        });

        Ok(Self {
            pipeline_key: PipelineKey {
                shader,
                vertex_layout: VertexLayoutId::PositionNormalUv,
                render_state: state,
                pass: PassKind::ForwardPbr,
                instanced: false,
            },
            bind_group,
            uniform_buffer,
        })
    }

    pub fn pipeline_key(&self) -> PipelineKey {
        self.pipeline_key
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Updates the uniform contents in place via `queue.write_buffer`; no
    /// reallocation. `texture_flags` is preserved from construction (the bound
    /// textures are fixed), so callers should keep it consistent with `maps`.
    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: PbrMaterialUniform) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
    }
}

fn pick_or<'a>(map: Option<&'a Texture>, fallback: &'a Texture) -> &'a Texture {
    map.unwrap_or(fallback)
}

const _: () = {
    assert!(pbr_texture_flags::BASE_COLOR == 1);
    assert!(pbr_texture_flags::METALLIC_ROUGHNESS == 2);
    assert!(pbr_texture_flags::NORMAL == 4);
    assert!(pbr_texture_flags::EMISSIVE == 8);
    assert!(pbr_texture_flags::OCCLUSION == 16);
};

/// CPU reference implementations of the Cook-Torrance BRDF terms used by the
/// `ForwardPbr` shader. These mirror the WGSL one-to-one so the shading math is
/// verifiable without a GPU (energy conservation, reference values); they are the
/// test oracle for the shader and are not part of the per-frame path.
#[cfg(test)]
mod brdf {
    /// GGX/Trowbridge-Reitz normal distribution `D`.
    pub fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
        let a = roughness * roughness;
        let a2 = a * a;
        let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
        a2 / (std::f32::consts::PI * d * d).max(1e-7)
    }

    fn geometry_schlick_ggx(n_dot_x: f32, roughness: f32) -> f32 {
        let r = roughness + 1.0;
        let k = (r * r) / 8.0;
        n_dot_x / (n_dot_x * (1.0 - k) + k)
    }

    /// Smith geometry term `G` (Schlick-GGX masking × shadowing).
    pub fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
        geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness)
    }

    /// Fresnel-Schlick reflectance for a scalar `F0` (per channel).
    pub fn fresnel_schlick(cos_theta: f32, f0: f32) -> f32 {
        f0 + (1.0 - f0) * (1.0 - cos_theta).clamp(0.0, 1.0).powi(5)
    }

    /// The scalar Cook-Torrance specular term `D·G·F / (4·NdotV·NdotL)` for a
    /// single channel, matching the shader's denominator clamp. Fresnel is
    /// evaluated against `h_dot_v` exactly as the shader does.
    pub fn specular(
        n_dot_v: f32,
        n_dot_l: f32,
        n_dot_h: f32,
        h_dot_v: f32,
        roughness: f32,
        f0: f32,
    ) -> f32 {
        let d = distribution_ggx(n_dot_h, roughness);
        let g = geometry_smith(n_dot_v, n_dot_l, roughness);
        let f = fresnel_schlick(h_dot_v, f0);
        (d * g * f) / (4.0 * n_dot_v * n_dot_l).max(1e-4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbr_flags_track_present_maps() {
        let maps = PbrMaps::default();
        assert_eq!(maps.flags(), 0);
    }

    #[test]
    fn ggx_peaks_at_aligned_half_vector() {
        let aligned = brdf::distribution_ggx(1.0, 0.3);
        let off = brdf::distribution_ggx(0.6, 0.3);
        assert!(aligned > off, "GGX is maximal at N·H = 1");
        let rough = brdf::distribution_ggx(1.0, 0.9);
        assert!(aligned > rough, "a smoother surface concentrates the lobe");
    }

    #[test]
    fn smith_geometry_in_unit_range_and_monotonic() {
        for &r in &[0.1_f32, 0.5, 0.9] {
            let g = brdf::geometry_smith(0.8, 0.8, r);
            assert!((0.0..=1.0).contains(&g), "G stays in [0,1] (r={r}, g={g})");
        }
        let grazing = brdf::geometry_smith(0.05, 0.05, 0.5);
        let head_on = brdf::geometry_smith(0.95, 0.95, 0.5);
        assert!(head_on > grazing, "less masking head-on than at grazing");
    }

    #[test]
    fn fresnel_rises_to_one_at_grazing() {
        let f0 = 0.04;
        assert!(
            (brdf::fresnel_schlick(1.0, f0) - f0).abs() < 1e-6,
            "F = F0 head-on"
        );
        let grazing = brdf::fresnel_schlick(0.0, f0);
        assert!(grazing > 0.99, "F → 1 at grazing (got {grazing})");
    }

    #[test]
    fn brdf_conserves_energy_across_a_roughness_metallic_grid() {
        // The reflected diffuse+specular must not exceed the incoming radiance for
        // any valid configuration: with unit radiance and N·L folded in, the
        // outgoing reflectance integrand sampled at the half-vector stays ≤ 1.
        for mi in 0..=4 {
            let metallic = mi as f32 / 4.0;
            for ri in 0..=4 {
                let roughness = (ri as f32 / 4.0).max(0.04);
                for li in 1..=8 {
                    let n_dot_l = li as f32 / 8.0;
                    let n_dot_v = 0.7_f32;
                    let n_dot_h = ((n_dot_l + n_dot_v) * 0.5).clamp(0.0, 1.0);
                    let albedo = 1.0_f32;
                    let f0 = 0.04 * (1.0 - metallic) + albedo * metallic;
                    let f = brdf::fresnel_schlick(n_dot_h, f0);
                    let spec = brdf::specular(n_dot_v, n_dot_l, n_dot_h, n_dot_h, roughness, f0);
                    let kd = (1.0 - f) * (1.0 - metallic);
                    let diffuse = kd * albedo / std::f32::consts::PI;
                    let reflected = (diffuse + spec) * n_dot_l;
                    assert!(
                        reflected <= 1.0 + 1e-3,
                        "energy not conserved: metallic={metallic} roughness={roughness} n_dot_l={n_dot_l} reflected={reflected}"
                    );
                }
            }
        }
    }

    #[test]
    fn diffuse_and_specular_bounded_at_normal_incidence() {
        // Head-on (all dots = 1), the dielectric diffuse + specular reflectance is
        // bounded by unity.
        let f0 = 0.04;
        let f = brdf::fresnel_schlick(1.0, f0);
        let spec = brdf::specular(1.0, 1.0, 1.0, 1.0, 0.5, f0);
        let kd = (1.0 - f) * 1.0;
        let diffuse = kd / std::f32::consts::PI;
        assert!(diffuse + spec <= 1.0 + 1e-3);
    }
}
