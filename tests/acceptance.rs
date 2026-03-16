//! Acceptance tests: interoperability with the standard `zstd` tool.
//!
//! Two directions are tested:
//!
//! 1. **Our compress → `zstd -d`**: we produce a byte stream that the reference
//!    implementation can decompress back to the original data.
//! 2. **`zstd -c` → our decompress**: we can correctly decode data that the
//!    reference implementation compressed.
//!
//! Both directions must agree byte-for-byte with the original input.
//!
//! The tests require `zstd` (≥ 1.4) to be present in PATH.  If the binary
//! cannot be found the entire module is skipped with an explanatory message
//! rather than failing, so the test suite remains usable in environments
//! without the tool.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use zstd_rs::{compress, decompress};

// ── Utilities ────────────────────────────────────────────────────────────────

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a unique path in the system temp directory with the given suffix.
fn tmp_path(suffix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "zstd_rs_accept_{}_{}{}",
        std::process::id(),
        id,
        suffix
    ))
}

/// RAII guard that deletes a file when dropped, even on panic.
struct TempFile(PathBuf);

impl TempFile {
    fn new(suffix: &str) -> Self {
        TempFile(tmp_path(suffix))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Check whether the `zstd` CLI is available.
///
/// Returns `None` when available, or `Some(skip_message)` when not found.
fn require_zstd() -> Option<&'static str> {
    let ok = Command::new("zstd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        None
    } else {
        Some("zstd binary not found in PATH — skipping acceptance tests")
    }
}

/// Compress `data` with our library at `level`, then decompress with `zstd -d`
/// and assert the result equals `data`.
fn check_our_compress_system_decompress(data: &[u8], level: i32) {
    if let Some(msg) = require_zstd() {
        eprintln!("SKIP: {msg}");
        return;
    }

    let compressed = compress(data, level)
        .unwrap_or_else(|e| panic!("our compress failed at level {level}: {e}"));

    let zst = TempFile::new(".zst");
    std::fs::write(zst.path(), &compressed).expect("failed to write temp compressed file");

    let out = Command::new("zstd")
        .args(["-d", "--stdout", "--no-progress"])
        .arg(zst.path())
        .output()
        .expect("failed to spawn zstd");

    assert!(
        out.status.success(),
        "zstd -d failed (exit {:?}) for level {level}, {} input bytes.\nstderr:\n{}",
        out.status.code(),
        data.len(),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(
        out.stdout,
        data,
        "our_compress → zstd_decompress: data mismatch at level {level} ({} bytes)",
        data.len(),
    );
}

/// Compress `data` with `zstd` at `level`, then decompress with our library
/// and assert the result equals `data`.
fn check_system_compress_our_decompress(data: &[u8], level: i32) {
    if let Some(msg) = require_zstd() {
        eprintln!("SKIP: {msg}");
        return;
    }

    let raw = TempFile::new(".bin");
    let zst = TempFile::new(".bin.zst");
    std::fs::write(raw.path(), data).expect("failed to write temp input file");

    // zstd levels 20-22 need --ultra
    let mut cmd = Command::new("zstd");
    cmd.args(["--no-progress", "-f", "-q"]);
    if level >= 20 {
        cmd.arg("--ultra");
    }
    cmd.arg(format!("-{level}"))
        .arg("-o")
        .arg(zst.path())
        .arg(raw.path());

    let status = cmd.status().expect("failed to spawn zstd");
    assert!(
        status.success(),
        "zstd compression failed at level {level} for {} bytes",
        data.len(),
    );

    let compressed =
        std::fs::read(zst.path()).expect("failed to read zst file after zstd compression");

    let decompressed = decompress(&compressed)
        .unwrap_or_else(|e| panic!("our decompress failed for zstd-level-{level} output: {e}"));

    assert_eq!(
        decompressed,
        data,
        "system_compress → our_decompress: data mismatch at level {level} ({} bytes)",
        data.len(),
    );
}

// ── Corpus helpers ────────────────────────────────────────────────────────────

fn repetitive_text(n: usize) -> Vec<u8> {
    b"the quick brown fox jumps over the lazy dog. ".repeat(n / 45 + 1)[..n].to_vec()
}

fn sequential_bytes(n: usize) -> Vec<u8> {
    (0u8..=255).cycle().take(n).collect()
}

fn pseudo_random_bytes(n: usize) -> Vec<u8> {
    let mut state: u64 = 0xDEAD_BEEF_1234_5678;
    (0..n)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Direction 1: our compress → system zstd decompress
// ═══════════════════════════════════════════════════════════════════════════════

/// Empty input compresses to a valid zstd frame that `zstd -d` accepts.
#[test]
fn ours_to_zstd_empty() {
    check_our_compress_system_decompress(b"", 3);
}

/// Single-byte frames are handled correctly.
#[test]
fn ours_to_zstd_single_byte() {
    check_our_compress_system_decompress(b"Z", 3);
}

/// All-zeros run → RLE / short compressed block.
#[test]
fn ours_to_zstd_all_zeros() {
    check_our_compress_system_decompress(&vec![0u8; 10_000], 3);
}

/// Highly repetitive text — exercises Huffman + sequence paths.
#[test]
fn ours_to_zstd_repetitive_text() {
    check_our_compress_system_decompress(&repetitive_text(32_768), 3);
}

/// Sequential byte pattern — moderate compressibility.
#[test]
fn ours_to_zstd_sequential_bytes() {
    check_our_compress_system_decompress(&sequential_bytes(8_192), 1);
}

/// Pseudo-random data — should fall back to a raw block without expanding much.
#[test]
fn ours_to_zstd_pseudo_random() {
    check_our_compress_system_decompress(&pseudo_random_bytes(16_384), 1);
}

/// Data that crosses the 128 KiB block boundary → multi-block frame.
#[test]
fn ours_to_zstd_multi_block() {
    let data: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(7)) as u8).collect();
    check_our_compress_system_decompress(&data, 1);
}

/// Sweep levels 1–9: every level must produce valid output.
#[test]
fn ours_to_zstd_level_sweep() {
    let data = repetitive_text(4_096);
    for level in 1..=9 {
        check_our_compress_system_decompress(&data, level);
    }
}

/// High levels (10–19) — same correctness requirement.
#[test]
fn ours_to_zstd_high_levels() {
    let data = repetitive_text(4_096);
    for level in [10, 13, 16, 19] {
        check_our_compress_system_decompress(&data, level);
    }
}

/// Binary blob with all 256 byte values present.
#[test]
fn ours_to_zstd_all_byte_values() {
    let data: Vec<u8> = (0u8..=255).collect::<Vec<_>>().repeat(64);
    check_our_compress_system_decompress(&data, 3);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Direction 2: system zstd compress → our decompress
// ═══════════════════════════════════════════════════════════════════════════════

/// Empty input compressed by the reference tool.
#[test]
fn zstd_to_ours_empty() {
    check_system_compress_our_decompress(b"", 3);
}

/// Single byte compressed by the reference tool.
#[test]
fn zstd_to_ours_single_byte() {
    check_system_compress_our_decompress(b"Z", 3);
}

/// All-zeros compressed by the reference tool (typically RLE block).
#[test]
fn zstd_to_ours_all_zeros() {
    check_system_compress_our_decompress(&vec![0u8; 10_000], 3);
}

/// Repetitive text compressed by reference at level 3.
#[test]
fn zstd_to_ours_repetitive_level3() {
    check_system_compress_our_decompress(&repetitive_text(32_768), 3);
}

/// Reference compressor at level 1 — fast mode.
#[test]
fn zstd_to_ours_repetitive_level1() {
    check_system_compress_our_decompress(&repetitive_text(32_768), 1);
}

/// Reference compressor at level 9 — higher ratio tables.
#[test]
fn zstd_to_ours_repetitive_level9() {
    check_system_compress_our_decompress(&repetitive_text(32_768), 9);
}

/// Reference compressor at level 19 — near-maximum ratio.
#[test]
fn zstd_to_ours_repetitive_level19() {
    check_system_compress_our_decompress(&repetitive_text(32_768), 19);
}

/// Multi-block data from the reference tool.
#[test]
fn zstd_to_ours_multi_block() {
    let data: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(7)) as u8).collect();
    check_system_compress_our_decompress(&data, 1);
}

/// Pseudo-random data (nearly incompressible) from the reference tool.
#[test]
fn zstd_to_ours_pseudo_random() {
    check_system_compress_our_decompress(&pseudo_random_bytes(16_384), 1);
}

/// Binary blob with all 256 byte values.
#[test]
fn zstd_to_ours_all_byte_values() {
    let data: Vec<u8> = (0u8..=255).collect::<Vec<_>>().repeat(64);
    check_system_compress_our_decompress(&data, 3);
}
