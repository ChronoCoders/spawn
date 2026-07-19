//! Bitmap text: a built-in monospace font rasterized into a glyph atlas, plus
//! left-to-right metrics.
//!
//! The Overlay2D pass (a later module) draws `spawn_ui`'s `DrawCommand::Text` by
//! sampling this atlas; here we own only the GPU resource and the CPU metrics.
//! The baseline font is an embedded 8×8 bitmap covering printable ASCII
//! (`0x20`-`0x7F`), so there is no font-file dependency and no new crate
//! dependency. Vector/TTF rasterization is deferred (it needs a glyph-rasterizer
//! crate); the [`Font`] API is shaped so it can slot in without changing callers.
//!
//! Glyph data is the public-domain `font8x8` basic set (IBM-PC-ROM-derived). Each
//! glyph is 8 rows of 8 bits, least-significant bit leftmost.

use spawn_core::Vec2;

use crate::error::RenderResult;
use crate::format::{AddressMode, FilterMode, SurfaceSize};
use crate::renderer::Renderer;
use crate::texture::{SamplerConfig, Texture};

/// Glyph edge length in atlas texels (native bitmap resolution).
const GLYPH: usize = 8;
/// Atlas grid width in glyphs; 96 printable glyphs lay out 16×6.
const ATLAS_COLS: usize = 16;
const ATLAS_ROWS: usize = 6;
const ATLAS_W: usize = ATLAS_COLS * GLYPH;
const ATLAS_H: usize = ATLAS_ROWS * GLYPH;
/// First and one-past-last code points covered (printable ASCII plus `DEL` as a
/// blank cell, filling the 96-cell grid).
const FIRST: u32 = 0x20;
const COUNT: u32 = (ATLAS_COLS * ATLAS_ROWS) as u32;

/// A glyph's atlas placement: the normalized UV rectangle to sample. Advance is
/// uniform across the monospace font (see [`Font::advance`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    /// Top-left atlas UV (in `[0, 1]`).
    pub uv_min: Vec2,
    /// Bottom-right atlas UV (in `[0, 1]`).
    pub uv_max: Vec2,
}

/// Cell metrics, separated from the GPU atlas so the measure/glyph math is
/// testable without a device. `cell` is the rendered cell size in logical pixels
/// (square cells; advance = line height).
#[derive(Debug, Clone, Copy)]
struct Metrics {
    cell: f32,
}

impl Metrics {
    fn new(px: f32) -> Self {
        Self { cell: px.max(1.0) }
    }

    fn glyph(self, ch: char) -> Option<Glyph> {
        let code = ch as u32;
        if !(FIRST..FIRST + COUNT).contains(&code) {
            return None;
        }
        let index = (code - FIRST) as usize;
        let col = (index % ATLAS_COLS) as f32;
        let row = (index / ATLAS_COLS) as f32;
        let gw = GLYPH as f32 / ATLAS_W as f32;
        let gh = GLYPH as f32 / ATLAS_H as f32;
        Some(Glyph {
            uv_min: Vec2::new(col * gw, row * gh),
            uv_max: Vec2::new((col + 1.0) * gw, (row + 1.0) * gh),
        })
    }

    fn measure(self, text: &str) -> Vec2 {
        let mut max_cols = 0usize;
        let mut lines = 0usize;
        for line in text.split('\n') {
            lines += 1;
            max_cols = max_cols.max(line.chars().count());
        }
        Vec2::new(max_cols as f32 * self.cell, lines as f32 * self.cell)
    }
}

/// A monospace bitmap font: a glyph atlas texture plus its cell metrics. Built
/// once (the atlas is uploaded at construction, never per frame).
pub struct Font {
    atlas: Texture,
    metrics: Metrics,
}

impl Font {
    /// Builds the built-in monospace font, rasterizing the embedded 8×8 glyphs
    /// into an atlas rendered at `px` logical pixels per cell. `px` is clamped to
    /// at least `1.0`.
    pub fn embedded_monospace(renderer: &Renderer, px: f32) -> RenderResult<Self> {
        Self::build(renderer.device(), renderer.queue(), px)
    }

    /// Device/queue-level constructor (so the atlas build is testable on a
    /// headless fallback adapter without a surface, like the pipeline tests).
    pub(crate) fn build(device: &wgpu::Device, queue: &wgpu::Queue, px: f32) -> RenderResult<Self> {
        let pixels = raster_atlas();
        // Nearest filtering keeps the block glyphs crisp when scaled to `px`; the
        // atlas is coverage data (white where a glyph bit is set), so it is linear
        // (`srgb = false`), not color.
        let atlas = Texture::build(
            device,
            queue,
            &pixels,
            SurfaceSize::new(ATLAS_W as u32, ATLAS_H as u32),
            false,
            SamplerConfig {
                mag_filter: FilterMode::Nearest,
                min_filter: FilterMode::Nearest,
                address_mode: AddressMode::ClampToEdge,
            },
        )?;
        Ok(Self {
            atlas,
            metrics: Metrics::new(px),
        })
    }

    /// The glyph atlas texture (and its sampler), bound by the overlay text path.
    pub fn atlas(&self) -> &Texture {
        &self.atlas
    }

    /// The per-character advance in logical pixels (monospace, square cells).
    pub fn advance(&self) -> f32 {
        self.metrics.cell
    }

    /// The line height in logical pixels.
    pub fn line_height(&self) -> f32 {
        self.metrics.cell
    }

    /// The atlas placement of `ch`, or `None` if it is outside the covered range
    /// (printable ASCII). A space is a covered (blank) glyph; an unsupported
    /// character is skipped when drawing but still advances when measuring.
    pub fn glyph(&self, ch: char) -> Option<Glyph> {
        self.metrics.glyph(ch)
    }

    /// The pixel size of `text` laid out left-to-right, splitting on `'\n'`.
    /// Width is the widest line's character count times the advance; height is the
    /// line count times the line height (an empty string is one blank line).
    pub fn measure(&self, text: &str) -> Vec2 {
        self.metrics.measure(text)
    }
}

/// A [`Font`] is a `spawn_ui` text-measure provider, so the editor's layout and
/// the overlay's glyph positioning use the same metrics (no drift between the
/// measured text box and the drawn glyphs).
impl spawn_ui::TextMeasure for Font {
    fn measure(&mut self, text: &str, _max_width: Option<f32>) -> Vec2 {
        self.metrics.measure(text)
    }
}

/// Rasterizes the embedded glyph table into a tightly packed RGBA8 atlas: white
/// (opaque) where a glyph bit is set, transparent elsewhere, so the overlay
/// shader can multiply it by the UI color as a coverage mask.
fn raster_atlas() -> Vec<u8> {
    let mut pixels = vec![0u8; ATLAS_W * ATLAS_H * 4];
    for (index, rows) in GLYPHS.iter().enumerate() {
        let base_x = (index % ATLAS_COLS) * GLYPH;
        let base_y = (index / ATLAS_COLS) * GLYPH;
        for (ry, &byte) in rows.iter().enumerate() {
            for col in 0..GLYPH {
                if (byte >> col) & 1 == 1 {
                    let x = base_x + col;
                    let y = base_y + ry;
                    let p = (y * ATLAS_W + x) * 4;
                    pixels[p] = 255;
                    pixels[p + 1] = 255;
                    pixels[p + 2] = 255;
                    pixels[p + 3] = 255;
                }
            }
        }
    }
    pixels
}

/// Public-domain `font8x8` basic set for `0x20`-`0x7F` (96 glyphs). 8 rows × 8
/// bits each; bit 0 (LSB) is the leftmost column, row 0 the top. `0x7F` is blank.
#[rustfmt::skip]
const GLYPHS: [[u8; 8]; 96] = [
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00], // 0x20 ' '
    [0x18,0x3C,0x3C,0x18,0x18,0x00,0x18,0x00], // '!'
    [0x36,0x36,0x00,0x00,0x00,0x00,0x00,0x00], // '"'
    [0x36,0x36,0x7F,0x36,0x7F,0x36,0x36,0x00], // '#'
    [0x0C,0x3E,0x03,0x1E,0x30,0x1F,0x0C,0x00], // '$'
    [0x00,0x63,0x33,0x18,0x0C,0x66,0x63,0x00], // '%'
    [0x1C,0x36,0x1C,0x6E,0x3B,0x33,0x6E,0x00], // '&'
    [0x06,0x06,0x03,0x00,0x00,0x00,0x00,0x00], // '\''
    [0x18,0x0C,0x06,0x06,0x06,0x0C,0x18,0x00], // '('
    [0x06,0x0C,0x18,0x18,0x18,0x0C,0x06,0x00], // ')'
    [0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00], // '*'
    [0x00,0x0C,0x0C,0x3F,0x0C,0x0C,0x00,0x00], // '+'
    [0x00,0x00,0x00,0x00,0x00,0x0C,0x0C,0x06], // ','
    [0x00,0x00,0x00,0x3F,0x00,0x00,0x00,0x00], // '-'
    [0x00,0x00,0x00,0x00,0x00,0x0C,0x0C,0x00], // '.'
    [0x60,0x30,0x18,0x0C,0x06,0x03,0x01,0x00], // '/'
    [0x3E,0x63,0x73,0x7B,0x6F,0x67,0x3E,0x00], // '0'
    [0x0C,0x0E,0x0C,0x0C,0x0C,0x0C,0x3F,0x00], // '1'
    [0x1E,0x33,0x30,0x1C,0x06,0x33,0x3F,0x00], // '2'
    [0x1E,0x33,0x30,0x1C,0x30,0x33,0x1E,0x00], // '3'
    [0x38,0x3C,0x36,0x33,0x7F,0x30,0x78,0x00], // '4'
    [0x3F,0x03,0x1F,0x30,0x30,0x33,0x1E,0x00], // '5'
    [0x1C,0x06,0x03,0x1F,0x33,0x33,0x1E,0x00], // '6'
    [0x3F,0x33,0x30,0x18,0x0C,0x0C,0x0C,0x00], // '7'
    [0x1E,0x33,0x33,0x1E,0x33,0x33,0x1E,0x00], // '8'
    [0x1E,0x33,0x33,0x3E,0x30,0x18,0x0E,0x00], // '9'
    [0x00,0x0C,0x0C,0x00,0x00,0x0C,0x0C,0x00], // ':'
    [0x00,0x0C,0x0C,0x00,0x00,0x0C,0x0C,0x06], // ';'
    [0x18,0x0C,0x06,0x03,0x06,0x0C,0x18,0x00], // '<'
    [0x00,0x00,0x3F,0x00,0x00,0x3F,0x00,0x00], // '='
    [0x06,0x0C,0x18,0x30,0x18,0x0C,0x06,0x00], // '>'
    [0x1E,0x33,0x30,0x18,0x0C,0x00,0x0C,0x00], // '?'
    [0x3E,0x63,0x7B,0x7B,0x7B,0x03,0x1E,0x00], // '@'
    [0x0C,0x1E,0x33,0x33,0x3F,0x33,0x33,0x00], // 'A'
    [0x3F,0x66,0x66,0x3E,0x66,0x66,0x3F,0x00], // 'B'
    [0x3C,0x66,0x03,0x03,0x03,0x66,0x3C,0x00], // 'C'
    [0x1F,0x36,0x66,0x66,0x66,0x36,0x1F,0x00], // 'D'
    [0x7F,0x46,0x16,0x1E,0x16,0x46,0x7F,0x00], // 'E'
    [0x7F,0x46,0x16,0x1E,0x16,0x06,0x0F,0x00], // 'F'
    [0x3C,0x66,0x03,0x03,0x73,0x66,0x7C,0x00], // 'G'
    [0x33,0x33,0x33,0x3F,0x33,0x33,0x33,0x00], // 'H'
    [0x1E,0x0C,0x0C,0x0C,0x0C,0x0C,0x1E,0x00], // 'I'
    [0x78,0x30,0x30,0x30,0x33,0x33,0x1E,0x00], // 'J'
    [0x67,0x66,0x36,0x1E,0x36,0x66,0x67,0x00], // 'K'
    [0x0F,0x06,0x06,0x06,0x46,0x66,0x7F,0x00], // 'L'
    [0x63,0x77,0x7F,0x7F,0x6B,0x63,0x63,0x00], // 'M'
    [0x63,0x67,0x6F,0x7B,0x73,0x63,0x63,0x00], // 'N'
    [0x1C,0x36,0x63,0x63,0x63,0x36,0x1C,0x00], // 'O'
    [0x3F,0x66,0x66,0x3E,0x06,0x06,0x0F,0x00], // 'P'
    [0x1E,0x33,0x33,0x33,0x3B,0x1E,0x38,0x00], // 'Q'
    [0x3F,0x66,0x66,0x3E,0x36,0x66,0x67,0x00], // 'R'
    [0x1E,0x33,0x07,0x0E,0x38,0x33,0x1E,0x00], // 'S'
    [0x3F,0x2D,0x0C,0x0C,0x0C,0x0C,0x1E,0x00], // 'T'
    [0x33,0x33,0x33,0x33,0x33,0x33,0x3F,0x00], // 'U'
    [0x33,0x33,0x33,0x33,0x33,0x1E,0x0C,0x00], // 'V'
    [0x63,0x63,0x63,0x6B,0x7F,0x77,0x63,0x00], // 'W'
    [0x63,0x63,0x36,0x1C,0x1C,0x36,0x63,0x00], // 'X'
    [0x33,0x33,0x33,0x1E,0x0C,0x0C,0x1E,0x00], // 'Y'
    [0x7F,0x63,0x31,0x18,0x4C,0x66,0x7F,0x00], // 'Z'
    [0x1E,0x06,0x06,0x06,0x06,0x06,0x1E,0x00], // '['
    [0x03,0x06,0x0C,0x18,0x30,0x60,0x40,0x00], // '\\'
    [0x1E,0x18,0x18,0x18,0x18,0x18,0x1E,0x00], // ']'
    [0x08,0x1C,0x36,0x63,0x00,0x00,0x00,0x00], // '^'
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFF], // '_'
    [0x0C,0x0C,0x18,0x00,0x00,0x00,0x00,0x00], // '`'
    [0x00,0x00,0x1E,0x30,0x3E,0x33,0x6E,0x00], // 'a'
    [0x07,0x06,0x06,0x3E,0x66,0x66,0x3B,0x00], // 'b'
    [0x00,0x00,0x1E,0x33,0x03,0x33,0x1E,0x00], // 'c'
    [0x38,0x30,0x30,0x3E,0x33,0x33,0x6E,0x00], // 'd'
    [0x00,0x00,0x1E,0x33,0x3F,0x03,0x1E,0x00], // 'e'
    [0x1C,0x36,0x06,0x0F,0x06,0x06,0x0F,0x00], // 'f'
    [0x00,0x00,0x6E,0x33,0x33,0x3E,0x30,0x1F], // 'g'
    [0x07,0x06,0x36,0x6E,0x66,0x66,0x67,0x00], // 'h'
    [0x0C,0x00,0x0E,0x0C,0x0C,0x0C,0x1E,0x00], // 'i'
    [0x30,0x00,0x30,0x30,0x30,0x33,0x33,0x1E], // 'j'
    [0x07,0x06,0x66,0x36,0x1E,0x36,0x67,0x00], // 'k'
    [0x0E,0x0C,0x0C,0x0C,0x0C,0x0C,0x1E,0x00], // 'l'
    [0x00,0x00,0x33,0x7F,0x7F,0x6B,0x63,0x00], // 'm'
    [0x00,0x00,0x1F,0x33,0x33,0x33,0x33,0x00], // 'n'
    [0x00,0x00,0x1E,0x33,0x33,0x33,0x1E,0x00], // 'o'
    [0x00,0x00,0x3B,0x66,0x66,0x3E,0x06,0x0F], // 'p'
    [0x00,0x00,0x6E,0x33,0x33,0x3E,0x30,0x78], // 'q'
    [0x00,0x00,0x3B,0x6E,0x66,0x06,0x0F,0x00], // 'r'
    [0x00,0x00,0x3E,0x03,0x1E,0x30,0x1F,0x00], // 's'
    [0x08,0x0C,0x3E,0x0C,0x0C,0x2C,0x18,0x00], // 't'
    [0x00,0x00,0x33,0x33,0x33,0x33,0x6E,0x00], // 'u'
    [0x00,0x00,0x33,0x33,0x33,0x1E,0x0C,0x00], // 'v'
    [0x00,0x00,0x63,0x6B,0x7F,0x7F,0x36,0x00], // 'w'
    [0x00,0x00,0x63,0x36,0x1C,0x36,0x63,0x00], // 'x'
    [0x00,0x00,0x33,0x33,0x33,0x3E,0x30,0x1F], // 'y'
    [0x00,0x00,0x3F,0x19,0x0C,0x26,0x3F,0x00], // 'z'
    [0x38,0x0C,0x0C,0x07,0x0C,0x0C,0x38,0x00], // '{'
    [0x18,0x18,0x18,0x00,0x18,0x18,0x18,0x00], // '|'
    [0x07,0x0C,0x0C,0x38,0x0C,0x0C,0x07,0x00], // '}'
    [0x6E,0x3B,0x00,0x00,0x00,0x00,0x00,0x00], // '~'
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00], // 0x7F (blank)
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_table_is_full_grid() {
        assert_eq!(GLYPHS.len(), (ATLAS_COLS * ATLAS_ROWS));
        assert_eq!(COUNT as usize, GLYPHS.len());
    }

    #[test]
    fn raster_atlas_has_expected_size_and_marks_set_bits() {
        let pixels = raster_atlas();
        assert_eq!(pixels.len(), ATLAS_W * ATLAS_H * 4);
        // '!' (index 1) has a set bit in its top rows, so its cell is not blank;
        // space (index 0) is entirely blank.
        let cell_opaque = |index: usize| {
            let bx = (index % ATLAS_COLS) * GLYPH;
            let by = (index / ATLAS_COLS) * GLYPH;
            (0..GLYPH).any(|ry| {
                (0..GLYPH).any(|cx| {
                    let p = ((by + ry) * ATLAS_W + (bx + cx)) * 4 + 3;
                    pixels[p] != 0
                })
            })
        };
        assert!(!cell_opaque(0), "space cell is blank");
        assert!(cell_opaque(1), "'!' cell has set pixels");
    }

    #[test]
    fn measure_is_monospace_and_multiline() {
        let m = Metrics::new(10.0);
        assert_eq!(m.measure("ABCD"), Vec2::new(40.0, 10.0));
        assert_eq!(m.measure(""), Vec2::new(0.0, 10.0));
        assert_eq!(m.measure("A\nBBB"), Vec2::new(30.0, 20.0));
        assert_eq!(m.cell, 10.0);
    }

    #[test]
    fn glyph_uv_in_range_and_unsupported_is_none() {
        let m = Metrics::new(16.0);
        let a = m.glyph('A').expect("ASCII 'A' is covered");
        assert!(a.uv_min.x >= 0.0 && a.uv_max.x <= 1.0);
        assert!(a.uv_min.y >= 0.0 && a.uv_max.y <= 1.0);
        assert!(a.uv_max.x > a.uv_min.x && a.uv_max.y > a.uv_min.y);
        assert!(m.glyph(' ').is_some(), "space is a covered blank glyph");
        assert!(m.glyph('\u{2603}').is_none(), "non-ASCII is uncovered");
    }

    #[test]
    fn px_is_clamped_to_one() {
        assert_eq!(Metrics::new(0.0).cell, 1.0);
    }

    fn try_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: true,
            compatible_surface: None,
        }))?;
        pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("spawn-text-test-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .ok()
    }

    #[test]
    fn atlas_builds_at_expected_size() {
        let Some((device, queue)) = try_device() else {
            eprintln!("SKIP atlas_builds_at_expected_size: no GPU adapter");
            return;
        };
        let font = Font::build(&device, &queue, 16.0).expect("atlas builds");
        assert_eq!(
            font.atlas().size(),
            SurfaceSize::new(ATLAS_W as u32, ATLAS_H as u32)
        );
        assert_eq!(
            font.atlas().format(),
            crate::format::TextureFormat::Rgba8Unorm
        );
        assert_eq!(font.advance(), 16.0);
        assert!(font.glyph('Z').is_some());
    }
}
