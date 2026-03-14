//! zstd frame format: reading and writing the frame header, block loop, and checksum.
//!
//! Frame layout:
//! ```text
//! Magic (4 bytes) | Frame_Header | Block+ | [Checksum (4 bytes)]
//! ```
//!
//! # References
//! - <https://github.com/facebook/zstd/blob/dev/doc/zstd_compression_format.md#frame-concatenation>

use crate::decoder::decode_block;
use crate::encoder::MatchConfig;
use crate::encoder::block::encode_block;
use crate::error::{Result, ZstdError};
use crate::xxhash::xxhash32;

/// Magic number identifying a zstd frame.
pub const MAGIC: u32 = 0xFD2F_B528;

/// Maximum block size (128 KiB).
const MAX_BLOCK_SIZE: usize = 128 * 1024;

/// Compress `input` into a zstd frame using the given compression level (1-22).
pub fn compress(input: &[u8], level: i32) -> Result<Vec<u8>> {
    if level < 1 || level > 22 {
        return Err(ZstdError::InvalidLevel(level));
    }
    let cfg = MatchConfig::for_level(level);
    compress_with_config(input, &cfg, true)
}

/// Compress using an explicit `MatchConfig`, optionally including a content checksum.
pub fn compress_with_config(
    input: &[u8],
    cfg: &MatchConfig,
    content_checksum: bool,
) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() / 2 + 64);

    // Magic
    out.extend_from_slice(&MAGIC.to_le_bytes());

    // Frame Header Descriptor (FHD)
    // Bits: [FCS_flag:2][single_segment:1][unused:1][checksum:1][dict_id_flag:2][reserved:1]
    // We always use FCS_flag=2 (4-byte content size), single_segment=0, no dict.
    let content_size = input.len() as u64;
    let checksum_flag: u8 = if content_checksum { 1 } else { 0 };
    // FCS_flag=2 → bits [7:6] = 0b10
    let fhd: u8 = (2u8 << 6) | (checksum_flag << 2);
    out.push(fhd);

    // Window descriptor (required when single_segment=0)
    // Encode 128 KiB: exponent=7, mantissa=0 → byte = (7 << 3) | 0 = 56
    out.push(56u8);

    // Content size (4 bytes, FCS_flag=2)
    out.extend_from_slice(&(content_size as u32).to_le_bytes());

    // Encode blocks
    let mut pos = 0usize;
    while pos < input.len() {
        let block_end = (pos + MAX_BLOCK_SIZE).min(input.len());
        let block_data = &input[pos..block_end];
        let is_last = block_end == input.len();

        let compressed = encode_block(block_data, cfg)?;

        // Decide whether to use compressed or raw block
        let (block_type, payload): (u8, &[u8]) = if compressed.len() < block_data.len() {
            (2, &compressed)
        } else {
            (0, block_data)
        };

        let block_size = payload.len();
        // Block header: 3 bytes, little-endian
        // [last:1][type:2][size:21]
        let header_val: u32 =
            ((is_last as u32) | ((block_type as u32) << 1) | ((block_size as u32) << 3)) as u32;
        out.extend_from_slice(&header_val.to_le_bytes()[..3]);
        out.extend_from_slice(payload);

        pos = block_end;
    }

    // Handle empty input: emit a single empty last block
    if input.is_empty() {
        let header_val: u32 = 1 | (0 << 1) | (0 << 3); // last=1, type=0, size=0
        out.extend_from_slice(&header_val.to_le_bytes()[..3]);
    }

    // Content checksum
    if content_checksum {
        let checksum = xxhash32(input, 0);
        out.extend_from_slice(&checksum.to_le_bytes());
    }

    Ok(out)
}

/// Decompress a zstd frame from `input`.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>> {
    let mut pos = 0usize;

    // Magic
    if input.len() < 4 {
        return Err(ZstdError::UnexpectedEof);
    }
    let magic = u32::from_le_bytes(input[0..4].try_into().unwrap());
    if magic != MAGIC {
        return Err(ZstdError::InvalidMagic(magic));
    }
    pos += 4;

    // Frame Header Descriptor
    if pos >= input.len() {
        return Err(ZstdError::UnexpectedEof);
    }
    let fhd = input[pos];
    pos += 1;

    let dict_id_flag = fhd & 0x3;
    let content_checksum_flag = (fhd >> 2) & 0x1;
    let single_segment = (fhd >> 5) & 0x1 != 0;
    let content_size_flag = (fhd >> 6) & 0x3;
    let _reserved = (fhd >> 3) & 0x1; // must be 0

    // Window descriptor (only if not single_segment)
    let _window_size: u64 = if single_segment {
        0 // determined by content size
    } else {
        if pos >= input.len() {
            return Err(ZstdError::UnexpectedEof);
        }
        let wd = input[pos];
        pos += 1;
        let mantissa = (wd & 0x7) as u64;
        let exponent = (wd >> 3) as u64;
        (1 + mantissa * 8) * (1u64 << (10 + exponent))
    };

    // Dictionary id
    let _dict_id_size = [0, 1, 2, 4][dict_id_flag as usize];
    pos += _dict_id_size;

    // Content size
    // FCS_flag: 0=no field (unless single_segment→1 byte), 1=2 bytes, 2=4 bytes, 3=8 bytes
    let content_size: Option<u64> = match (single_segment, content_size_flag) {
        (true, 0) => {
            // Single segment, no explicit flag → 1 byte
            if pos >= input.len() {
                return Err(ZstdError::UnexpectedEof);
            }
            let v = input[pos] as u64;
            pos += 1;
            Some(v)
        }
        (_, 0) => None,
        (_, 1) => {
            // 2 bytes
            if pos + 2 > input.len() {
                return Err(ZstdError::UnexpectedEof);
            }
            let v = u16::from_le_bytes(input[pos..pos + 2].try_into().unwrap()) as u64 + 256;
            pos += 2;
            Some(v)
        }
        (_, 2) => {
            // 4 bytes
            if pos + 4 > input.len() {
                return Err(ZstdError::UnexpectedEof);
            }
            let v = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap()) as u64;
            pos += 4;
            Some(v)
        }
        (_, 3) => {
            // 8 bytes
            if pos + 8 > input.len() {
                return Err(ZstdError::UnexpectedEof);
            }
            let v = u64::from_le_bytes(input[pos..pos + 8].try_into().unwrap());
            pos += 8;
            Some(v)
        }
        _ => None,
    };

    // Decode blocks
    let mut output: Vec<u8> = Vec::with_capacity(content_size.unwrap_or(4096) as usize);
    let mut repeat_offsets = [1usize, 4, 8];
    loop {
        if pos + 3 > input.len() {
            return Err(ZstdError::UnexpectedEof);
        }
        let h0 = input[pos] as u32;
        let h1 = input[pos + 1] as u32;
        let h2 = input[pos + 2] as u32;
        pos += 3;
        let header_val = h0 | (h1 << 8) | (h2 << 16);

        let last_block = (header_val & 1) != 0;
        let block_type = ((header_val >> 1) & 0x3) as u8;
        let block_size = (header_val >> 3) as usize;

        if pos + block_size > input.len() {
            return Err(ZstdError::UnexpectedEof);
        }
        let block_data = &input[pos..pos + block_size];
        pos += block_size;

        let history_start = output.len().saturating_sub(128 * 1024);
        let history = output[history_start..].to_vec();
        decode_block(block_data, block_type, block_size, &history, &mut repeat_offsets, &mut output)?;

        if last_block {
            break;
        }
    }

    // Content checksum
    if content_checksum_flag != 0 {
        if pos + 4 > input.len() {
            return Err(ZstdError::UnexpectedEof);
        }
        let stored = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap());
        let computed = xxhash32(&output, 0);
        if stored != computed {
            return Err(ZstdError::ChecksumMismatch {
                expected: stored,
                actual: computed,
            });
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_decompress_empty() {
        let data = b"";
        let compressed = compress(data, 3).unwrap();
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_compress_decompress_small() {
        let data = b"hello world";
        let compressed = compress(data, 3).unwrap();
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_compress_decompress_repetitive() {
        let data = b"aaaaaaaaaa".repeat(1000);
        let compressed = compress(&data, 3).unwrap();
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, data.as_slice());
        // Repetitive data should compress well
        assert!(compressed.len() < data.len() / 2);
    }

    #[test]
    fn test_invalid_magic() {
        let bad = b"\x00\x00\x00\x00hello";
        let result = decompress(bad);
        assert!(matches!(result, Err(ZstdError::InvalidMagic(_))));
    }

    #[test]
    fn test_checksum_mismatch() {
        let data = b"hello world";
        let mut compressed = compress(data, 1).unwrap();
        // Corrupt the checksum (last 4 bytes)
        let len = compressed.len();
        compressed[len - 1] ^= 0xFF;
        let result = decompress(&compressed);
        assert!(matches!(result, Err(ZstdError::ChecksumMismatch { .. })));
    }

    #[test]
    fn test_various_sizes() {
        for size in [0, 1, 255, 256, 1024, 10_000, 100_000] {
            let data: Vec<u8> = (0..size).map(|i| (i * 7 + 13) as u8).collect();
            let compressed = compress(&data, 1).unwrap();
            let decompressed = decompress(&compressed).unwrap();
            assert_eq!(decompressed, data, "failed at size {size}");
        }
    }

    #[test]
    fn test_level_range() {
        let data = b"test data for compression level testing";
        for level in [1, 3, 6, 9, 12, 22] {
            let compressed = compress(data, level).unwrap();
            let decompressed = decompress(&compressed).unwrap();
            assert_eq!(&decompressed, data, "failed at level {level}");
        }
    }
}
