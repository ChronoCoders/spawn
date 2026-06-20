//! Built-in engine WGSL: the depth-only shadow caster and the Lambert-plus-
//! ambient, PCF-shadowed lit forward shader. Both are compiled once by the
//! renderer at construction (never mid-frame). Both consume the fixed
//! position/normal/uv vertex layout and the shared group 0 (camera, model)
//! bindings; the lit shader also binds group 1 (material) and group 2 (light,
//! shadow map, comparison sampler).

/// Depth-only shadow caster. Transforms positions by the light view-projection
/// (written into the shadow pass's per-pass camera slot) times the model matrix;
/// no fragment stage. `normal`/`uv` are declared to match the vertex layout but
/// unused.
pub(crate) const SHADOW_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32>, view_pos: vec4<f32> };
struct Model { model: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> model: Model;

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> @builtin(position) vec4<f32> {
    return camera.view_proj * model.model * vec4<f32>(position, 1.0);
}
"#;

/// Lit forward shader: Lambert diffuse + flat ambient, modulated by a 3×3 PCF
/// shadow lookup. Fragment world position is projected into light clip space and
/// compared against the shadow map with `depth_bias`; fragments outside the light
/// frustum are treated as fully lit.
pub(crate) const LIT_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32>, view_pos: vec4<f32> };
struct Model { model: mat4x4<f32> };
struct Material { base_color: vec4<f32>, params: vec4<f32> };
struct Light {
    direction: vec4<f32>,
    color: vec4<f32>,
    ambient: vec4<f32>,
    light_view_proj: mat4x4<f32>,
    shadow_params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> model: Model;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;
@group(2) @binding(0) var<uniform> light: Light;
@group(2) @binding(1) var shadow_map: texture_depth_2d;
@group(2) @binding(2) var shadow_samp: sampler_comparison;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> VsOut {
    var out: VsOut;
    let world = model.model * vec4<f32>(position, 1.0);
    out.world_pos = world.xyz;
    out.world_normal = (model.model * vec4<f32>(normal, 0.0)).xyz;
    out.uv = uv;
    out.clip = camera.view_proj * world;
    return out;
}

fn shadow_factor(world_pos: vec3<f32>) -> f32 {
    let lp = light.light_view_proj * vec4<f32>(world_pos, 1.0);
    let ndc = lp.xyz / lp.w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 || ndc.z < 0.0) {
        return 1.0;
    }
    let texel = light.shadow_params.x;
    let bias = light.shadow_params.y;
    let radius = i32(light.shadow_params.z);
    let ref_depth = ndc.z - bias;
    var sum = 0.0;
    var count = 0.0;
    for (var dx = -radius; dx <= radius; dx = dx + 1) {
        for (var dy = -radius; dy <= radius; dy = dy + 1) {
            let off = vec2<f32>(f32(dx), f32(dy)) * texel;
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_samp, uv + off, ref_depth);
            count = count + 1.0;
        }
    }
    return sum / count;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let l = normalize(-light.direction.xyz);
    let n_dot_l = max(dot(n, l), 0.0);
    let shadow = shadow_factor(in.world_pos);
    let diffuse = light.color.rgb * light.color.w * n_dot_l * shadow;
    let base = material.base_color * textureSample(tex, samp, in.uv);
    let lit = (light.ambient.rgb + diffuse) * base.rgb;
    return vec4<f32>(lit, base.a);
}
"#;

/// Physically based forward shader: Cook-Torrance specular (GGX distribution,
/// Smith geometry, Fresnel-Schlick) plus energy-conserving Lambert diffuse, lit
/// by the analytic directional light (group 2) with the same PCF shadow lookup as
/// the lit pass. The five metallic-roughness textures are optional, gated by the
/// `texture_flags` bitmask (bit 0 base-color, 1 metallic-roughness, 2 normal,
/// 3 emissive, 4 occlusion); absent maps fall back to the scalar factors. Normal
/// mapping derives a tangent frame from screen-space derivatives, so no tangent
/// vertex attribute is required. Writes linear HDR (consumed by the tonemap pass).
pub(crate) const PBR_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32>, view_pos: vec4<f32> };
struct Model { model: mat4x4<f32> };
struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    factors: vec4<f32>,
    texture_flags: vec4<u32>,
};
struct Light {
    direction: vec4<f32>,
    color: vec4<f32>,
    ambient: vec4<f32>,
    light_view_proj: mat4x4<f32>,
    shadow_params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> model: Model;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var base_color_tex: texture_2d<f32>;
@group(1) @binding(2) var base_color_samp: sampler;
@group(1) @binding(3) var mr_tex: texture_2d<f32>;
@group(1) @binding(4) var mr_samp: sampler;
@group(1) @binding(5) var normal_tex: texture_2d<f32>;
@group(1) @binding(6) var normal_samp: sampler;
@group(1) @binding(7) var emissive_tex: texture_2d<f32>;
@group(1) @binding(8) var emissive_samp: sampler;
@group(1) @binding(9) var occlusion_tex: texture_2d<f32>;
@group(1) @binding(10) var occlusion_samp: sampler;
@group(2) @binding(0) var<uniform> light: Light;
@group(2) @binding(1) var shadow_map: texture_depth_2d;
@group(2) @binding(2) var shadow_samp: sampler_comparison;

const PI: f32 = 3.14159265359;
const FLAG_BASE: u32 = 1u;
const FLAG_MR: u32 = 2u;
const FLAG_NORMAL: u32 = 4u;
const FLAG_EMISSIVE: u32 = 8u;
const FLAG_OCCLUSION: u32 = 16u;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> VsOut {
    var out: VsOut;
    let world = model.model * vec4<f32>(position, 1.0);
    out.world_pos = world.xyz;
    out.world_normal = (model.model * vec4<f32>(normal, 0.0)).xyz;
    out.uv = uv;
    out.clip = camera.view_proj * world;
    return out;
}

fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-7);
}

fn geometry_schlick_ggx(n_dot_x: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    return n_dot_x / (n_dot_x * (1.0 - k) + k);
}

fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    return geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness);
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

fn shadow_factor(world_pos: vec3<f32>) -> f32 {
    let lp = light.light_view_proj * vec4<f32>(world_pos, 1.0);
    let ndc = lp.xyz / lp.w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 || ndc.z < 0.0) {
        return 1.0;
    }
    let texel = light.shadow_params.x;
    let bias = light.shadow_params.y;
    let radius = i32(light.shadow_params.z);
    let ref_depth = ndc.z - bias;
    var sum = 0.0;
    var count = 0.0;
    for (var dx = -radius; dx <= radius; dx = dx + 1) {
        for (var dy = -radius; dy <= radius; dy = dy + 1) {
            let off = vec2<f32>(f32(dx), f32(dy)) * texel;
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_samp, uv + off, ref_depth);
            count = count + 1.0;
        }
    }
    return sum / count;
}

fn perturb_normal(n: vec3<f32>, world_pos: vec3<f32>, uv: vec2<f32>, scale: f32) -> vec3<f32> {
    let sampled = textureSample(normal_tex, normal_samp, uv).xyz * 2.0 - vec3<f32>(1.0);
    let tangent_normal = vec3<f32>(sampled.xy * scale, sampled.z);
    let dp1 = dpdx(world_pos);
    let dp2 = dpdy(world_pos);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let inv_max = inverseSqrt(max(dot(t, t), dot(b, b)));
    let tbn = mat3x3<f32>(t * inv_max, b * inv_max, n);
    return normalize(tbn * tangent_normal);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let flags = material.texture_flags.x;

    var albedo = material.base_color;
    if ((flags & FLAG_BASE) != 0u) {
        albedo = albedo * textureSample(base_color_tex, base_color_samp, in.uv);
    }

    var metallic = material.factors.x;
    var roughness = material.factors.y;
    if ((flags & FLAG_MR) != 0u) {
        let mr = textureSample(mr_tex, mr_samp, in.uv);
        roughness = roughness * mr.g;
        metallic = metallic * mr.b;
    }
    roughness = clamp(roughness, 0.04, 1.0);
    metallic = clamp(metallic, 0.0, 1.0);

    var n = normalize(in.world_normal);
    if ((flags & FLAG_NORMAL) != 0u) {
        n = perturb_normal(n, in.world_pos, in.uv, material.factors.z);
    }

    var occlusion = 1.0;
    if ((flags & FLAG_OCCLUSION) != 0u) {
        occlusion = mix(1.0, textureSample(occlusion_tex, occlusion_samp, in.uv).r, material.factors.w);
    }

    var emissive = material.emissive.rgb;
    if ((flags & FLAG_EMISSIVE) != 0u) {
        emissive = emissive * textureSample(emissive_tex, emissive_samp, in.uv).rgb;
    }

    let v = normalize(camera.view_pos.xyz - in.world_pos);
    let l = normalize(-light.direction.xyz);
    let h = normalize(v + l);
    let n_dot_l = max(dot(n, l), 0.0);
    let n_dot_v = max(dot(n, v), 1e-4);
    let n_dot_h = max(dot(n, h), 0.0);
    let h_dot_v = max(dot(h, v), 0.0);

    let f0 = mix(vec3<f32>(0.04), albedo.rgb, metallic);
    let d = distribution_ggx(n_dot_h, roughness);
    let g = geometry_smith(n_dot_v, n_dot_l, roughness);
    let f = fresnel_schlick(h_dot_v, f0);
    let specular = (d * g * f) / max(4.0 * n_dot_v * n_dot_l, 1e-4);

    let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    let diffuse = kd * albedo.rgb / PI;

    let radiance = light.color.rgb * light.color.w;
    let shadow = shadow_factor(in.world_pos);
    let direct = (diffuse + specular) * radiance * n_dot_l * shadow;
    let ambient = light.ambient.rgb * albedo.rgb * occlusion;

    let color = ambient + direct + emissive;
    return vec4<f32>(color, albedo.a);
}
"#;

/// Minimal tonemap pass: a fullscreen triangle (positions generated from the
/// vertex index, no vertex buffer) that samples the linear HDR scene transient
/// and applies the Reinhard operator, writing the LDR sRGB surface (the sRGB
/// target encodes the linear result on store). The post-processing phase replaces
/// this with the configurable exposure/ACES chain; the operator is kept minimal
/// here so the PBR path produces a presentable frame.
pub(crate) const TONEMAP_WGSL: &str = r#"
@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var hdr_samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    out.uv = vec2<f32>(x, y);
    out.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let hdr = textureSample(hdr_tex, hdr_samp, in.uv).rgb;
    let mapped = hdr / (hdr + vec3<f32>(1.0));
    return vec4<f32>(mapped, 1.0);
}
"#;

/// Overlay UI shader: clip-space quads (positions are built in NDC on the CPU)
/// textured by a group-0 texture+sampler and tinted by a per-vertex color. Solid
/// rects/borders sample a 1×1 white texture (so the result is the color); text
/// samples the glyph atlas (a coverage mask) so the color shows through set
/// pixels. Alpha-blended, no depth.
pub(crate) const OVERLAY_UI_WGSL: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color * textureSample(tex, samp, in.uv);
}
"#;

/// Overlay line shader: world-space line vertices projected by the scene camera
/// (group 0, binding 0; the model binding the camera group also carries is
/// declared in the layout but unused here), colored per vertex. Alpha-blended, no
/// depth, so gizmo handles and selection outlines draw on top.
pub(crate) const OVERLAY_LINE_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32>, view_pos: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(position, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// Instanced unlit forward shader: reads the per-instance model + tint from the
/// group-2 storage buffer at `@builtin(instance_index)`. Group 0 is camera (the
/// model uniform binding is declared in the shared layout but unused here), group
/// 1 the material (uniform + texture). One `draw_indexed(.., 0..N)` shades N
/// instances; `tint == [1,1,1,1]` reproduces the non-instanced result.
pub(crate) const INSTANCED_OPAQUE_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32>, view_pos: vec4<f32> };
struct Material { base_color: vec4<f32>, params: vec4<f32> };
struct Instance { model: mat4x4<f32>, tint: vec4<f32> };

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;
@group(2) @binding(0) var<storage, read> instances: array<Instance>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(instance_index) ii: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * instances[ii].model * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.tint = instances[ii].tint;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return material.base_color * in.tint * textureSample(tex, samp, in.uv);
}
"#;

/// Instanced depth-only shadow caster: reads the per-instance model from the
/// group-1 storage buffer at `@builtin(instance_index)`. No fragment stage.
pub(crate) const INSTANCED_SHADOW_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32>, view_pos: vec4<f32> };
struct Instance { model: mat4x4<f32>, tint: vec4<f32> };

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<storage, read> instances: array<Instance>;

@vertex
fn vs_main(
    @builtin(instance_index) ii: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> @builtin(position) vec4<f32> {
    return camera.view_proj * instances[ii].model * vec4<f32>(position, 1.0);
}
"#;

/// Instanced physically based shader: the [`PBR_WGSL`] Cook-Torrance fragment with
/// the per-instance model + tint read from the group-3 storage buffer at
/// `@builtin(instance_index)`. Groups 0/1/2 are camera / PBR material / light as
/// in the non-instanced PBR pass; group 3 is the instance storage.
pub(crate) const INSTANCED_PBR_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32>, view_pos: vec4<f32> };
struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    factors: vec4<f32>,
    texture_flags: vec4<u32>,
};
struct Light {
    direction: vec4<f32>,
    color: vec4<f32>,
    ambient: vec4<f32>,
    light_view_proj: mat4x4<f32>,
    shadow_params: vec4<f32>,
};
struct Instance { model: mat4x4<f32>, tint: vec4<f32> };

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var base_color_tex: texture_2d<f32>;
@group(1) @binding(2) var base_color_samp: sampler;
@group(1) @binding(3) var mr_tex: texture_2d<f32>;
@group(1) @binding(4) var mr_samp: sampler;
@group(1) @binding(5) var normal_tex: texture_2d<f32>;
@group(1) @binding(6) var normal_samp: sampler;
@group(1) @binding(7) var emissive_tex: texture_2d<f32>;
@group(1) @binding(8) var emissive_samp: sampler;
@group(1) @binding(9) var occlusion_tex: texture_2d<f32>;
@group(1) @binding(10) var occlusion_samp: sampler;
@group(2) @binding(0) var<uniform> light: Light;
@group(2) @binding(1) var shadow_map: texture_depth_2d;
@group(2) @binding(2) var shadow_samp: sampler_comparison;
@group(3) @binding(0) var<storage, read> instances: array<Instance>;

const PI: f32 = 3.14159265359;
const FLAG_BASE: u32 = 1u;
const FLAG_MR: u32 = 2u;
const FLAG_NORMAL: u32 = 4u;
const FLAG_EMISSIVE: u32 = 8u;
const FLAG_OCCLUSION: u32 = 16u;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tint: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(instance_index) ii: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> VsOut {
    var out: VsOut;
    let model = instances[ii].model;
    let world = model * vec4<f32>(position, 1.0);
    out.world_pos = world.xyz;
    out.world_normal = (model * vec4<f32>(normal, 0.0)).xyz;
    out.uv = uv;
    out.tint = instances[ii].tint;
    out.clip = camera.view_proj * world;
    return out;
}

fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-7);
}

fn geometry_schlick_ggx(n_dot_x: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    return n_dot_x / (n_dot_x * (1.0 - k) + k);
}

fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    return geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness);
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

fn shadow_factor(world_pos: vec3<f32>) -> f32 {
    let lp = light.light_view_proj * vec4<f32>(world_pos, 1.0);
    let ndc = lp.xyz / lp.w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 || ndc.z < 0.0) {
        return 1.0;
    }
    let texel = light.shadow_params.x;
    let bias = light.shadow_params.y;
    let radius = i32(light.shadow_params.z);
    let ref_depth = ndc.z - bias;
    var sum = 0.0;
    var count = 0.0;
    for (var dx = -radius; dx <= radius; dx = dx + 1) {
        for (var dy = -radius; dy <= radius; dy = dy + 1) {
            let off = vec2<f32>(f32(dx), f32(dy)) * texel;
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_samp, uv + off, ref_depth);
            count = count + 1.0;
        }
    }
    return sum / count;
}

fn perturb_normal(n: vec3<f32>, world_pos: vec3<f32>, uv: vec2<f32>, scale: f32) -> vec3<f32> {
    let sampled = textureSample(normal_tex, normal_samp, uv).xyz * 2.0 - vec3<f32>(1.0);
    let tangent_normal = vec3<f32>(sampled.xy * scale, sampled.z);
    let dp1 = dpdx(world_pos);
    let dp2 = dpdy(world_pos);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let inv_max = inverseSqrt(max(dot(t, t), dot(b, b)));
    let tbn = mat3x3<f32>(t * inv_max, b * inv_max, n);
    return normalize(tbn * tangent_normal);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let flags = material.texture_flags.x;

    var albedo = material.base_color * in.tint;
    if ((flags & FLAG_BASE) != 0u) {
        albedo = albedo * textureSample(base_color_tex, base_color_samp, in.uv);
    }

    var metallic = material.factors.x;
    var roughness = material.factors.y;
    if ((flags & FLAG_MR) != 0u) {
        let mr = textureSample(mr_tex, mr_samp, in.uv);
        roughness = roughness * mr.g;
        metallic = metallic * mr.b;
    }
    roughness = clamp(roughness, 0.04, 1.0);
    metallic = clamp(metallic, 0.0, 1.0);

    var n = normalize(in.world_normal);
    if ((flags & FLAG_NORMAL) != 0u) {
        n = perturb_normal(n, in.world_pos, in.uv, material.factors.z);
    }

    var occlusion = 1.0;
    if ((flags & FLAG_OCCLUSION) != 0u) {
        occlusion = mix(1.0, textureSample(occlusion_tex, occlusion_samp, in.uv).r, material.factors.w);
    }

    var emissive = material.emissive.rgb;
    if ((flags & FLAG_EMISSIVE) != 0u) {
        emissive = emissive * textureSample(emissive_tex, emissive_samp, in.uv).rgb;
    }

    let v = normalize(camera.view_pos.xyz - in.world_pos);
    let l = normalize(-light.direction.xyz);
    let h = normalize(v + l);
    let n_dot_l = max(dot(n, l), 0.0);
    let n_dot_v = max(dot(n, v), 1e-4);
    let n_dot_h = max(dot(n, h), 0.0);
    let h_dot_v = max(dot(h, v), 0.0);

    let f0 = mix(vec3<f32>(0.04), albedo.rgb, metallic);
    let d = distribution_ggx(n_dot_h, roughness);
    let g = geometry_smith(n_dot_v, n_dot_l, roughness);
    let f = fresnel_schlick(h_dot_v, f0);
    let specular = (d * g * f) / max(4.0 * n_dot_v * n_dot_l, 1e-4);

    let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    let diffuse = kd * albedo.rgb / PI;

    let radiance = light.color.rgb * light.color.w;
    let shadow = shadow_factor(in.world_pos);
    let direct = (diffuse + specular) * radiance * n_dot_l * shadow;
    let ambient = light.ambient.rgb * albedo.rgb * occlusion;

    let color = ambient + direct + emissive;
    return vec4<f32>(color, albedo.a);
}
"#;
