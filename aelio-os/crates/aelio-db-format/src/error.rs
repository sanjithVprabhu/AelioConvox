use std::fmt;
use std::io;

/// Result alias for the file-format crate.
pub type Result<T> = std::result::Result<T, FormatError>;

/// Errors produced while reading or writing an LL file.
#[derive(Debug)]
pub enum FormatError {
    /// Underlying I/O failure.
    Io(io::Error),
    /// A magic number did not match (file is not an LL file, or end marker corrupt).
    BadMagic { expected: u32, found: u32 },
    /// The file's `format_version` is newer/older than this reader supports.
    UnsupportedVersion(u16),
    /// The footer's CRC32C did not match — the footer bytes are corrupt.
    FooterCrcMismatch { expected: u32, found: u32 },
    /// A section's CRC32C did not match — that section's bytes are corrupt.
    SectionCrcMismatch {
        section: String,
        expected: u32,
        found: u32,
    },
    /// A column-chunk page's CRC32C did not match — that page's bytes are corrupt.
    PageCrcMismatch {
        column_id: u32,
        page_index: u32,
        expected: u32,
        found: u32,
    },
    /// The file is shorter than a structure requires.
    Truncated(&'static str),
    /// A field could not be decoded (bad enum tag, length mismatch, etc.).
    Decode(String),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Io(e) => write!(f, "io error: {e}"),
            FormatError::BadMagic { expected, found } => {
                write!(f, "bad magic: expected {expected:#010x}, found {found:#010x}")
            }
            FormatError::UnsupportedVersion(v) => write!(f, "unsupported format version: {v}"),
            FormatError::FooterCrcMismatch { expected, found } => write!(
                f,
                "footer CRC mismatch: expected {expected:#010x}, found {found:#010x}"
            ),
            FormatError::SectionCrcMismatch {
                section,
                expected,
                found,
            } => write!(
                f,
                "section '{section}' CRC mismatch: expected {expected:#010x}, found {found:#010x}"
            ),
            FormatError::PageCrcMismatch {
                column_id,
                page_index,
                expected,
                found,
            } => write!(
                f,
                "column {column_id} page {page_index} CRC mismatch: expected {expected:#010x}, found {found:#010x}"
            ),
            FormatError::Truncated(what) => write!(f, "truncated file: {what}"),
            FormatError::Decode(msg) => write!(f, "decode error: {msg}"),
        }
    }
}

impl std::error::Error for FormatError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FormatError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for FormatError {
    fn from(e: io::Error) -> Self {
        FormatError::Io(e)
    }
}
