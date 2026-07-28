//! Pointer & scope path grammar (§6.1).
//!
//! `path = segment ("." segment | index)*` · `segment = identifier | "[" quoted-string "]"` ·
//! `index = "[" int "]"`. Identifiers `[A-Za-z_][A-Za-z0-9_]*`; quoted-bracket form reaches
//! non-identifier keys (`["user-id"]`) so no key is pointer-unreachable. Max depth 32 (§4.4).
//!
//! **All instruction paths are literals** (§4.2.5): this parser accepts only static path *text*.
//! It structurally cannot express a computed path or a root `into` — a `Path` is always ≥1 segment
//! rooted at a body key, never the whole bag.

use crate::error::{LimitError, SolError, SolResult};
use crate::value::SolValue;
use std::fmt;

const MAX_PATH_DEPTH: usize = 32;

/// A parsed, validated literal path — a non-empty sequence of segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// A map key (identifier or quoted-bracket form).
    Key(String),
    /// A list index (`[int]`).
    Index(usize),
}

impl Path {
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Parse per §6.1. Rejects computed paths (there is no syntax for them), empty paths, and paths
    /// deeper than 32 segments. The first element must be a `segment` (key), never an `index` — the
    /// root is a body map, not a list.
    pub fn parse(text: &str) -> SolResult<Path> {
        let bytes = text.as_bytes();
        let mut pos = 0usize;
        let mut segments = Vec::new();

        // First element: a key segment (identifier or quoted-bracket).
        let (seg, next) = parse_key_segment(bytes, pos)?;
        segments.push(seg);
        pos = next;

        while pos < bytes.len() {
            match bytes[pos] {
                b'.' => {
                    pos += 1;
                    let (seg, next) = parse_key_segment(bytes, pos)?;
                    segments.push(seg);
                    pos = next;
                }
                b'[' => {
                    // After a segment, `[` may only begin an `index` (a quoted key needs a leading
                    // `.` per the grammar). A quoted bracket here is a syntax error.
                    if bytes.get(pos + 1) == Some(&b'"') {
                        return Err(SolError::PathSyntax {
                            at: pos,
                            why: "quoted key must follow '.' (grammar: `.` segment)",
                        });
                    }
                    let (idx, next) = parse_index(bytes, pos)?;
                    segments.push(Segment::Index(idx));
                    pos = next;
                }
                _ => {
                    return Err(SolError::PathSyntax {
                        at: pos,
                        why: "expected '.' or '['",
                    });
                }
            }
            if segments.len() > MAX_PATH_DEPTH {
                return Err(SolError::Limit(LimitError::Depth {
                    max: MAX_PATH_DEPTH,
                }));
            }
        }

        Ok(Path { segments })
    }

    /// Resolve the path against a root value; `None` on any missing key / out-of-range index /
    /// type mismatch (§6.4: absence is not fabrication — the caller decides skip vs default).
    pub fn get<'a>(&self, root: &'a SolValue) -> Option<&'a SolValue> {
        let mut cur = root;
        for seg in &self.segments {
            cur = match (seg, cur) {
                (Segment::Key(k), SolValue::Map(m)) => m.get(k)?,
                (Segment::Index(i), SolValue::List(l)) => l.get(*i)?,
                _ => return None,
            };
        }
        Some(cur)
    }

    /// §9 `exists`: true iff the path resolves.
    pub fn exists(&self, root: &SolValue) -> bool {
        self.get(root).is_some()
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, segment) in self.segments.iter().enumerate() {
            match segment {
                Segment::Key(key)
                    if key.bytes().enumerate().all(|(i, byte)| {
                        byte == b'_'
                            || byte.is_ascii_alphabetic()
                            || (i > 0 && byte.is_ascii_digit())
                    }) =>
                {
                    if index > 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(key)?;
                }
                Segment::Key(key) => {
                    if index > 0 {
                        f.write_str(".")?;
                    }
                    let quoted = crate::canonical::to_string(&SolValue::Str(key.clone()));
                    write!(f, "[{quoted}]")?;
                }
                Segment::Index(value) => write!(f, "[{value}]")?,
            }
        }
        Ok(())
    }
}

fn parse_key_segment(bytes: &[u8], pos: usize) -> SolResult<(Segment, usize)> {
    match bytes.get(pos) {
        Some(b'[') => parse_quoted_key(bytes, pos),
        Some(&c) if is_ident_start(c) => {
            let mut end = pos + 1;
            while end < bytes.len() && is_ident_continue(bytes[end]) {
                end += 1;
            }
            // Safe: identifier bytes are ASCII.
            let ident = std::str::from_utf8(&bytes[pos..end]).unwrap().to_string();
            Ok((Segment::Key(ident), end))
        }
        _ => Err(SolError::PathSyntax {
            at: pos,
            why: "expected identifier or quoted-bracket key",
        }),
    }
}

/// Parse `["...quoted..."]`; supports `\"` and `\\` escapes. Content is a raw key string.
fn parse_quoted_key(bytes: &[u8], pos: usize) -> SolResult<(Segment, usize)> {
    // bytes[pos] == '['
    if bytes.get(pos + 1) != Some(&b'"') {
        return Err(SolError::PathSyntax {
            at: pos,
            why: "expected '\"' after '[' for a quoted key",
        });
    }
    let mut i = pos + 2;
    let mut key = String::new();
    loop {
        match bytes.get(i) {
            None => {
                return Err(SolError::PathSyntax {
                    at: pos,
                    why: "unterminated quoted key",
                })
            }
            Some(b'\\') => match bytes.get(i + 1) {
                Some(b'"') => {
                    key.push('"');
                    i += 2;
                }
                Some(b'\\') => {
                    key.push('\\');
                    i += 2;
                }
                _ => {
                    return Err(SolError::PathSyntax {
                        at: i,
                        why: "invalid escape in quoted key",
                    })
                }
            },
            Some(b'"') => {
                // Closing quote; must be followed by ']'.
                if bytes.get(i + 1) != Some(&b']') {
                    return Err(SolError::PathSyntax {
                        at: i + 1,
                        why: "expected ']' after quoted key",
                    });
                }
                // Reparse the accumulated bytes as UTF-8 to allow multibyte key chars.
                return Ok((Segment::Key(key), i + 2));
            }
            Some(_) => {
                // Copy one UTF-8 code point.
                let ch_len = utf8_len(bytes[i]);
                let slice = &bytes[i..i + ch_len];
                key.push_str(
                    std::str::from_utf8(slice).map_err(|_| SolError::PathSyntax {
                        at: i,
                        why: "invalid UTF-8 in quoted key",
                    })?,
                );
                i += ch_len;
            }
        }
    }
}

fn parse_index(bytes: &[u8], pos: usize) -> SolResult<(usize, usize)> {
    // bytes[pos] == '['
    let mut i = pos + 1;
    let start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return Err(SolError::PathSyntax {
            at: pos,
            why: "expected a non-negative integer index",
        });
    }
    if bytes.get(i) != Some(&b']') {
        return Err(SolError::PathSyntax {
            at: i,
            why: "expected ']' to close index",
        });
    }
    let digits = std::str::from_utf8(&bytes[start..i]).unwrap();
    let idx: usize = digits.parse().map_err(|_| SolError::PathSyntax {
        at: start,
        why: "index out of range",
    })?;
    Ok((idx, i + 1))
}

fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}
fn is_ident_continue(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}
fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}
