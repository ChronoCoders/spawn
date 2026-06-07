//! Glob-lite pattern matching over canonical relative paths.

use crate::error::{BuildError, BuildResult};

/// A single compiled token within one path segment.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A literal run of characters.
    Literal(String),
    /// `*`: zero or more characters within a single segment (never `/`).
    Star,
}

/// One compiled path segment, or the recursive `**` wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// `**`: matches zero or more whole segments.
    DoubleStar,
    /// A normal segment made of literals and `*` wildcards.
    Tokens(Vec<Token>),
}

/// A compiled glob-lite pattern.
///
/// Exactly two wildcards are supported and this subset is frozen for Phase 1:
/// `*` matches any run of zero or more characters *within one segment* (never
/// crossing `/`); `**` matches zero or more *whole* segments. `**` is only valid
/// as a complete segment — a `**` adjacent to other characters (e.g. `a**b`) is
/// rejected at compile time. Matching is full-path anchored and case-sensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    original: String,
    segments: Vec<Segment>,
}

impl Pattern {
    pub fn compile(pattern: &str) -> BuildResult<Self> {
        let mut segments = Vec::new();
        for raw in pattern.split('/') {
            if raw == "**" {
                segments.push(Segment::DoubleStar);
                continue;
            }
            if raw.contains("**") {
                return Err(BuildError::InvalidPattern {
                    pattern: pattern.to_string(),
                    detail: "`**` is only valid as a whole path segment",
                });
            }
            let mut tokens = Vec::new();
            let mut literal = String::new();
            for ch in raw.chars() {
                if ch == '*' {
                    if !literal.is_empty() {
                        tokens.push(Token::Literal(std::mem::take(&mut literal)));
                    }
                    if !matches!(tokens.last(), Some(Token::Star)) {
                        tokens.push(Token::Star);
                    }
                } else {
                    literal.push(ch);
                }
            }
            if !literal.is_empty() {
                tokens.push(Token::Literal(literal));
            }
            segments.push(Segment::Tokens(tokens));
        }
        Ok(Self {
            original: pattern.to_string(),
            segments,
        })
    }

    pub fn matches(&self, canonical_path: &str) -> bool {
        let path_segments: Vec<&str> = if canonical_path.is_empty() {
            Vec::new()
        } else {
            canonical_path.split('/').collect()
        };
        match_segments(&self.segments, &path_segments)
    }

    pub fn as_str(&self) -> &str {
        &self.original
    }
}

fn match_segments(pattern: &[Segment], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((Segment::DoubleStar, rest)) => {
            // `**` consumes zero or more whole segments.
            for skip in 0..=path.len() {
                if match_segments(rest, &path[skip..]) {
                    return true;
                }
            }
            false
        }
        Some((Segment::Tokens(tokens), rest)) => match path.split_first() {
            Some((head, tail)) if match_tokens(tokens, head) => match_segments(rest, tail),
            _ => false,
        },
    }
}

fn match_tokens(tokens: &[Token], segment: &str) -> bool {
    match tokens.split_first() {
        None => segment.is_empty(),
        Some((Token::Literal(lit), rest)) => match segment.strip_prefix(lit.as_str()) {
            Some(remaining) => match_tokens(rest, remaining),
            None => false,
        },
        Some((Token::Star, rest)) => {
            // `*` consumes zero or more chars within this segment.
            for (idx, _) in segment.char_indices() {
                if match_tokens(rest, &segment[idx..]) {
                    return true;
                }
            }
            match_tokens(rest, "")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, path: &str) -> bool {
        Pattern::compile(pattern).unwrap().matches(path)
    }

    #[test]
    fn star_does_not_cross_separator() {
        assert!(m("a/*.txt", "a/b.txt"));
        assert!(!m("a/*.txt", "a/b/c.txt"));
        assert!(!m("*.txt", "a/b.txt"));
        assert!(m("*.txt", "b.txt"));
    }

    #[test]
    fn double_star_matches_zero_or_more_segments() {
        assert!(m("**", "a/b/c.txt"));
        assert!(m("**", "a.txt"));
        assert!(m("**", ""));
        assert!(m("a/**/c.txt", "a/c.txt"));
        assert!(m("a/**/c.txt", "a/b/c.txt"));
        assert!(m("a/**/c.txt", "a/b/d/c.txt"));
        assert!(m("**/c.txt", "c.txt"));
        assert!(m("**/c.txt", "a/b/c.txt"));
        assert!(m("a/**", "a"));
        assert!(m("a/**", "a/b/c"));
    }

    #[test]
    fn full_anchored_and_case_sensitive() {
        assert!(!m("a/b.txt", "a/b.txt/c"));
        assert!(!m("b.txt", "a/b.txt"));
        assert!(!m("A.txt", "a.txt"));
        assert!(m("a.txt", "a.txt"));
    }

    #[test]
    fn multiple_stars_in_segment() {
        assert!(m("*foo*", "xxfooyy"));
        assert!(m("*foo*", "foo"));
        assert!(!m("*foo*", "bar"));
    }

    #[test]
    fn malformed_double_star_rejected() {
        assert!(matches!(
            Pattern::compile("a**b"),
            Err(BuildError::InvalidPattern { .. })
        ));
        assert!(matches!(
            Pattern::compile("a/x**/c"),
            Err(BuildError::InvalidPattern { .. })
        ));
    }

    #[test]
    fn as_str_returns_original() {
        assert_eq!(Pattern::compile("a/*.txt").unwrap().as_str(), "a/*.txt");
    }
}
