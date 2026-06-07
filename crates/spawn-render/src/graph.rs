//! Explicit, ordered render-graph-lite.
//!
//! Phase 1 is an ordered list of passes with author-declared inputs/outputs.
//! wgpu performs the actual GPU synchronization; this graph makes ordering and
//! resource I/O visible and validates them. It inserts no barriers (Phase 2).

use std::cell::Cell;

use spawn_core::Color;

use crate::error::{RenderError, RenderResult};

/// A logical resource reference declared as a pass input or output. `Surface` is
/// the swapchain color target; `Texture` is an offscreen target identified by a
/// caller-chosen stable name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceRef {
    Surface,
    Texture(&'static str),
}

/// Where a pass writes its color.
///
/// Phase 1 execution can only render to the swapchain surface: [`SurfaceColor`]
/// is the sole target the frame loop honors. [`Texture`] (offscreen rendering)
/// is modeled here for forward compatibility but is rejected by
/// [`RenderGraph::validate`] — offscreen targets are Phase 2. Constructing a
/// graph with a [`Texture`] target therefore fails validation rather than
/// silently rendering to the surface.
///
/// [`SurfaceColor`]: ColorTarget::SurfaceColor
/// [`Texture`]: ColorTarget::Texture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTarget {
    SurfaceColor,
    Texture(&'static str),
}

/// Depth attachment selector for a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthTarget {
    Default,
}

/// The pass kind. Phase 1 has exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassKind {
    ForwardOpaque,
}

/// One pass in execution order with its attachments, clear behavior, and
/// declared resource I/O.
pub struct RenderPassDesc {
    pub name: &'static str,
    pub kind: PassKind,
    pub color_target: ColorTarget,
    pub depth_target: Option<DepthTarget>,
    pub clear_color: Option<Color>,
    pub clear_depth: Option<f32>,
    pub inputs: Vec<ResourceRef>,
    pub outputs: Vec<ResourceRef>,
}

/// Ordered list of passes. Built once at setup and reused across frames; it is
/// not reallocated per frame and [`RenderGraph::validate`] is called on change,
/// never in the frame loop.
pub struct RenderGraph {
    passes: Vec<RenderPassDesc>,
    /// Set by [`RenderGraph::validate`] on success, cleared by any mutation
    /// ([`RenderGraph::add_pass`]). `Cell` because validation is logically a
    /// read (`validate(&self)`) but must record that this exact pass set passed,
    /// so [`crate::frame::FrameContext::execute`] can refuse an unvalidated or
    /// since-mutated graph before recording anything (audit finding #5: an
    /// unvalidated two-surface-pass graph would otherwise clobber the singleton
    /// camera/model uniforms).
    validated: Cell<bool>,
}

impl Default for RenderGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderGraph {
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            validated: Cell::new(false),
        }
    }

    /// Appends `desc` in execution order.
    ///
    /// Mutating the graph invalidates any prior validation: the validated flag is
    /// cleared so a graph changed after [`RenderGraph::validate`] must be
    /// re-validated before [`crate::frame::FrameContext::execute`] will record it.
    pub fn add_pass(&mut self, desc: RenderPassDesc) -> &mut Self {
        self.passes.push(desc);
        self.validated.set(false);
        self
    }

    pub fn passes(&self) -> &[RenderPassDesc] {
        &self.passes
    }

    /// Whether the current pass set has passed [`RenderGraph::validate`] with no
    /// intervening mutation. `execute` gates on this; a graph that was never
    /// validated, or was mutated after validation, returns `false`.
    pub(crate) fn is_validated(&self) -> bool {
        self.validated.get()
    }

    /// Validates ordering and resource I/O. `Err(InvalidArgument)` if the graph
    /// is empty, if a pass input is produced by no earlier pass (and is not an
    /// external resource), or if any Phase 1 restriction below is violated.
    ///
    /// Phase 1 execution restrictions (enforced here so the graph never
    /// expresses something the frame loop cannot honor):
    ///
    /// - Every pass must target [`ColorTarget::SurfaceColor`]. Offscreen
    ///   [`ColorTarget::Texture`] targets are Phase 2; the frame loop only ever
    ///   renders to the swapchain surface, so a texture target is rejected
    ///   rather than silently redirected to the surface.
    /// - Exactly one surface-color pass per frame. The renderer's camera and
    ///   per-draw model uniforms are singleton buffers submitted once per frame
    ///   (see [`crate::renderer::Renderer::write_camera`]); a second pass would
    ///   clobber the first pass's uniforms. Multi-pass-to-surface is therefore
    ///   structurally prevented until Phase 2 gives each pass its own uniforms.
    ///
    /// Called at build/change time only; [`crate::frame::FrameContext::execute`]
    /// assumes a validated graph.
    pub fn validate(&self) -> RenderResult<()> {
        if self.passes.is_empty() {
            return Err(RenderError::InvalidArgument {
                context: "render graph has no passes",
            });
        }

        let mut produced: Vec<ResourceRef> = Vec::new();
        let mut surface_targets = 0usize;

        for pass in &self.passes {
            for input in &pass.inputs {
                let external = matches!(input, ResourceRef::Surface);
                if !external && !produced.contains(input) {
                    return Err(RenderError::InvalidArgument {
                        context: "render pass input not produced by an earlier pass",
                    });
                }
            }
            match pass.color_target {
                ColorTarget::SurfaceColor => surface_targets += 1,
                ColorTarget::Texture(_) => {
                    return Err(RenderError::InvalidArgument {
                        context: "offscreen color targets are Phase 2; \
                                  Phase 1 passes must target the surface",
                    });
                }
            }
            for output in &pass.outputs {
                if !produced.contains(output) {
                    produced.push(*output);
                }
            }
            if !produced.contains(&ResourceRef::Surface) {
                produced.push(ResourceRef::Surface);
            }
        }

        if surface_targets != 1 {
            return Err(RenderError::InvalidArgument {
                context: "Phase 1 requires exactly one surface-color pass per frame",
            });
        }

        self.validated.set(true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque_pass(target: ColorTarget, inputs: Vec<ResourceRef>) -> RenderPassDesc {
        RenderPassDesc {
            name: "opaque",
            kind: PassKind::ForwardOpaque,
            color_target: target,
            depth_target: Some(DepthTarget::Default),
            clear_color: Some(Color::BLACK),
            clear_depth: Some(1.0),
            inputs,
            outputs: Vec::new(),
        }
    }

    #[test]
    fn empty_graph_is_invalid() {
        assert!(RenderGraph::new().validate().is_err());
    }

    #[test]
    fn single_opaque_pass_is_valid() {
        let mut g = RenderGraph::new();
        g.add_pass(opaque_pass(ColorTarget::SurfaceColor, Vec::new()));
        assert!(g.validate().is_ok());
    }

    #[test]
    fn unproduced_input_is_invalid() {
        let mut g = RenderGraph::new();
        g.add_pass(opaque_pass(
            ColorTarget::SurfaceColor,
            vec![ResourceRef::Texture("gbuffer")],
        ));
        assert!(g.validate().is_err());
    }

    #[test]
    fn offscreen_texture_target_is_rejected_phase1() {
        let mut g = RenderGraph::new();
        g.add_pass(opaque_pass(ColorTarget::Texture("gbuffer"), Vec::new()));
        assert!(g.validate().is_err());
    }

    #[test]
    fn fresh_graph_is_not_validated() {
        assert!(!RenderGraph::new().is_validated());
    }

    #[test]
    fn successful_validate_marks_validated() {
        let mut g = RenderGraph::new();
        g.add_pass(opaque_pass(ColorTarget::SurfaceColor, Vec::new()));
        assert!(g.validate().is_ok());
        assert!(g.is_validated());
    }

    #[test]
    fn failed_validate_leaves_graph_unvalidated() {
        let g = RenderGraph::new();
        assert!(g.validate().is_err());
        assert!(!g.is_validated());
    }

    #[test]
    fn add_pass_clears_validated_flag() {
        let mut g = RenderGraph::new();
        g.add_pass(opaque_pass(ColorTarget::SurfaceColor, Vec::new()));
        assert!(g.validate().is_ok());
        assert!(g.is_validated());
        g.add_pass(opaque_pass(ColorTarget::SurfaceColor, Vec::new()));
        assert!(!g.is_validated());
    }

    #[test]
    fn two_surface_passes_are_rejected_phase1() {
        // Finding B: a two-pass-to-surface graph would let the second pass's
        // singleton camera/model uniforms clobber the first pass. validate()
        // rejects it so the hazard cannot reach execution.
        let mut g = RenderGraph::new();
        g.add_pass(opaque_pass(ColorTarget::SurfaceColor, Vec::new()));
        g.add_pass(opaque_pass(ColorTarget::SurfaceColor, Vec::new()));
        assert!(g.validate().is_err());
    }
}
