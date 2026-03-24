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

    // Window descriptor (required when single_segment=0).
    // Format: byte = (exponent << 3) | mantissa, where window = (1 + mantissa/8) << (10 + exponent).
    // With mantissa=0: window = 1 << (10 + exponent), so exponent = window_log - 10.
    // Clamp to 17 until Step 2 enables variable window sizes.
    let window_log = cfg.window_log.min(17);
    let window_byte = ((window_log - 10) as u8) << 3;
    out.push(window_byte);

    // Content size (4 bytes, FCS_flag=2)
    out.extend_from_slice(&(content_size as u32).to_le_bytes());

    // Encode blocks
    let mut pos = 0usize;
    while pos < input.len() {
        let block_end = (pos + MAX_BLOCK_SIZE).min(input.len());
        let block_data = &input[pos..block_end];
        let is_last = block_end == input.len();

        let repeated_byte = repeated_byte(block_data);
        let compressed =
            if repeated_byte.is_none() && should_attempt_compressed_block(block_data, cfg) {
                Some(encode_block(block_data, cfg)?)
            } else {
                None
            };

        let use_rle = repeated_byte.is_some();
        let use_compressed = !use_rle
            && compressed
                .as_ref()
                .is_some_and(|payload| payload.len() < block_data.len());
        let block_type = if use_rle {
            1
        } else if use_compressed {
            2
        } else {
            0
        };
        let block_size = if use_rle {
            block_data.len()
        } else if use_compressed {
            compressed.as_ref().unwrap().len()
        } else {
            block_data.len()
        };
        // Block header: 3 bytes, little-endian
        // [last:1][type:2][size:21]
        let header_val: u32 =
            (is_last as u32) | ((block_type as u32) << 1) | ((block_size as u32) << 3);
        out.extend_from_slice(&header_val.to_le_bytes()[..3]);
        if let Some(byte) = repeated_byte {
            out.push(byte);
        } else if use_compressed {
            out.extend_from_slice(compressed.as_ref().unwrap());
        } else {
            out.extend_from_slice(block_data);
        }

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

fn should_attempt_compressed_block(data: &[u8], cfg: &MatchConfig) -> bool {
    !looks_incompressible(data, cfg)
}

fn repeated_byte(data: &[u8]) -> Option<u8> {
    let (&first, rest) = data.split_first()?;
    let repeated_word = u64::from_ne_bytes([first; 8]);
    let mut chunks = rest.chunks_exact(8);
    for chunk in &mut chunks {
        let word = unsafe { chunk.as_ptr().cast::<u64>().read_unaligned() };
        if word != repeated_word {
            return None;
        }
    }

    if chunks.remainder().iter().all(|&byte| byte == first) {
        Some(first)
    } else {
        None
    }
}

fn looks_incompressible(data: &[u8], cfg: &MatchConfig) -> bool {
    if data.len() < 8 * 1024 || cfg.search_depth() > 8 {
        return false;
    }

    const SAMPLE_BYTES: usize = 4 * 1024;
    const SAMPLE_STRIDE: usize = 8;
    const SAMPLE_HASH_LOG: usize = 9;
    const SAMPLE_TABLE_SIZE: usize = 1 << SAMPLE_HASH_LOG;
    const SAMPLE_HASH_PRIME: u64 = 0x9E37_79B1_9E37_79B1;

    let sample_end = data.len().saturating_sub(4).min(SAMPLE_BYTES);
    if sample_end < SAMPLE_STRIDE * 8 {
        return false;
    }

    let mut table = [u32::MAX; SAMPLE_TABLE_SIZE];
    let mut exact_repeats = 0usize;

    for pos in (0..=sample_end).step_by(SAMPLE_STRIDE) {
        let seq = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        let hash =
            ((u64::from(seq).wrapping_mul(SAMPLE_HASH_PRIME)) >> (64 - SAMPLE_HASH_LOG)) as usize;
        if table[hash] == seq {
            exact_repeats += 1;
            if exact_repeats > 1 {
                return false;
            }
        }
        table[hash] = seq;
    }

    true
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

        let payload_size = if block_type == 1 { 1 } else { block_size };
        if pos + payload_size > input.len() {
            return Err(ZstdError::UnexpectedEof);
        }
        let block_data = &input[pos..pos + payload_size];
        pos += payload_size;

        decode_block(
            block_data,
            block_type,
            block_size,
            &mut repeat_offsets,
            &mut output,
        )?;

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
