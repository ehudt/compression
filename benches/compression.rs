//! Benchmarks for zstd_rs compression and decompression.
//!
//! Run with: `cargo bench`
//!
//! # What is measured
//!
//! ## Speed (throughput)
//! Criterion measures wall-clock time and reports throughput in MiB/s.
//! Groups:
//!   - `compress/level_sweep`   – all levels 1..22 on three corpus types (64 KiB)
//!   - `compress/size_scaling`  – level 3, input sizes 1 KiB–256 KiB
//!   - `decompress`             – decompression throughput for repetitive and random data
//!   - `roundtrip`              – compress + decompress end-to-end
//!
//! ## Size (compression ratio)
//! A non-timing pass (`ratio_table`) runs once per `cargo bench` invocation and
//! prints a human-readable table to stderr showing:
//!   - compressed size in bytes
//!   - ratio as a percentage of the original
//! across all corpus types and levels.  This makes the speed/ratio tradeoff
//! immediately visible in the same run.

use std::env;
#[cfg(feature = "profiling")]
use std::path::Path;

use criterion::profiler::Profiler;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
#[cfg(feature = "profiling")]
use zstd_rs::{compress, decompress};

const BENCHES_ENV_VAR: &str = "ZSTD_RS_PROFILE_BENCHES";

fn criterion_config() -> Criterion {
    let criterion = Criterion::default();

    #[cfg(feature = "profiling")]
    {
        if profiling_enabled() {
            return criterion.with_profiler(BenchProfiler::new(100));
        }
    }

    criterion
}

fn profiling_enabled() -> bool {
    match env::var(BENCHES_ENV_VAR) {
        Ok(value) => {
            let value = value.trim();
            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
        }
        Err(env::VarError::NotPresent) => false,
        Err(err) => {
            eprintln!("failed to read {BENCHES_ENV_VAR}: {err}");
            false
        }
    }
}

#[cfg(feature = "profiling")]
struct BenchProfiler {
    frequency: i32,
    active_profiler: Option<pprof::ProfilerGuard<'static>>,
}

#[cfg(feature = "profiling")]
impl BenchProfiler {
    fn new(frequency: i32) -> Self {
        Self {
            frequency,
            active_profiler: None,
        }
    }
}

#[cfg(feature = "profiling")]
impl Profiler for BenchProfiler {
    fn start_profiling(&mut self, _benchmark_id: &str, _benchmark_dir: &Path) {
        self.active_profiler = Some(
            pprof::ProfilerGuard::new(self.frequency).expect("failed to start benchmark profiler"),
        );
    }

    fn stop_profiling(&mut self, _benchmark_id: &str, benchmark_dir: &Path) {
        std::fs::create_dir_all(benchmark_dir).expect("failed to create benchmark profile dir");

        if let Some(profiler) = self.active_profiler.take() {
            let report = profiler
                .report()
                .build()
                .expect("failed to build benchmark profile report");
            zstd_rs::profiling::write_report_outputs(
                &report,
                &benchmark_dir.join("flamegraph.svg"),
            )
            .expect("failed to write benchmark profile artifacts");
        }
    }
}

// ── Corpus generators ────────────────────────────────────────────────────────

fn corpus_repetitive(size: usize) -> Vec<u8> {
    b"the quick brown fox jumps over the lazy dog. ".repeat(size / 45 + 1)[..size].to_vec()
}

fn corpus_random(size: usize) -> Vec<u8> {
    let mut state: u64 = 0xCAFE_BABE_1337_C0DE;
    (0..size)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect()
}

fn corpus_binary_structured(size: usize) -> Vec<u8> {
    // Simulates something like a WASM or ELF binary: mixed structured data
    (0..size)
        .map(|i| match i % 16 {
            0..=3 => 0x00u8,
            4 => 0xFFu8,
            5..=7 => (i as u8).wrapping_mul(3),
            8..=11 => (i >> 2) as u8,
            _ => i as u8,
        })
        .collect()
}

fn corpus_all_zeros(size: usize) -> Vec<u8> {
    vec![0u8; size]
}

fn corpora(size: usize) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("all_zeros", corpus_all_zeros(size)),
        ("repetitive", corpus_repetitive(size)),
        ("binary_structured", corpus_binary_structured(size)),
        ("random", corpus_random(size)),
    ]
}

// ── Ratio table (size side of the tradeoff) ──────────────────────────────────

/// Prints a compression-ratio table to stderr and registers a single trivial
/// Criterion benchmark so this shows up in the HTML report index.
///
/// The table looks like:
/// ```text
/// ┌─────────────────────────────────────────────────────────────────────────────┐
/// │ Compression ratio  (64 KiB input, smaller % is better)                     │
/// ├──────────────────────┬───────────────────────────────────────────────────── │
/// │ corpus               │  L1      L3      L6      L9     L12     L19     L22  │
/// ├──────────────────────┼───────────────────────────────────────────────────── │
/// │ all_zeros            │  0.1%    0.1%    0.1%    0.1%   0.1%    0.1%   0.1% │
/// │ repetitive           │  5.2%    4.8%    4.7%    4.7%   4.7%    4.7%   4.7% │
/// │ binary_structured    │ 42.3%   41.0%   40.8%   40.8%  40.8%   40.8%  40.8%│
/// │ random               │ 99.8%   99.8%   99.8%   99.8%  99.8%   99.8%  99.8%│
/// └──────────────────────┴───────────────────────────────────────────────────── ┘
/// ```
fn ratio_table(c: &mut Criterion) {
    const SIZE: usize = 64 * 1024;
    const LEVELS: &[i32] = &[1, 3, 6, 9, 12, 19, 22];

    let data = corpora(SIZE);

    // Build and print the table to stderr (visible during `cargo bench`).
    eprintln!();
    eprintln!("┌─────────────────────────────────────────────────────────────────────────────┐");
    eprintln!(
        "│ Compression ratio  ({} KiB input, smaller % → better)                    │",
        SIZE / 1024
    );
    eprintln!("├──────────────────────┬──────────────────────────────────────────────────────┤");
    eprint!("│ {:<20} │", "corpus");
    for &lvl in LEVELS {
        eprint!(" L{:<5}", lvl);
    }
    eprintln!(" │");
    eprintln!("├──────────────────────┼──────────────────────────────────────────────────────┤");

    for (name, bytes) in &data {
        eprint!("│ {:<20} │", name);
        for &lvl in LEVELS {
            let compressed = compress(bytes, lvl).expect("compress failed");
            let ratio = 100.0 * compressed.len() as f64 / SIZE as f64;
            eprint!(" {:>5.1}%", ratio);
        }
        eprintln!(" │");
    }
    eprintln!("└──────────────────────┴──────────────────────────────────────────────────────┘");
    eprintln!();

    // Print a second table: absolute compressed sizes in bytes.
    eprintln!("┌─────────────────────────────────────────────────────────────────────────────┐");
    eprintln!(
        "│ Compressed size in bytes  ({} KiB input)                                  │",
        SIZE / 1024
    );
    eprintln!("├──────────────────────┬──────────────────────────────────────────────────────┤");
    eprint!("│ {:<20} │", "corpus");
    for &lvl in LEVELS {
        eprint!(" L{:<7}", lvl);
    }
    eprintln!("│");
    eprintln!("├──────────────────────┼──────────────────────────────────────────────────────┤");

    for (name, bytes) in &data {
        eprint!("│ {:<20} │", name);
        for &lvl in LEVELS {
            let compressed = compress(bytes, lvl).expect("compress failed");
            eprint!(" {:>7}", compressed.len());
        }
        eprintln!(" │");
    }
    eprintln!("└──────────────────────┴──────────────────────────────────────────────────────┘");
    eprintln!();

    // Register a trivial timing entry so this shows up in the Criterion report.
    c.bench_function("ratio_table/64KiB_all_levels", |b| {
        b.iter(|| black_box(0u8))
    });
}

// ── Level sweep: speed × ratio across all levels ─────────────────────────────

/// Measures compression **throughput** for every level on every corpus type.
/// Combined with the `ratio_table` output above this gives the full
/// speed/ratio picture: how much faster is level 1 vs level 22, and how much
/// worse is the ratio?
fn bench_level_sweep(c: &mut Criterion) {
    const SIZE: usize = 64 * 1024;
    const LEVELS: &[i32] = &[1, 3, 6, 9, 12, 19, 22];

    let data = corpora(SIZE);
    let mut group = c.benchmark_group("compress/level_sweep");
    group.throughput(Throughput::Bytes(SIZE as u64));

    for (corpus_name, bytes) in &data {
        for &lvl in LEVELS {
            group.bench_with_input(BenchmarkId::new(*corpus_name, lvl), bytes, |b, d| {
                b.iter(|| compress(black_box(d), lvl).unwrap())
            });
        }
    }
    group.finish();
}

// ── Size scaling: how does throughput scale with input size? ─────────────────

fn bench_compress_size_scaling(c: &mut Criterion) {
    const LEVEL: i32 = 3;
    let mut group = c.benchmark_group("compress/size_scaling");

    for size in [1024usize, 4 * 1024, 16 * 1024, 64 * 1024, 256 * 1024] {
        let rep = corpus_repetitive(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("repetitive", size), &rep, |b, d| {
            b.iter(|| compress(black_box(d), LEVEL).unwrap())
        });

        let rnd = corpus_random(size);
        group.bench_with_input(BenchmarkId::new("random", size), &rnd, |b, d| {
            b.iter(|| compress(black_box(d), LEVEL).unwrap())
        });

        let bin = corpus_binary_structured(size);
        group.bench_with_input(BenchmarkId::new("binary_structured", size), &bin, |b, d| {
            b.iter(|| compress(black_box(d), LEVEL).unwrap())
        });
    }
    group.finish();
}

// ── Decompression benchmarks ─────────────────────────────────────────────────

fn bench_decompress(c: &mut Criterion) {
    let mut group = c.benchmark_group("decompress");

    for size in [16 * 1024usize, 64 * 1024, 256 * 1024] {
        let rep = corpus_repetitive(size);
        let compressed_rep = compress(&rep, 3).unwrap();
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("repetitive", size),
            &compressed_rep,
            |b, c| b.iter(|| decompress(black_box(c)).unwrap()),
        );

        let rnd = corpus_random(size);
        let compressed_rnd = compress(&rnd, 1).unwrap();
        group.bench_with_input(BenchmarkId::new("random", size), &compressed_rnd, |b, c| {
            b.iter(|| decompress(black_box(c)).unwrap())
        });
    }
    group.finish();
}

// ── End-to-end round-trip ────────────────────────────────────────────────────

fn bench_roundtrip(c: &mut Criterion) {
    const SIZE: usize = 64 * 1024;
    let mut group = c.benchmark_group("roundtrip");
    group.throughput(Throughput::Bytes(SIZE as u64));

    let rep = corpus_repetitive(SIZE);
    group.bench_function("repetitive_level3", |b| {
        b.iter(|| {
            let compressed = compress(black_box(&rep), 3).unwrap();
            decompress(black_box(&compressed)).unwrap()
        });
    });

    let rnd = corpus_random(SIZE);
    group.bench_function("random_level1", |b| {
        b.iter(|| {
            let compressed = compress(black_box(&rnd), 1).unwrap();
            decompress(black_box(&compressed)).unwrap()
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets =
        ratio_table,
        bench_level_sweep,
        bench_compress_size_scaling,
        bench_decompress,
        bench_roundtrip
}
criterion_main!(benches);
