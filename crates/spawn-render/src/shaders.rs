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
