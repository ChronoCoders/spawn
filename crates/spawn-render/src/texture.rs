//! 2D textures (sRGB or linear) with their sampler.

use crate::error::{RenderError, RenderResult};
use crate::format::{AddressMode, FilterMode, SurfaceSize, TextureFormat};
use crate::renderer::Renderer;

/// Sampler parameters. `address_mode` applies to both U and V in Phase 1.
#[derive(Debug, Clone, Copy)]
pub struct SamplerConfig {
    pub mag_filter: FilterMode,
    pub min_filter: FilterMode,
    pub address_mode: AddressMode,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            address_mode: AddressMode::ClampToEdge,
        }
    }
}

/// A GPU texture's view and sampler. The `wgpu::TextureView` retains its backing
/// `wgpu::Texture` internally (wgpu holds it via an `Arc`), so dropping this
/// `Texture` releases the GPU texture once no view/bind-group references remain;
/// wgpu defers GPU-side free until idle. A `Material` bind group holds the
/// view/sampler alive for its own lifetime.
pub struct Texture {
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    format: TextureFormat,
    size: SurfaceSize,
}

impl Texture {
    /// Uploads RGBA8 `pixels` as a single-mip 2D texture.
    ///
    /// `srgb` selects `Rgba8UnormSrgb` (color) vs `Rgba8Unorm` (data/normal).
    /// `Err(InvalidArgument)` if `size` is zero or `pixels.len() != w*h*4`.
    pub fn from_rgba8(
        renderer: &Renderer,
        pixels: &[u8],
        size: SurfaceSize,
        srgb: bool,
        sampler: SamplerConfig,
    ) -> RenderResult<Self> {
        Self::build(
            renderer.device(),
            renderer.queue(),
            pixels,
            size,
            srgb,
            sampler,
        )
    }

    /// Device/queue-level constructor used by [`Texture::from_rgba8`] and by the
    /// renderer to build its fallback texture before a `Renderer` value exists.
    pub(crate) fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pixels: &[u8],
        size: SurfaceSize,
        srgb: bool,
        sampler: SamplerConfig,
    ) -> RenderResult<Self> {
        if size.is_zero() {
            return Err(RenderError::InvalidArgument {
                context: "texture size is zero",
            });
        }
        let expected = (size.width as usize)
            .checked_mul(size.height as usize)
            .and_then(|p| p.checked_mul(4))
            .ok_or(RenderError::InvalidArgument {
                context: "texture size overflow",
            })?;
        if pixels.len() != expected {
            return Err(RenderError::InvalidArgument {
                context: "texture pixel byte count != width*height*4",
            });
        }

        let format = if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        };
        let extent = wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("spawn-texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: Some(size.height),
            },
            extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let wgpu_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("spawn-sampler"),
            address_mode_u: sampler.address_mode,
            address_mode_v: sampler.address_mode,
            address_mode_w: sampler.address_mode,
            mag_filter: sampler.mag_filter,
            min_filter: sampler.min_filter,
            mipmap_filter: FilterMode::Nearest,
            ..Default::default()
        });

        Ok(Self {
            view,
            sampler: wgpu_sampler,
            format,
            size,
        })
    }

    /// Resolves texture bytes from spawn-asset, then uploads via
    /// [`Texture::from_rgba8`]. The bytes are interpreted as tightly packed
    /// RGBA8 of dimensions `size`; decoding image containers is out of Phase 1
    /// scope. `Err(InvalidArgument)` if the asset is not yet loaded.
    pub fn from_asset(
        renderer: &Renderer,
        asset: &std::sync::Arc<spawn_asset::BinaryAsset>,
        size: SurfaceSize,
        srgb: bool,
        sampler: SamplerConfig,
    ) -> RenderResult<Self> {
        Self::from_rgba8(renderer, &asset.0, size, srgb, sampler)
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    pub fn format(&self) -> TextureFormat {
        self.format
    }

    pub fn size(&self) -> SurfaceSize {
        self.size
    }
}
