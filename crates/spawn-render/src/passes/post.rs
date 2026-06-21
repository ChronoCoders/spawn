//! Fullscreen post-processing: configuration ([`PostChain`]/[`BloomConfig`]/
//! [`TonemapConfig`]), a graph builder that appends the bloom → tonemap → FXAA
//! tail onto a `RenderGraph`, and the generic fullscreen-pass record.
//!
//! Each post pass draws one screen-covering triangle (generated from the vertex
//! index, no vertex buffer) sampling its input transient with a small post-uniform
//! built into its bind group at compile.

use spawn_core::Color;

use crate::error::{RenderError, RenderResult};
use crate::format::{SurfaceSize, TextureFormat};
use crate::graph::{
    ColorWrite, PassDesc, PassKind, RenderGraph, ResourceDesc, ResourceId, ResourceKind, SizeSpec,
};
use crate::pipeline::PipelineKey;
use crate::renderer::Renderer;

/// Tonemapping operator applied (after exposure) by the tonemap pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TonemapOperator {
    /// `x / (1 + x)`.
    Reinhard,
    /// ACES-fitted filmic curve (default).
    #[default]
    Aces,
}

/// Tonemap configuration: a linear `exposure` multiplier and the operator.
#[derive(Debug, Clone, Copy)]
pub struct TonemapConfig {
    pub exposure: f32,
    pub operator: TonemapOperator,
}

impl Default for TonemapConfig {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            operator: TonemapOperator::Aces,
        }
    }
}

impl TonemapConfig {
    fn validate(&self) -> RenderResult<()> {
        if !self.exposure.is_finite() || self.exposure <= 0.0 {
            return Err(RenderError::PostConfigInvalid {
                context: "tonemap exposure must be a positive finite number",
            });
        }
        Ok(())
    }

    fn uniform(&self) -> [f32; 4] {
        let op = match self.operator {
            TonemapOperator::Reinhard => 0.0,
            TonemapOperator::Aces => 1.0,
        };
        [self.exposure, op, 0.0, 0.0]
    }
}

/// Bloom configuration: bright-pass `threshold`/`knee`, composite `intensity`, and
/// the number of separable blur `iterations` (each is a horizontal + vertical pair).
#[derive(Debug, Clone, Copy)]
pub struct BloomConfig {
    pub threshold: f32,
    pub knee: f32,
    pub intensity: f32,
    pub iterations: u32,
}

impl Default for BloomConfig {
    fn default() -> Self {
        Self {
            threshold: 1.0,
            knee: 0.5,
            intensity: 0.5,
            iterations: 3,
        }
    }
}

impl BloomConfig {
    fn validate(&self) -> RenderResult<()> {
        if self.iterations == 0 {
            return Err(RenderError::PostConfigInvalid {
                context: "bloom enabled with zero iterations",
            });
        }
        if !self.threshold.is_finite() || !self.knee.is_finite() || !self.intensity.is_finite() {
            return Err(RenderError::PostConfigInvalid {
                context: "bloom threshold/knee/intensity must be finite",
            });
        }
        Ok(())
    }
}

/// The post-processing chain: optional bloom, a required tonemap (HDR→LDR), and
/// optional FXAA. Disabled stages are omitted from the graph entirely (their
/// transients are never allocated).
#[derive(Debug, Clone, Copy, Default)]
pub struct PostChain {
    pub bloom: Option<BloomConfig>,
    pub tonemap: TonemapConfig,
    pub fxaa: bool,
}

impl PostChain {
    /// Validates the chain config. `Err(PostConfigInvalid)` on a bad exposure or a
    /// zero-iteration / non-finite bloom.
    pub fn validate(&self) -> RenderResult<()> {
        self.tonemap.validate()?;
        if let Some(bloom) = &self.bloom {
            bloom.validate()?;
        }
        Ok(())
    }

    /// Appends the post tail onto `graph`: optional bloom (bright → blur ping-pong
    /// → additive composite into `scene_hdr`), then tonemap (`scene_hdr` → LDR),
    /// then optional FXAA (→ surface). Returns the declared transients so the
    /// caller can confirm derivation. `surface_size` sizes the half-res bloom and
    /// LDR transients. `Err(PostConfigInvalid)` for an invalid config.
    pub fn build(
        &self,
        graph: &mut RenderGraph,
        scene_hdr: ResourceId,
        hdr_format: TextureFormat,
        surface_format: TextureFormat,
        surface_size: SurfaceSize,
    ) -> RenderResult<()> {
        self.validate()?;
        let surface = graph.surface();
        let inv_w = 1.0 / surface_size.width.max(1) as f32;
        let inv_h = 1.0 / surface_size.height.max(1) as f32;
        // What the tonemap samples: the bloom-composited scene when bloom is on,
        // otherwise the scene HDR directly.
        let mut tonemap_input = scene_hdr;

        if let Some(bloom) = &self.bloom {
            let half = SizeSpec::SurfaceRelative { num: 1, den: 2 };
            let half_desc = |name: &'static str| ResourceDesc {
                name,
                format: hdr_format,
                size: half,
                kind: ResourceKind::Color,
            };
            // Bright-pass scene HDR → the first bloom transient.
            let bright = graph.transient(half_desc("bloom-bright"));
            graph.add_post_pass(
                fullscreen_pass("bloom-bright", PassKind::BloomBright, scene_hdr, bright),
                [bloom.threshold, bloom.knee, 0.0, 0.0],
            );
            // Separable Gaussian blur: each iteration is a horizontal then a
            // vertical pass, chaining through a fresh transient per step (the graph
            // derivation requires a single writer per resource; the aliasing pass
            // reclaims the disjoint-lifetime memory). Texel size targets the
            // half-res transient (2× the full-res texel).
            let (bx, by) = (2.0 * inv_w, 2.0 * inv_h);
            let mut src = bright;
            for _ in 0..bloom.iterations {
                let h = graph.transient(half_desc("bloom-blur-h"));
                graph.add_post_pass(
                    fullscreen_pass("bloom-blur-h", PassKind::BloomBlur, src, h),
                    [1.0, 0.0, bx, by],
                );
                let v = graph.transient(half_desc("bloom-blur-v"));
                graph.add_post_pass(
                    fullscreen_pass("bloom-blur-v", PassKind::BloomBlur, h, v),
                    [0.0, 1.0, bx, by],
                );
                src = v;
            }
            // Composite scene HDR + blurred bloom (now in `src`) into a fresh HDR
            // transient. Two inputs (rather than blending back into the scene)
            // avoids a read/write cycle: the bright-pass reads the scene that the
            // composite would otherwise also write.
            let composited = graph.transient(ResourceDesc {
                name: "scene-composited",
                format: hdr_format,
                size: SizeSpec::SurfaceRelative { num: 1, den: 1 },
                kind: ResourceKind::Color,
            });
            graph.add_post_pass(
                PassDesc {
                    name: "bloom-composite",
                    kind: PassKind::BloomComposite,
                    reads: vec![scene_hdr, src],
                    color: Some(ColorWrite {
                        target: composited,
                        clear: None,
                    }),
                    depth: None,
                },
                [bloom.intensity, 0.0, 0.0, 0.0],
            );
            tonemap_input = composited;
        }

        // Tonemap the (possibly bloom-composited) scene HDR → LDR (an intermediate
        // transient when FXAA follows, else straight to the surface).
        let tonemap_target = if self.fxaa {
            graph.transient(ResourceDesc {
                name: "ldr",
                format: surface_format,
                size: SizeSpec::SurfaceRelative { num: 1, den: 1 },
                kind: ResourceKind::Color,
            })
        } else {
            surface
        };
        graph.add_post_pass(
            fullscreen_pass("tonemap", PassKind::Tonemap, tonemap_input, tonemap_target),
            self.tonemap.uniform(),
        );

        if self.fxaa {
            graph.add_post_pass(
                fullscreen_pass("fxaa", PassKind::Fxaa, tonemap_target, surface),
                [inv_w, inv_h, 0.0, 0.0],
            );
        }
        Ok(())
    }
}

/// A fullscreen pass reading `input` and writing `target` (load — the fullscreen
/// triangle overwrites every pixel), no depth.
fn fullscreen_pass(
    name: &'static str,
    kind: PassKind,
    input: ResourceId,
    target: ResourceId,
) -> PassDesc {
    PassDesc {
        name,
        kind,
        reads: vec![input],
        color: Some(ColorWrite {
            target,
            clear: None,
        }),
        depth: None,
    }
}

/// Records a fullscreen post pass into `encoder` against `color_view`. `pipeline`
/// is the built-in pipeline for the pass kind; `input_bind_group` is the
/// graph-built group-0 binding (input texture + sampler + post-uniform). Clears
/// per `clear_color` (`None` ⇒ load). No depth, no vertex buffer, no allocation.
pub(crate) fn record_fullscreen(
    renderer: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    color_view: &wgpu::TextureView,
    clear_color: Option<Color>,
    pipeline_key: PipelineKey,
    input_bind_group: &wgpu::BindGroup,
) -> RenderResult<()> {
    let color_load = match clear_color {
        Some(c) => wgpu::LoadOp::Clear(wgpu::Color {
            r: c.r as f64,
            g: c.g as f64,
            b: c.b as f64,
            a: c.a as f64,
        }),
        None => wgpu::LoadOp::Load,
    };
    let pipeline = renderer.cache.get(&pipeline_key)?;

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("spawn-post-fullscreen"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color_view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: color_load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, input_bind_group, &[]);
    pass.draw(0..3, 0..1);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_chain_validates() {
        assert!(PostChain::default().validate().is_ok());
        assert!(PostChain {
            bloom: Some(BloomConfig::default()),
            tonemap: TonemapConfig::default(),
            fxaa: true,
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn zero_bloom_iterations_is_rejected() {
        let chain = PostChain {
            bloom: Some(BloomConfig {
                iterations: 0,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(matches!(
            chain.validate(),
            Err(RenderError::PostConfigInvalid { .. })
        ));
    }

    #[test]
    fn nonpositive_or_nonfinite_exposure_is_rejected() {
        for exposure in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let chain = PostChain {
                tonemap: TonemapConfig {
                    exposure,
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(matches!(
                chain.validate(),
                Err(RenderError::PostConfigInvalid { .. })
            ));
        }
    }

    #[test]
    fn tonemap_uniform_packs_exposure_and_operator() {
        let reinhard = TonemapConfig {
            exposure: 2.0,
            operator: TonemapOperator::Reinhard,
        };
        assert_eq!(reinhard.uniform(), [2.0, 0.0, 0.0, 0.0]);
        let aces = TonemapConfig {
            exposure: 1.5,
            operator: TonemapOperator::Aces,
        };
        assert_eq!(aces.uniform(), [1.5, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn operator_defaults_to_aces() {
        assert_eq!(TonemapOperator::default(), TonemapOperator::Aces);
    }
}
