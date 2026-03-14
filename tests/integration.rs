//! Integration tests for zstd_rs.
//!
//! These tests verify round-trip correctness across a variety of data patterns
//! and compression levels.

use zstd_rs::{compress, compress_bound, decompress};
use zstd_rs::error::ZstdError;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn roundtrip(data: &[u8], level: i32) {
    let compressed = compress(data, level)
        .unwrap_or_else(|e| panic!("compress failed at level {level}: {e}"));
    let decompressed = decompress(&compressed)
        .unwrap_or_else(|e| panic!("decompress failed at level {level}: {e}"));
    assert_eq!(
        decompressed, data,
        "round-trip mismatch at level {level} for {} bytes",
        data.len()
    );
}

fn benchmark_repetitive_corpus(size: usize) -> Vec<u8> {
    b"the quick brown fox jumps over the lazy dog. ".repeat(size / 45 + 1)[..size].to_vec()
}

// ── Empty / trivial ──────────────────────────────────────────────────────────

#[test]
fn roundtrip_empty() {
    roundtrip(b"", 3);
}

#[test]
fn roundtrip_single_byte() {
    roundtrip(b"X", 3);
}

#[test]
fn roundtrip_two_bytes() {
    roundtrip(b"AB", 3);
}

// ── Text-like data ───────────────────────────────────────────────────────────

#[test]
fn roundtrip_ascii_sentence() {
    roundtrip(b"The quick brown fox jumps over the lazy dog.", 3);
}

#[test]
fn roundtrip_lorem_ipsum() {
    let text = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
                 Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
                 Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris.";
    roundtrip(text, 3);
}

#[test]
fn roundtrip_repeated_text() {
    let text = b"hello world! ".repeat(500);
    roundtrip(&text, 3);
}

#[test]
fn roundtrip_benchmark_repetitive_16k() {
    let data = benchmark_repetitive_corpus(16 * 1024);
    roundtrip(&data, 3);
}

// ── Binary patterns ──────────────────────────────────────────────────────────

#[test]
fn roundtrip_all_zeros() {
    let data = vec![0u8; 10_000];
    roundtrip(&data, 3);
}

#[test]
fn roundtrip_all_same_byte() {
    let data = vec![0xABu8; 8_192];
    roundtrip(&data, 3);
}

#[test]
fn roundtrip_sequential_bytes() {
    let data: Vec<u8> = (0u8..=255).cycle().take(5_000).collect();
    roundtrip(&data, 3);
}

#[test]
fn roundtrip_pseudo_random() {
    // Simple LCG pseudo-random to get repeatable "random" bytes
    let mut state: u64 = 0xDEAD_BEEF_1234_5678;
    let data: Vec<u8> = (0..50_000)
        .map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect();
    roundtrip(&data, 1);
}

// ── Large blocks ─────────────────────────────────────────────────────────────

#[test]
fn roundtrip_exactly_one_block() {
    // 128 KiB = MAX_BLOCK_SIZE
    let data: Vec<u8> = (0..131_072).map(|i| (i * 3) as u8).collect();
    roundtrip(&data, 1);
}

#[test]
fn roundtrip_multi_block() {
    let data: Vec<u8> = (0..400_000).map(|i| (i as u8).wrapping_mul(7)).collect();
    roundtrip(&data, 1);
}

// ── Compression levels ───────────────────────────────────────────────────────

#[test]
fn roundtrip_all_fast_levels() {
    let data = b"zstd level testing data. ".repeat(200);
    for level in 1..=3 {
        roundtrip(&data, level);
    }
}

#[test]
fn roundtrip_mid_levels() {
    let data = b"zstd level testing data. ".repeat(200);
    for level in [4, 6, 9, 12] {
        roundtrip(&data, level);
    }
}

#[test]
fn roundtrip_high_levels() {
    let data = b"high compression ratio test. ".repeat(100);
    for level in [15, 19, 22] {
        roundtrip(&data, level);
    }
}

// ── Error handling ───────────────────────────────────────────────────────────

#[test]
fn invalid_level_too_low() {
    assert!(matches!(compress(b"x", 0), Err(ZstdError::InvalidLevel(0))));
}

#[test]
fn invalid_level_too_high() {
    assert!(matches!(compress(b"x", 23), Err(ZstdError::InvalidLevel(23))));
}

#[test]
fn decompress_wrong_magic() {
    let result = decompress(b"\x00\x00\x00\x00hello world");
    assert!(matches!(result, Err(ZstdError::InvalidMagic(_))));
}

#[test]
fn decompress_truncated_frame() {
    let compressed = compress(b"hello world", 3).unwrap();
    // Truncate halfway through
    let truncated = &compressed[..compressed.len() / 2];
    assert!(decompress(truncated).is_err());
}

#[test]
fn decompress_corrupt_checksum() {
    let mut compressed = compress(b"hello world", 3).unwrap();
    let len = compressed.len();
    compressed[len - 1] ^= 0xFF;
    assert!(matches!(
        decompress(&compressed),
        Err(ZstdError::ChecksumMismatch { .. })
    ));
}

// ── compress_bound ───────────────────────────────────────────────────────────

#[test]
fn compress_bound_always_sufficient() {
    for size in [0, 1, 100, 10_000, 100_000] {
        let data: Vec<u8> = (0..size).map(|i| i as u8).collect();
        let compressed = compress(&data, 1).unwrap();
        let bound = compress_bound(size);
        assert!(
            compressed.len() <= bound,
            "compress_bound({size}) = {bound} < actual {}", compressed.len()
        );
    }
}

// ── Compression ratio sanity ─────────────────────────────────────────────────

#[test]
fn highly_compressible_data_shrinks() {
    let data = vec![b'A'; 100_000];
    let compressed = compress(&data, 3).unwrap();
    // Should compress to well under 1% of original
    assert!(
        compressed.len() < 1_000,
        "expected <1000 bytes for 100K zeros, got {}",
        compressed.len()
    );
}

#[test]
fn already_random_data_does_not_expand_much() {
    let data: Vec<u8> = (0..10_000u32)
        .map(|i| ((i.wrapping_mul(2654435769)) >> 24) as u8)
        .collect();
    let compressed = compress(&data, 1).unwrap();
    let bound = compress_bound(data.len());
    assert!(
        compressed.len() <= bound,
        "compressed size {} exceeds bound {}",
        compressed.len(),
        bound
    );
}
