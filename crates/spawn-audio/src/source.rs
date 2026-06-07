//! The [`AudioSource`] asset and its [`AudioLoader`] for spawn-asset.

use spawn_asset::{AssetError, AssetLoader, AssetResult, AssetServer, LoadContext};

use crate::backend::{decode_source, BackendSound};
use crate::error::{AudioError, AudioResult};

/// One decoded clip, held fully in memory (Phase 1 streams from memory). Cheap
/// to share across `play` calls; the inner sample buffer is ref-counted. Lives
/// in the asset store as a `Handle<AudioSource>`.
pub struct AudioSource {
    inner: BackendSound,
}

impl AudioSource {
    pub(crate) fn backend_sound(&self) -> &BackendSound {
        &self.inner
    }

    /// Clip length in seconds.
    pub fn duration(&self) -> f32 {
        self.inner.duration_secs()
    }

    /// The clip's source channel count (e.g. `1` for mono, `2` for stereo),
    /// sniffed from the file header at load. `0` for an empty clip. If the header
    /// cannot be sniffed, falls back to the decoded-frame count (`2`, since kira
    /// always decodes to stereo frames).
    pub fn channels(&self) -> u16 {
        self.inner.channels()
    }

    /// The clip's sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
}

/// Sniffs the source channel count from raw `wav`/`ogg` bytes without decoding.
///
/// WAV: walks the RIFF chunk list to the `fmt ` chunk and reads the 16-bit LE
/// channel field at its offset 2. OGG Vorbis: locates the Vorbis identification
/// header (`\x01vorbis`) inside the first Ogg page and reads its 8-bit channel
/// field. Returns the decoded-frame fallback `2` when the header cannot be
/// parsed (kira always decodes to stereo frames, so `2` is the safe default).
fn sniff_channels(bytes: &[u8]) -> u16 {
    const FALLBACK: u16 = 2;
    sniff_wav_channels(bytes)
        .or_else(|| sniff_ogg_channels(bytes))
        .unwrap_or(FALLBACK)
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn sniff_wav_channels(bytes: &[u8]) -> Option<u16> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    // Walk RIFF chunks starting after "RIFF<size>WAVE".
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let id = bytes.get(offset..offset + 4)?;
        let size = read_u32_le(bytes, offset + 4)? as usize;
        let body = offset + 8;
        if id == b"fmt " {
            // Channel count is the u16 LE at fmt-body offset 2.
            return read_u16_le(bytes, body + 2);
        }
        // Chunks are word-aligned: bodies are padded to even length.
        offset = body + size + (size & 1);
    }
    None
}

fn sniff_ogg_channels(bytes: &[u8]) -> Option<u16> {
    if bytes.get(0..4)? != b"OggS" {
        return None;
    }
    // The Vorbis identification header begins with "\x01vorbis"; its channel
    // field is the single byte after the 1-byte packet type, 6-byte signature,
    // and 4-byte vorbis_version.
    const IDENT: &[u8] = b"\x01vorbis";
    let pos = bytes.windows(IDENT.len()).position(|w| w == IDENT)?;
    let channels_offset = pos + IDENT.len() + 4;
    Some(u16::from(*bytes.get(channels_offset)?))
}

impl spawn_asset::Asset for AudioSource {}

pub struct AudioLoader;

fn map_decode_error(err: AudioError) -> AssetError {
    AssetError::Parse {
        path: String::new(),
        detail: err.to_string(),
    }
}

impl AssetLoader for AudioLoader {
    type Output = AudioSource;

    fn extensions(&self) -> &'static [&'static str] {
        &["wav", "ogg"]
    }

    fn load(&self, bytes: &[u8], ctx: &LoadContext) -> AssetResult<Self::Output> {
        match ctx.extension {
            "wav" | "ogg" => {}
            other => {
                return Err(AssetError::Parse {
                    path: ctx.canonical_path.to_owned(),
                    detail: AudioError::UnsupportedFormat {
                        context: "audio loader only handles wav and ogg",
                    }
                    .to_string()
                        + " ("
                        + other
                        + ")",
                });
            }
        }
        let channels = sniff_channels(bytes);
        let sound = decode_source(bytes, channels).map_err(map_decode_error)?;
        Ok(AudioSource { inner: sound })
    }
}

/// Registers [`AudioLoader`] for the `wav` and `ogg` extensions on `server`.
/// Call once at startup. Returns [`AudioError::Backend`] if either extension is
/// already claimed by another loader.
pub fn register(server: &mut AssetServer) -> AudioResult<()> {
    server
        .register_loader(AudioLoader)
        .map_err(|_| AudioError::Backend {
            context: "audio loader extension already registered",
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_send_sync<T: Send + Sync>() {}

    #[test]
    fn audio_source_is_send_sync() {
        is_send_sync::<AudioSource>();
    }

    #[test]
    fn extensions_are_wav_and_ogg() {
        assert_eq!(AudioLoader.extensions(), &["wav", "ogg"]);
    }

    #[test]
    fn garbage_bytes_fail_to_decode() {
        let ctx = LoadContext {
            id: spawn_asset::AssetId::from_raw(1),
            canonical_path: "x.wav",
            extension: "wav",
        };
        assert!(AudioLoader.load(&[0, 1, 2, 3, 4, 5], &ctx).is_err());
    }
}
