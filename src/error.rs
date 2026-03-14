//! Error types for the zstd_rs library.

use thiserror::Error;

/// Errors that can occur during compression or decompression.
#[derive(Debug, Error)]
pub enum ZstdError {
    #[error("Invalid magic number: expected 0xFD2FB528, got {0:#010x}")]
    InvalidMagic(u32),

    #[error("Unsupported frame type")]
    UnsupportedFrameType,

    #[error("Corrupt data: {0}")]
    CorruptData(&'static str),

    #[error("Checksum mismatch: expected {expected:#010x}, got {actual:#010x}")]
    ChecksumMismatch { expected: u32, actual: u32 },

    #[error("Invalid block type {0}")]
    InvalidBlockType(u8),

    #[error("Decompressed size exceeds limit: {0}")]
    SizeLimit(u64),

    #[error("Unexpected end of input")]
    UnexpectedEof,

    #[error("Huffman table error: {0}")]
    HuffmanError(&'static str),

    #[error("FSE table error: {0}")]
    FseError(&'static str),

    #[error("Sequence decode error: {0}")]
    SequenceError(&'static str),

    #[error("Window too large: {0} bytes")]
    WindowTooLarge(u64),

    #[error("Invalid level {0}: must be 1-22")]
    InvalidLevel(i32),
}

pub type Result<T> = std::result::Result<T, ZstdError>;
