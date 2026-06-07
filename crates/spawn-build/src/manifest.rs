//! Hand-parsed line-based build manifest (`assets.manifest`).

use std::path::{Path, PathBuf};

use crate::error::{BuildError, BuildResult};
use crate::glob::Pattern;

/// Parsed build manifest.
///
/// `include` is never empty after parsing: when the manifest declares no
/// `include`, it defaults to the single pattern `**` so discovery has a
/// well-defined match set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildManifest {
    pub source_root: PathBuf,
    pub output_dir: PathBuf,
    pub include: Vec<Pattern>,
    pub exclude: Vec<Pattern>,
}

impl BuildManifest {
    pub fn parse_file(manifest_path: &Path) -> BuildResult<Self> {
        let text = std::fs::read_to_string(manifest_path).map_err(|source| BuildError::Io {
            path: manifest_path.to_path_buf(),
            source,
        })?;
        let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
        Self::parse_str(&text, manifest_dir)
    }

    pub fn parse_str(text: &str, manifest_dir: &Path) -> BuildResult<Self> {
        let mut source_root: Option<PathBuf> = None;
        let mut output_dir: Option<PathBuf> = None;
        let mut include = Vec::new();
        let mut exclude = Vec::new();

        for (idx, raw_line) in text.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = raw_line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let (key, value) = match raw_line.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => {
                    return Err(BuildError::ManifestParse {
                        line: line_no,
                        detail: "missing `=` in directive",
                    })
                }
            };
            match key {
                "source_root" => {
                    source_root = Some(manifest_dir.join(value));
                }
                "output_dir" => {
                    output_dir = Some(manifest_dir.join(value));
                }
                "include" => {
                    include.push(Pattern::compile(value)?);
                }
                "exclude" => {
                    exclude.push(Pattern::compile(value)?);
                }
                _ => {
                    return Err(BuildError::ManifestParse {
                        line: line_no,
                        detail: "unrecognized key",
                    })
                }
            }
        }

        let output_dir = output_dir.ok_or(BuildError::ManifestMissingKey { key: "output_dir" })?;
        let source_root = source_root.unwrap_or_else(|| manifest_dir.join("."));
        if include.is_empty() {
            include.push(Pattern::compile("**")?);
        }

        Ok(Self {
            source_root,
            output_dir,
            include,
            exclude,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_manifest() {
        let text = "\
# top comment
source_root = assets

output_dir = build/out
include = **/*.png
include = textures/*.ktx
exclude = **/*.tmp
# trailing comment
";
        let dir = Path::new("/proj");
        let manifest = BuildManifest::parse_str(text, dir).unwrap();
        assert_eq!(manifest.source_root, PathBuf::from("/proj/assets"));
        assert_eq!(manifest.output_dir, PathBuf::from("/proj/build/out"));
        assert_eq!(manifest.include.len(), 2);
        assert_eq!(manifest.include[0].as_str(), "**/*.png");
        assert_eq!(manifest.exclude.len(), 1);
    }

    #[test]
    fn default_source_root_and_include() {
        let manifest = BuildManifest::parse_str("output_dir = out\n", Path::new("/p")).unwrap();
        assert_eq!(manifest.source_root, PathBuf::from("/p/."));
        assert_eq!(manifest.include.len(), 1);
        assert_eq!(manifest.include[0].as_str(), "**");
    }

    #[test]
    fn missing_equals_reports_line() {
        let text = "output_dir = out\nbad line here\n";
        match BuildManifest::parse_str(text, Path::new("/p")) {
            Err(BuildError::ManifestParse { line, .. }) => assert_eq!(line, 2),
            other => panic!("expected ManifestParse, got {other:?}"),
        }
    }

    #[test]
    fn unknown_key_reports_line() {
        let text = "output_dir = out\n\nfoo = bar\n";
        match BuildManifest::parse_str(text, Path::new("/p")) {
            Err(BuildError::ManifestParse { line, detail }) => {
                assert_eq!(line, 3);
                assert_eq!(detail, "unrecognized key");
            }
            other => panic!("expected ManifestParse, got {other:?}"),
        }
    }

    #[test]
    fn missing_output_dir_is_missing_key() {
        let text = "source_root = a\n";
        assert!(matches!(
            BuildManifest::parse_str(text, Path::new("/p")),
            Err(BuildError::ManifestMissingKey { key: "output_dir" })
        ));
    }

    #[test]
    fn value_may_contain_equals() {
        let manifest = BuildManifest::parse_str("output_dir = a=b\n", Path::new("/p")).unwrap();
        assert_eq!(manifest.output_dir, PathBuf::from("/p/a=b"));
    }
}
