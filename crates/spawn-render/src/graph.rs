//! Render graph: declared pass I/O, with execution order, transient-resource
//! lifetimes, and transient-target memory aliasing *derived* from those
//! declarations (not hand-specified). wgpu inserts all GPU barriers; the graph's
//! job is scheduling, lifetime, and aliasing only.
//!
//! Derivation is pure CPU and unit-testable without a GPU; allocation of the
//! transient pool ([`RenderGraph::compile`]) needs a device.

use spawn_core::Color;

use crate::error::{RenderError, RenderResult};
use crate::format::{SurfaceSize, TextureFormat};
use crate::renderer::Renderer;

/// Handle into a graph's resource table. `0` is always the swapchain surface and
/// `1` the renderer's primary depth buffer; transients are assigned ids from `2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceId(u32);

impl ResourceId {
    const fn index(self) -> usize {
        self.0 as usize
    }
}

const SURFACE_ID: ResourceId = ResourceId(0);
const DEPTH_ID: ResourceId = ResourceId(1);

/// Whether a transient is a color or depth texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Color,
    Depth,
}

/// How a transient texture is sized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeSpec {
    /// `dimension * num / den` of the surface dimension (full-res = `1/1`).
    SurfaceRelative { num: u32, den: u32 },
    /// Fixed dimensions (e.g. a 2048×2048 shadow map).
    Fixed { width: u32, height: u32 },
}

impl SizeSpec {
    fn resolve(self, surface: SurfaceSize) -> (u32, u32) {
        match self {
            SizeSpec::SurfaceRelative { num, den } => {
                let den = den.max(1);
                (
                    (surface.width * num / den).max(1),
                    (surface.height * num / den).max(1),
                )
            }
            SizeSpec::Fixed { width, height } => (width.max(1), height.max(1)),
        }
    }
}

/// Declaration of a transient graph resource.
#[derive(Debug, Clone, Copy)]
pub struct ResourceDesc {
    pub name: &'static str,
    pub format: TextureFormat,
    pub size: SizeSpec,
    pub kind: ResourceKind,
}

/// The pass kind, driving how the pass records. `ForwardOpaque` is the retained
/// unlit pass; `ShadowDepth` renders depth-only into the shadow map and
/// `ForwardLit` shades with Lambert + ambient + PCF shadowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PassKind {
    ForwardOpaque,
    ShadowDepth,
    ForwardLit,
    /// Physically based forward pass: Cook-Torrance shading (GGX/Smith/Fresnel +
    /// energy-conserving Lambert) with the group-2 directional light + PCF shadow,
    /// writing the linear HDR scene transient. The tonemap pass reduces it to the
    /// LDR surface.
    ForwardPbr,
    /// Fullscreen tonemap: samples the linear HDR scene transient and writes the
    /// LDR surface. No vertex buffer (the triangle is generated in the shader), no
    /// depth, no cull.
    Tonemap,
    /// 2D overlay: rasterizes a `spawn_ui` draw list (rects, borders, text,
    /// scissors) plus editor line geometry (gizmos, selection) onto the surface
    /// after the lit scene. No depth; alpha-blended; loads (composites over) the
    /// existing color.
    Overlay2D,
}

/// A color attachment a pass writes.
#[derive(Debug, Clone, Copy)]
pub struct ColorWrite {
    pub target: ResourceId,
    pub clear: Option<Color>,
}

/// A depth attachment a pass writes/tests.
#[derive(Debug, Clone, Copy)]
pub struct DepthWrite {
    pub target: ResourceId,
    pub clear: Option<f32>,
    pub write: bool,
}

/// One pass: what it reads (sampled inputs) and writes (targets).
#[derive(Debug, Clone)]
pub struct PassDesc {
    pub name: &'static str,
    pub kind: PassKind,
    pub reads: Vec<ResourceId>,
    pub color: Option<ColorWrite>,
    pub depth: Option<DepthWrite>,
}

enum ResourceEntry {
    Surface,
    PrimaryDepth,
    Transient(ResourceDesc),
}

/// Authoring graph: declare resources and passes, then
/// [`compile`](RenderGraph::compile) to derive the plan and allocate the
/// transient pool.
pub struct RenderGraph {
    resources: Vec<ResourceEntry>,
    passes: Vec<PassDesc>,
}

impl Default for RenderGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderGraph {
    /// An empty graph with the surface and primary-depth built-ins pre-registered.
    pub fn new() -> Self {
        Self {
            resources: vec![ResourceEntry::Surface, ResourceEntry::PrimaryDepth],
            passes: Vec::new(),
        }
    }

    /// The swapchain color target.
    pub fn surface(&self) -> ResourceId {
        SURFACE_ID
    }

    /// The renderer's primary depth buffer.
    pub fn primary_depth(&self) -> ResourceId {
        DEPTH_ID
    }

    /// Declares a transient texture, returning its id.
    pub fn transient(&mut self, desc: ResourceDesc) -> ResourceId {
        let id = ResourceId(self.resources.len() as u32);
        self.resources.push(ResourceEntry::Transient(desc));
        id
    }

    /// Appends a pass in declaration order.
    pub fn add_pass(&mut self, desc: PassDesc) -> &mut Self {
        self.passes.push(desc);
        self
    }

    fn transient_desc(&self, id: ResourceId) -> Option<&ResourceDesc> {
        match self.resources.get(id.index()) {
            Some(ResourceEntry::Transient(desc)) => Some(desc),
            _ => None,
        }
    }

    /// Derives execution order, transient lifetimes, and the aliasing plan — pure
    /// CPU, no GPU. Errors on a cycle, an unproduced read, or a dangling transient.
    /// Crate-private: the public surface is `compile` plus `CompiledGraph`'s
    /// memory accessors (spec §2); `plan` exists so the derivation is unit-tested
    /// without a GPU (spec §12).
    fn plan(&self, surface_size: SurfaceSize) -> RenderResult<GraphPlan> {
        let order = self.derive_order()?;
        let order_pos = invert_order(&order, self.passes.len());

        // Lifetimes + validation for every transient.
        let mut transients: Vec<TransientPlan> = Vec::new();
        for (idx, entry) in self.resources.iter().enumerate() {
            let ResourceEntry::Transient(desc) = entry else {
                continue;
            };
            let id = ResourceId(idx as u32);
            let mut first_write: Option<usize> = None;
            let mut last_read: Option<usize> = None;
            for (pass_idx, pass) in self.passes.iter().enumerate() {
                let pos = order_pos[pass_idx];
                if writes(pass, id) {
                    first_write = Some(first_write.map_or(pos, |p| p.min(pos)));
                }
                if pass.reads.contains(&id) {
                    last_read = Some(last_read.map_or(pos, |p| p.max(pos)));
                }
            }
            match (first_write, last_read) {
                (Some(w), Some(r)) => {
                    let (width, height) = desc.size.resolve(surface_size);
                    let byte_size =
                        u64::from(width) * u64::from(height) * format_bytes_per_texel(desc.format);
                    transients.push(TransientPlan {
                        resource: id,
                        width,
                        height,
                        format: desc.format,
                        kind: desc.kind,
                        lifetime: (w, r),
                        byte_size,
                        region: 0,
                    });
                }
                _ => {
                    return Err(RenderError::GraphDanglingResource {
                        resource: desc.name,
                    })
                }
            }
        }

        let (regions, transient_memory, naive_memory) = assign_aliasing(&mut transients);
        Ok(GraphPlan {
            order,
            transients,
            regions,
            transient_memory,
            naive_memory,
        })
    }

    fn derive_order(&self) -> RenderResult<Vec<usize>> {
        let n = self.passes.len();
        // Edges: a pass reading R depends on every pass writing R.
        let mut in_degree = vec![0usize; n];
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (consumer, pass) in self.passes.iter().enumerate() {
            for &read in &pass.reads {
                let producers: Vec<usize> = (0..n)
                    .filter(|&w| w != consumer && writes(&self.passes[w], read))
                    .collect();
                if producers.is_empty() {
                    return Err(RenderError::GraphResourceNotProduced {
                        resource: self
                            .transient_desc(read)
                            .map(|d| d.name)
                            .unwrap_or("surface"),
                    });
                }
                for w in producers {
                    edges[w].push(consumer);
                    in_degree[consumer] += 1;
                }
            }
        }
        // Kahn's algorithm; ties broken by declaration order (lowest index first)
        // so the order is deterministic.
        let mut order = Vec::with_capacity(n);
        let mut ready: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
        while let Some(pos) = ready
            .iter()
            .enumerate()
            .min_by_key(|(_, &p)| p)
            .map(|(i, _)| i)
        {
            let pass = ready.remove(pos);
            order.push(pass);
            for &next in &edges[pass] {
                in_degree[next] -= 1;
                if in_degree[next] == 0 {
                    ready.push(next);
                }
            }
        }
        if order.len() != n {
            return Err(RenderError::GraphCycle);
        }
        Ok(order)
    }

    /// Derives the plan and allocates the transient texture pool. Needs a device.
    pub fn compile(&self, renderer: &Renderer) -> RenderResult<CompiledGraph> {
        let size = renderer.size();
        let plan = self.plan(size)?;
        let pool = allocate_pool(renderer, &plan.regions);
        let mut resource_pool: Vec<Option<usize>> = vec![None; self.resources.len()];
        for t in &plan.transients {
            resource_pool[t.resource.index()] = Some(t.region);
        }
        // A graph with a shadow pass binds group 2 (light uniform + shadow map +
        // comparison sampler) once, here at compile/resize — never per frame. The
        // shadow map is the shadow pass's depth target in the transient pool.
        let light_bind_group = self
            .passes
            .iter()
            .find(|p| p.kind == PassKind::ShadowDepth)
            .and_then(|p| p.depth)
            .and_then(|d| resource_pool.get(d.target.index()).copied().flatten())
            .map(|region| renderer.create_light_bind_group(&pool[region].view));
        // The tonemap pass samples the HDR scene transient it reads. Its group-0
        // bind group is built here (compile/resize), never per frame.
        let tonemap_bind_group = self
            .passes
            .iter()
            .find(|p| p.kind == PassKind::Tonemap)
            .and_then(|p| p.reads.first().copied())
            .and_then(|id| resource_pool.get(id.index()).copied().flatten())
            .map(|region| renderer.create_fullscreen_bind_group(&pool[region].view));
        Ok(CompiledGraph {
            order: plan.order.clone(),
            passes: self.passes.clone(),
            pool,
            resource_pool,
            light_bind_group,
            tonemap_bind_group,
            plan,
            size,
        })
    }
}

fn writes(pass: &PassDesc, id: ResourceId) -> bool {
    pass.color.map(|c| c.target) == Some(id) || pass.depth.map(|d| d.target) == Some(id)
}

fn invert_order(order: &[usize], n: usize) -> Vec<usize> {
    let mut pos = vec![0usize; n];
    for (p, &pass) in order.iter().enumerate() {
        pos[pass] = p;
    }
    pos
}

/// The allocation requirements of one pooled memory region.
struct RegionSpec {
    width: u32,
    height: u32,
    format: TextureFormat,
    kind: ResourceKind,
    byte_size: u64,
    last_end: usize,
}

/// Greedy interval assignment: transients with identical allocation requirements
/// and disjoint lifetimes share a memory region. Sets each transient's `region`
/// and returns `(region_specs, aliased_bytes, naive_bytes)`.
fn assign_aliasing(transients: &mut [TransientPlan]) -> (Vec<RegionSpec>, u64, u64) {
    let naive: u64 = transients.iter().map(|t| t.byte_size).sum();
    // Process in lifetime-start order for the greedy assignment.
    let mut idx: Vec<usize> = (0..transients.len()).collect();
    idx.sort_by_key(|&i| transients[i].lifetime.0);
    let mut regions: Vec<RegionSpec> = Vec::new();
    for &i in &idx {
        let t = &transients[i];
        let mut placed = None;
        for (r, region) in regions.iter_mut().enumerate() {
            let same = region.width == t.width
                && region.height == t.height
                && region.format == t.format
                && region.kind == t.kind;
            // Strict: the prior occupant must be fully dead before this one is
            // first written, so two simultaneously-live transients never share.
            if same && region.last_end < t.lifetime.0 {
                region.last_end = t.lifetime.1;
                placed = Some(r);
                break;
            }
        }
        let region = placed.unwrap_or_else(|| {
            regions.push(RegionSpec {
                width: t.width,
                height: t.height,
                format: t.format,
                kind: t.kind,
                byte_size: t.byte_size,
                last_end: t.lifetime.1,
            });
            regions.len() - 1
        });
        transients[i].region = region;
    }
    let aliased: u64 = regions.iter().map(|r| r.byte_size).sum();
    (regions, aliased, naive)
}

fn format_bytes_per_texel(format: TextureFormat) -> u64 {
    format.block_copy_size(None).map(u64::from).unwrap_or(4)
}

struct PoolTexture {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

fn allocate_pool(renderer: &Renderer, regions: &[RegionSpec]) -> Vec<PoolTexture> {
    let device = renderer.device();
    let mut pool = Vec::with_capacity(regions.len());
    // One texture per region; transients sharing a region share it (disjoint
    // lifetimes make that safe). Both color and depth transients can be sampled,
    // so both carry RENDER_ATTACHMENT | TEXTURE_BINDING.
    for region in regions {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("spawn-graph-transient"),
            size: wgpu::Extent3d {
                width: region.width,
                height: region.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: region.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        pool.push(PoolTexture { texture, view });
    }
    pool
}

/// The derived plan: execution order, transient lifetimes, and the aliasing
/// memory accounting. Pure data — produced without a GPU. Crate-private; the
/// memory accounting is exposed on `CompiledGraph` (spec §2).
struct GraphPlan {
    order: Vec<usize>,
    transients: Vec<TransientPlan>,
    regions: Vec<RegionSpec>,
    transient_memory: u64,
    naive_memory: u64,
}

struct TransientPlan {
    resource: ResourceId,
    width: u32,
    height: u32,
    format: TextureFormat,
    kind: ResourceKind,
    lifetime: (usize, usize),
    byte_size: u64,
    region: usize,
}

/// A compiled graph: derived order plus the allocated transient pool. Rebuilt
/// only on resize, never per frame.
pub struct CompiledGraph {
    order: Vec<usize>,
    passes: Vec<PassDesc>,
    pool: Vec<PoolTexture>,
    resource_pool: Vec<Option<usize>>,
    light_bind_group: Option<wgpu::BindGroup>,
    tonemap_bind_group: Option<wgpu::BindGroup>,
    plan: GraphPlan,
    size: SurfaceSize,
}

impl CompiledGraph {
    pub(crate) fn order(&self) -> &[usize] {
        &self.order
    }

    pub(crate) fn pass(&self, index: usize) -> &PassDesc {
        &self.passes[index]
    }

    /// Resolves a resource id to the transient pool view, or `None` for the
    /// surface / primary depth (resolved against the live frame by the caller).
    pub(crate) fn transient_view(&self, id: ResourceId) -> Option<&wgpu::TextureView> {
        self.resource_pool
            .get(id.index())
            .copied()
            .flatten()
            .map(|region| &self.pool[region].view)
    }

    pub(crate) fn is_surface(&self, id: ResourceId) -> bool {
        id == SURFACE_ID
    }

    /// The group-2 light bind group, present when the graph has a shadow pass.
    /// Built at compile/resize and reused every frame.
    pub(crate) fn light_bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.light_bind_group.as_ref()
    }

    /// The tonemap pass's group-0 bind group (sampling the HDR scene transient),
    /// present when the graph has a tonemap pass. Built at compile/resize and
    /// reused every frame.
    pub(crate) fn tonemap_bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.tonemap_bind_group.as_ref()
    }

    /// Re-sizes surface-relative transients and re-derives the aliasing plan.
    pub fn resize(&mut self, graph: &RenderGraph, renderer: &Renderer) -> RenderResult<()> {
        let recompiled = graph.compile(renderer)?;
        *self = recompiled;
        Ok(())
    }

    /// Total transient bytes after aliasing.
    pub fn transient_memory(&self) -> u64 {
        self.plan.transient_memory
    }

    /// Total transient bytes without aliasing.
    pub fn naive_memory(&self) -> u64 {
        self.plan.naive_memory
    }

    /// The surface size this graph was compiled for.
    pub fn size(&self) -> SurfaceSize {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::TextureFormat;

    fn color(name: &'static str) -> ResourceDesc {
        ResourceDesc {
            name,
            format: TextureFormat::Rgba8Unorm,
            size: SizeSpec::SurfaceRelative { num: 1, den: 1 },
            kind: ResourceKind::Color,
        }
    }

    fn opaque(reads: Vec<ResourceId>, color: ResourceId) -> PassDesc {
        PassDesc {
            name: "p",
            kind: PassKind::ForwardOpaque,
            reads,
            color: Some(ColorWrite {
                target: color,
                clear: Some(Color::BLACK),
            }),
            depth: None,
        }
    }

    const SIZE: SurfaceSize = SurfaceSize {
        width: 256,
        height: 256,
    };

    #[test]
    fn single_surface_pass_orders_trivially() {
        let mut g = RenderGraph::new();
        g.add_pass(opaque(Vec::new(), g.surface()));
        let plan = g.plan(SIZE).unwrap();
        assert_eq!(plan.order, vec![0]);
        assert_eq!(plan.transient_memory, 0);
    }

    #[test]
    fn dependency_orders_producer_before_consumer() {
        let mut g = RenderGraph::new();
        let t = g.transient(color("gbuffer"));
        // Declared consumer-first; derivation must still order producer first.
        g.add_pass(opaque(vec![t], g.surface()));
        g.add_pass(opaque(Vec::new(), t));
        let plan = g.plan(SIZE).unwrap();
        assert_eq!(plan.order, vec![1, 0], "producer (1) before consumer (0)");
    }

    #[test]
    fn cycle_is_rejected() {
        let mut g = RenderGraph::new();
        let a = g.transient(color("a"));
        let b = g.transient(color("b"));
        g.add_pass(PassDesc {
            name: "p0",
            kind: PassKind::ForwardOpaque,
            reads: vec![b],
            color: Some(ColorWrite {
                target: a,
                clear: None,
            }),
            depth: None,
        });
        g.add_pass(PassDesc {
            name: "p1",
            kind: PassKind::ForwardOpaque,
            reads: vec![a],
            color: Some(ColorWrite {
                target: b,
                clear: None,
            }),
            depth: None,
        });
        assert!(matches!(g.plan(SIZE), Err(RenderError::GraphCycle)));
    }

    #[test]
    fn read_without_producer_is_rejected() {
        let mut g = RenderGraph::new();
        let t = g.transient(color("never-written"));
        g.add_pass(opaque(vec![t], g.surface()));
        assert!(matches!(
            g.plan(SIZE),
            Err(RenderError::GraphResourceNotProduced { .. })
        ));
    }

    #[test]
    fn written_never_read_is_dangling() {
        let mut g = RenderGraph::new();
        let t = g.transient(color("orphan"));
        g.add_pass(opaque(Vec::new(), t));
        g.add_pass(opaque(Vec::new(), g.surface()));
        assert!(matches!(
            g.plan(SIZE),
            Err(RenderError::GraphDanglingResource { .. })
        ));
    }

    #[test]
    fn disjoint_lifetimes_alias_into_one_region() {
        // p0 writes T1, p1 reads T1, p2 writes T2, p3 reads T2, p4 writes surface.
        // T1 lifetime [0,1], T2 lifetime [2,3] — disjoint, same spec → one region.
        let mut g = RenderGraph::new();
        let t1 = g.transient(color("t1"));
        let t2 = g.transient(color("t2"));
        g.add_pass(opaque(Vec::new(), t1));
        g.add_pass(opaque(vec![t1], g.surface()));
        g.add_pass(opaque(Vec::new(), t2));
        g.add_pass(opaque(vec![t2], g.surface()));
        let plan = g.plan(SIZE).unwrap();
        assert_eq!(
            plan.transient_memory * 2,
            plan.naive_memory,
            "two equal disjoint transients alias into one region (half the memory)"
        );
    }

    #[test]
    fn shadow_then_lit_derives_shadow_first() {
        // Declared lit-first; derivation must still order the shadow caster before
        // the lit pass that reads its shadow map, and size the shadow transient.
        let mut g = RenderGraph::new();
        let shadow = g.transient(ResourceDesc {
            name: "shadow-map",
            format: TextureFormat::Depth32Float,
            size: SizeSpec::Fixed {
                width: 1024,
                height: 1024,
            },
            kind: ResourceKind::Depth,
        });
        g.add_pass(PassDesc {
            name: "lit",
            kind: PassKind::ForwardLit,
            reads: vec![shadow],
            color: Some(ColorWrite {
                target: g.surface(),
                clear: Some(Color::BLACK),
            }),
            depth: Some(DepthWrite {
                target: g.primary_depth(),
                clear: Some(1.0),
                write: true,
            }),
        });
        g.add_pass(PassDesc {
            name: "shadow",
            kind: PassKind::ShadowDepth,
            reads: Vec::new(),
            color: None,
            depth: Some(DepthWrite {
                target: shadow,
                clear: Some(1.0),
                write: true,
            }),
        });
        let plan = g.plan(SIZE).unwrap();
        assert_eq!(plan.order, vec![1, 0], "shadow caster (1) before lit (0)");
        assert_eq!(
            plan.transient_memory,
            u64::from(1024u32 * 1024 * 4),
            "shadow map sized at its fixed resolution"
        );
    }

    #[test]
    fn pbr_graph_derives_shadow_then_pbr_then_tonemap() {
        // Declared out of order; derivation must order shadow → PBR → tonemap from
        // the read/write dependencies (PBR reads the shadow map and writes the HDR
        // scene; tonemap reads the HDR scene and writes the surface).
        let mut g = RenderGraph::new();
        let shadow = g.transient(ResourceDesc {
            name: "shadow-map",
            format: TextureFormat::Depth32Float,
            size: SizeSpec::Fixed {
                width: 1024,
                height: 1024,
            },
            kind: ResourceKind::Depth,
        });
        let hdr = g.transient(ResourceDesc {
            name: "scene-hdr",
            format: TextureFormat::Rgba16Float,
            size: SizeSpec::SurfaceRelative { num: 1, den: 1 },
            kind: ResourceKind::Color,
        });
        g.add_pass(PassDesc {
            name: "tonemap",
            kind: PassKind::Tonemap,
            reads: vec![hdr],
            color: Some(ColorWrite {
                target: g.surface(),
                clear: Some(Color::BLACK),
            }),
            depth: None,
        });
        g.add_pass(PassDesc {
            name: "pbr",
            kind: PassKind::ForwardPbr,
            reads: vec![shadow],
            color: Some(ColorWrite {
                target: hdr,
                clear: Some(Color::BLACK),
            }),
            depth: Some(DepthWrite {
                target: g.primary_depth(),
                clear: Some(1.0),
                write: true,
            }),
        });
        g.add_pass(PassDesc {
            name: "shadow",
            kind: PassKind::ShadowDepth,
            reads: Vec::new(),
            color: None,
            depth: Some(DepthWrite {
                target: shadow,
                clear: Some(1.0),
                write: true,
            }),
        });
        let plan = g.plan(SIZE).unwrap();
        assert_eq!(
            plan.order,
            vec![2, 1, 0],
            "shadow (2) → pbr (1) → tonemap (0)"
        );
        // The shadow map and HDR scene transients are both live, so they cannot
        // alias; transient memory equals their sum.
        let hdr_bytes = u64::from(SIZE.width) * u64::from(SIZE.height) * 8;
        let shadow_bytes = u64::from(1024u32 * 1024 * 4);
        assert_eq!(plan.transient_memory, hdr_bytes + shadow_bytes);
        assert_eq!(plan.naive_memory, plan.transient_memory);
    }

    #[test]
    fn tonemap_reading_unproduced_hdr_is_rejected() {
        let mut g = RenderGraph::new();
        let hdr = g.transient(ResourceDesc {
            name: "scene-hdr",
            format: TextureFormat::Rgba16Float,
            size: SizeSpec::SurfaceRelative { num: 1, den: 1 },
            kind: ResourceKind::Color,
        });
        g.add_pass(PassDesc {
            name: "tonemap",
            kind: PassKind::Tonemap,
            reads: vec![hdr],
            color: Some(ColorWrite {
                target: g.surface(),
                clear: Some(Color::BLACK),
            }),
            depth: None,
        });
        assert!(matches!(
            g.plan(SIZE),
            Err(RenderError::GraphResourceNotProduced { .. })
        ));
    }

    #[test]
    fn overlapping_lifetimes_do_not_alias() {
        // T1 [0,2], T2 [1,3] overlap → two regions, no aliasing.
        let mut g = RenderGraph::new();
        let t1 = g.transient(color("t1"));
        let t2 = g.transient(color("t2"));
        g.add_pass(opaque(Vec::new(), t1)); // 0 write t1
        g.add_pass(opaque(Vec::new(), t2)); // 1 write t2
        g.add_pass(opaque(vec![t1], t2)); // 2 read t1 (also writes t2, extends t2)
        g.add_pass(opaque(vec![t2], g.surface())); // 3 read t2
        let plan = g.plan(SIZE).unwrap();
        assert_eq!(
            plan.transient_memory, plan.naive_memory,
            "overlapping transients cannot share memory"
        );
    }
}
