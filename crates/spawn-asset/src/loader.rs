//! Asset loaders: the [`AssetLoader`] trait, its private type-erased shim, the
//! [`LoadContext`] passed to a load, and the two built-in loaders.
//!
//! A loader's [`AssetLoader::load`] runs on an IO worker thread, never the main
//! thread. It receives the already-read file bytes and must be a pure
//! `bytes -> Output` transformation: it must not touch the filesystem and must
//! return errors rather than panic.

use crate::error::{AssetError, AssetResult};
use crate::handle::Asset;
use crate::id::AssetId;

pub struct LoadContext<'a> {
    pub id: AssetId,
    pub canonical_path: &'a str,
    pub extension: &'a str,
}

pub trait AssetLoader: Send + Sync + 'static {
    type Output: Asset;

    /// Extensions this loader claims: lowercase, without a leading dot
    /// (e.g. `["txt", "md"]`). Dispatch matches a path's extension
    /// case-insensitively against these.
    fn extensions(&self) -> &'static [&'static str];

    /// Transforms file bytes into the output asset. Runs on an IO worker thread.
    fn load(&self, bytes: &[u8], ctx: &LoadContext) -> AssetResult<Self::Output>;
}

/// Boxed payload produced by an erased loader. The concrete type is recovered
/// downstream by downcasting through the registry's typed slot.
pub(crate) type ErasedPayload = Box<dyn std::any::Any + Send>;

/// Object-safe shim over [`AssetLoader`] that hides the associated `Output`
/// type so the server can store heterogeneous loaders as trait objects.
pub(crate) trait ErasedLoader: Send + Sync + 'static {
    fn load_erased(&self, bytes: &[u8], ctx: &LoadContext) -> AssetResult<ErasedPayload>;
}

pub(crate) struct LoaderShim<L: AssetLoader>(pub(crate) L);

impl<L: AssetLoader> ErasedLoader for LoaderShim<L> {
    fn load_erased(&self, bytes: &[u8], ctx: &LoadContext) -> AssetResult<ErasedPayload> {
        let output = self.0.load(bytes, ctx)?;
        Ok(Box::new(output))
    }
}

pub struct BinaryAsset(pub Vec<u8>);

pub struct TextAsset(pub String);

pub struct BinaryLoader;

impl AssetLoader for BinaryLoader {
    type Output = BinaryAsset;

    fn extensions(&self) -> &'static [&'static str] {
        &["bin", "dat"]
    }

    fn load(&self, bytes: &[u8], _ctx: &LoadContext) -> AssetResult<Self::Output> {
        Ok(BinaryAsset(bytes.to_vec()))
    }
}

pub struct TextLoader;

impl AssetLoader for TextLoader {
    type Output = TextAsset;

    fn extensions(&self) -> &'static [&'static str] {
        &["txt", "md", "json", "toml", "lua", "wgsl", "glsl"]
    }

    fn load(&self, bytes: &[u8], ctx: &LoadContext) -> AssetResult<Self::Output> {
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(TextAsset(text.to_owned())),
            Err(err) => Err(AssetError::InvalidUtf8 {
                path: ctx.canonical_path.to_owned(),
                offset: err.valid_up_to(),
            }),
        }
    }
}

/// Registers both built-in loaders ([`BinaryLoader`], [`TextLoader`]). Returns
/// [`AssetError::DuplicateLoader`] if either extension is already claimed.
pub fn register_builtin_loaders(server: &mut crate::server::AssetServer) -> AssetResult<()> {
    server.register_loader(BinaryLoader)?;
    server.register_loader(TextLoader)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(path: &'a str, ext: &'a str) -> LoadContext<'a> {
        LoadContext {
            id: AssetId::from_raw(1),
            canonical_path: path,
            extension: ext,
        }
    }

    #[test]
    fn binary_copies_verbatim() {
        let out = BinaryLoader.load(&[1, 2, 3], &ctx("a.bin", "bin")).unwrap();
        assert_eq!(out.0, vec![1, 2, 3]);
    }

    #[test]
    fn text_validates_utf8() {
        match TextLoader.load(b"hello", &ctx("a.txt", "txt")) {
            Ok(ok) => assert_eq!(ok.0, "hello"),
            Err(e) => panic!("expected ok, got {e}"),
        }
        match TextLoader.load(&[0xff, 0xfe], &ctx("a.txt", "txt")) {
            Ok(_) => panic!("expected utf8 error"),
            Err(err) => assert!(matches!(err, AssetError::InvalidUtf8 { offset: 0, .. })),
        }
    }

    #[test]
    fn erased_shim_boxes_output() {
        let shim = LoaderShim(TextLoader);
        let any = shim.load_erased(b"hi", &ctx("a.txt", "txt")).unwrap();
        assert!(any.downcast::<TextAsset>().is_ok());
    }
}
