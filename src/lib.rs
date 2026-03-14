//! # zstd_rs — A Zstandard Compression Library
//!
//! This crate provides a pure-Rust implementation of the
//! [Zstandard](https://facebook.github.io/zstd/) compression algorithm,
//! written from scratch without any C dependencies.
//!
//! ## Quick Start
//!
//! ```rust
//! use zstd_rs::{compress, decompress};
//!
//! let original = b"Hello, world! This is some data to compress.";
//! let compressed = compress(original, 3).unwrap();
//! let decompressed = decompress(&compressed).unwrap();
//! assert_eq!(decompressed, original);
//! ```
//!
//! ## Compression Levels
//!
//! Levels 1–22 are supported (matching zstd conventions):
//! - **1–3**: Fast, lower compression ratio.
//! - **4–9**: Balanced speed and ratio.
//! - **10–19**: High compression ratio.
//! - **20–22**: Ultra compression (slow).
//!
//! ## Architecture
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`frame`] | Frame header encoding/decoding |
//! | [`encoder`] | Block encoder (LZ77 + literals/sequences packing) |
//! | [`decoder`] | Block decoder (literals + sequence execution) |
//! | [`fse`] | Finite State Entropy coding |
//! | [`huffman`] | Huffman coding for literals |
//! | [`xxhash`] | XXHash-32 content checksum |
//! | [`error`] | Error types |

pub mod decoder;
pub mod encoder;
pub mod error;
pub mod frame;
pub mod fse;
pub mod huffman;
pub mod profiling;
pub mod tables;
pub mod xxhash;

use crate::error::Result;

/// Compress `data` at the given level (1–22).
///
/// Returns a complete zstd frame, compatible with the reference `zstd` tool.
///
/// # Errors
///
/// Returns [`error::ZstdError::InvalidLevel`] if `level` is outside 1–22.
///
/// # Example
///
/// ```rust
/// let compressed = zstd_rs::compress(b"hello world", 3).unwrap();
/// assert!(compressed.len() > 0);
/// ```
pub fn compress(data: &[u8], level: i32) -> Result<Vec<u8>> {
    frame::compress(data, level)
}

/// Decompress a zstd-compressed frame.
///
/// # Errors
///
/// - [`error::ZstdError::InvalidMagic`] — not a zstd frame.
/// - [`error::ZstdError::ChecksumMismatch`] — content checksum failed.
/// - [`error::ZstdError::CorruptData`] — malformed bitstream.
///
/// # Example
///
/// ```rust
/// let compressed = zstd_rs::compress(b"hello world", 3).unwrap();
/// let decompressed = zstd_rs::decompress(&compressed).unwrap();
/// assert_eq!(decompressed, b"hello world");
/// ```
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    frame::decompress(data)
}

/// Estimate the compressed size of `data` at the given level without allocating the output.
///
/// Returns a conservative upper bound: `data.len() + overhead`.
pub fn compress_bound(data_len: usize) -> usize {
    // zstd worst case: input + block headers + frame header + checksum
    data_len + (data_len / 128 + 3) * 3 + 18 + 4
}
