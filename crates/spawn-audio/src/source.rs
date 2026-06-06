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

    pub fn channels(&self) -> u16 {
        self.inner.channels()
    }

    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
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
        let sound = decode_source(bytes).map_err(map_decode_error)?;
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
