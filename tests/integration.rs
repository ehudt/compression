//! Integration tests for zstd_rs.
//!
//! These tests verify round-trip correctness across a variety of data patterns
//! and compression levels.

use zstd_rs::error::ZstdError;
use zstd_rs::profiling::ProfileSession;
use zstd_rs::{compress, compress_bound, decompress};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn roundtrip(data: &[u8], level: i32) {
    let compressed =
        compress(data, level).unwrap_or_else(|e| panic!("compress failed at level {level}: {e}"));
    let decompressed = decompress(&compressed)
        .unwrap_or_else(|e| panic!("decompress failed at level {level}: {e}"));
    assert_eq!(
        decompressed,
        data,
        "round-trip mismatch at level {level} for {} bytes",
        data.len()
    );
}

fn benchmark_repetitive_corpus(size: usize) -> Vec<u8> {
    b"the quick brown fox jumps over the lazy dog. ".repeat(size / 45 + 1)[..size].to_vec()
}

fn profile_test(test_name: &str) -> ProfileSession {
    ProfileSession::from_test_env(test_name).unwrap_or_else(|message| panic!("{message}"))
}

fn silesia_dickens() -> &'static [u8] {
    include_bytes!(concat!(env!("HOME"), "/silesia/dickens"))
}

// ── Empty / trivial ──────────────────────────────────────────────────────────

#[test]
fn roundtrip_empty() {
    let _profile = profile_test("roundtrip_empty");
    roundtrip(b"", 3);
}

#[test]
fn roundtrip_single_byte() {
    let _profile = profile_test("roundtrip_single_byte");
    roundtrip(b"X", 3);
}

#[test]
fn roundtrip_two_bytes() {
    let _profile = profile_test("roundtrip_two_bytes");
    roundtrip(b"AB", 3);
}

// ── Text-like data ───────────────────────────────────────────────────────────

#[test]
fn roundtrip_ascii_sentence() {
    let _profile = profile_test("roundtrip_ascii_sentence");
    roundtrip(b"The quick brown fox jumps over the lazy dog.", 3);
}

#[test]
fn roundtrip_lorem_ipsum() {
    let _profile = profile_test("roundtrip_lorem_ipsum");
    let text = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
                 Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
                 Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris.";
    roundtrip(text, 3);
}

#[test]
fn roundtrip_repeated_text() {
    let _profile = profile_test("roundtrip_repeated_text");
    let text = b"hello world! ".repeat(500);
    roundtrip(&text, 3);
}

#[test]
fn roundtrip_benchmark_repetitive_16k() {
    let _profile = profile_test("roundtrip_benchmark_repetitive_16k");
    let data = benchmark_repetitive_corpus(16 * 1024);
    roundtrip(&data, 3);
}

// ── Binary patterns ──────────────────────────────────────────────────────────

#[test]
fn roundtrip_all_zeros() {
    let _profile = profile_test("roundtrip_all_zeros");
    let data = vec![0u8; 10_000];
    roundtrip(&data, 3);
}

#[test]
fn roundtrip_all_same_byte() {
    let _profile = profile_test("roundtrip_all_same_byte");
    let data = vec![0xABu8; 8_192];
    roundtrip(&data, 3);
}

#[test]
fn roundtrip_sequential_bytes() {
    let _profile = profile_test("roundtrip_sequential_bytes");
    let data: Vec<u8> = (0u8..=255).cycle().take(5_000).collect();
    roundtrip(&data, 3);
}

#[test]
fn roundtrip_pseudo_random() {
    let _profile = profile_test("roundtrip_pseudo_random");
    // Simple LCG pseudo-random to get repeatable "random" bytes
    let mut state: u64 = 0xDEAD_BEEF_1234_5678;
    let data: Vec<u8> = (0..50_000)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect();
    roundtrip(&data, 1);
}

// ── Large blocks ─────────────────────────────────────────────────────────────

#[test]
fn roundtrip_exactly_one_block() {
    let _profile = profile_test("roundtrip_exactly_one_block");
    // 128 KiB = MAX_BLOCK_SIZE
    let data: Vec<u8> = (0..131_072).map(|i| (i * 3) as u8).collect();
    roundtrip(&data, 1);
}

#[test]
fn roundtrip_multi_block() {
    let _profile = profile_test("roundtrip_multi_block");
    let data: Vec<u8> = (0..400_000).map(|i| (i as u8).wrapping_mul(7)).collect();
    roundtrip(&data, 1);
}

// ── Compression levels ───────────────────────────────────────────────────────

#[test]
fn roundtrip_all_fast_levels() {
    let _profile = profile_test("roundtrip_all_fast_levels");
    let data = b"zstd level testing data. ".repeat(200);
    for level in 1..=3 {
        roundtrip(&data, level);
    }
}

#[test]
fn roundtrip_mid_levels() {
    let _profile = profile_test("roundtrip_mid_levels");
    let data = b"zstd level testing data. ".repeat(200);
    for level in [4, 6, 9, 12] {
        roundtrip(&data, level);
    }
}

#[test]
fn roundtrip_high_levels() {
    let _profile = profile_test("roundtrip_high_levels");
    let data = b"high compression ratio test. ".repeat(100);
    for level in [15, 19, 22] {
        roundtrip(&data, level);
    }
}

#[test]
fn roundtrip_silesia_dickens_regression_levels() {
    let _profile = profile_test("roundtrip_silesia_dickens_regression_levels");
    let data = silesia_dickens();
    for level in [1, 3, 9, 19] {
        roundtrip(data, level);
    }
}

// ── Error handling ───────────────────────────────────────────────────────────

#[test]
fn invalid_level_too_low() {
    let _profile = profile_test("invalid_level_too_low");
    assert!(matches!(compress(b"x", 0), Err(ZstdError::InvalidLevel(0))));
}

#[test]
fn invalid_level_too_high() {
    let _profile = profile_test("invalid_level_too_high");
    assert!(matches!(
        compress(b"x", 23),
        Err(ZstdError::InvalidLevel(23))
    ));
}

#[test]
fn decompress_wrong_magic() {
    let _profile = profile_test("decompress_wrong_magic");
    let result = decompress(b"\x00\x00\x00\x00hello world");
    assert!(matches!(result, Err(ZstdError::InvalidMagic(_))));
}

#[test]
fn decompress_truncated_frame() {
    let _profile = profile_test("decompress_truncated_frame");
    let compressed = compress(b"hello world", 3).unwrap();
    // Truncate halfway through
    let truncated = &compressed[..compressed.len() / 2];
    assert!(decompress(truncated).is_err());
}

#[test]
fn decompress_corrupt_checksum() {
    let _profile = profile_test("decompress_corrupt_checksum");
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
    let _profile = profile_test("compress_bound_always_sufficient");
    for size in [0, 1, 100, 10_000, 100_000] {
        let data: Vec<u8> = (0..size).map(|i| i as u8).collect();
        let compressed = compress(&data, 1).unwrap();
        let bound = compress_bound(size);
        assert!(
            compressed.len() <= bound,
            "compress_bound({size}) = {bound} < actual {}",
            compressed.len()
        );
    }
}

// ── Lazy matching ─────────────────────────────────────────────────────────────

#[test]
fn roundtrip_lazy_levels() {
    let _profile = profile_test("roundtrip_lazy_levels");
    let data = benchmark_repetitive_corpus(64 * 1024);
    // All lazy and lazy2 levels: 6-12
    for level in 6..=12 {
        roundtrip(&data, level);
    }
}

#[test]
fn lazy2_ratio_better_than_greedy() {
    let _profile = profile_test("lazy2_ratio_better_than_greedy");
    // On compressible text, lazy2 (level 8) should compress better than greedy (level 5).
    let data = benchmark_repetitive_corpus(256 * 1024);
    let greedy_size = compress(&data, 5).unwrap().len();
    let lazy2_size = compress(&data, 8).unwrap().len();
    assert!(
        lazy2_size < greedy_size,
        "level 8 (lazy2) size {lazy2_size} should be < level 5 (greedy) size {greedy_size}"
    );
}

// ── Compression ratio sanity ─────────────────────────────────────────────────

#[test]
fn highly_compressible_data_shrinks() {
    let _profile = profile_test("highly_compressible_data_shrinks");
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
    let _profile = profile_test("already_random_data_does_not_expand_much");
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
