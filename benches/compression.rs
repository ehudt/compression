//! Benchmarks for zstd_rs compression and decompression.
//!
//! Run with: `cargo bench`
//! Full sweep: `ZSTD_RS_FULL_BENCHES=1 cargo bench`
//!
//! # What is measured
//!
//! ## Speed (throughput)
//! Criterion measures wall-clock time and reports throughput in MiB/s.
//! Groups:
//!   - `compress/level_sweep`   – representative levels by default, all levels 1..22 in full mode
//!   - `compress/size_scaling`  – level 3, input sizes 1 KiB–256 KiB
//!   - `decompress`             – decompression throughput for the same corpus/level cases
//!   - `roundtrip`              – compress + decompress end-to-end
//!
//! ## Size (compression ratio)
//! A non-timing pass (`ratio_table`) runs once per `cargo bench` invocation and
//! prints a human-readable table to stderr showing, for each timed case:
//!   - corpus
//!   - level
//!   - compressed size in bytes
//!   - ratio as a percentage of the original
//! This makes the speed/ratio tradeoff immediately visible in the same run.

use std::env;
#[cfg(feature = "profiling")]
use std::path::Path;

#[cfg(feature = "profiling")]
use criterion::profiler::Profiler;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use zstd_rs::{compress, decompress};

#[cfg(feature = "profiling")]
const BENCHES_ENV_VAR: &str = "ZSTD_RS_PROFILE_BENCHES";
const FULL_BENCHES_ENV_VAR: &str = "ZSTD_RS_FULL_BENCHES";

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

#[cfg(feature = "profiling")]
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

fn compression_levels() -> impl Iterator<Item = i32> {
    1..=22
}

fn default_levels() -> &'static [i32] {
    &[1, 3, 9, 19, 22]
}

fn full_benchmarks_enabled() -> bool {
    match env::var(FULL_BENCHES_ENV_VAR) {
        Ok(value) => {
            let value = value.trim();
            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
        }
        Err(env::VarError::NotPresent) => false,
        Err(err) => {
            eprintln!("failed to read {FULL_BENCHES_ENV_VAR}: {err}");
            false
        }
    }
}

fn active_levels() -> Vec<i32> {
    if full_benchmarks_enabled() {
        compression_levels().collect()
    } else {
        default_levels().to_vec()
    }
}

struct BenchCase {
    corpus_name: &'static str,
    level: i32,
    input: Vec<u8>,
    compressed: Vec<u8>,
}

impl BenchCase {
    fn benchmark_id(&self) -> BenchmarkId {
        BenchmarkId::new(self.corpus_name, self.level)
    }

    fn input_len(&self) -> usize {
        self.input.len()
    }

    fn compressed_len(&self) -> usize {
        self.compressed.len()
    }

    fn ratio_percent(&self) -> f64 {
        100.0 * self.compressed_len() as f64 / self.input_len() as f64
    }
}

fn benchmark_cases(size: usize, levels: &[i32]) -> Vec<BenchCase> {
    let data = corpora(size);
    let mut cases = Vec::with_capacity(data.len() * levels.len());

    for (corpus_name, bytes) in data {
        for &level in levels {
            let compressed = compress(&bytes, level).expect("compress failed");
            cases.push(BenchCase {
                corpus_name,
                level,
                input: bytes.clone(),
                compressed,
            });
        }
    }

    cases
}

// ── Ratio table (size side of the tradeoff) ──────────────────────────────────

/// Prints a compression-ratio table to stderr and registers a single trivial
/// Criterion benchmark so this shows up in the HTML report index.
///
/// The table looks like:
/// ```text
/// ┌──────────────────────────────────────────────────────────────────────────┐
/// │ Compression ratio summary (64 KiB input, smaller % is better)           │
/// ├──────────────────┬───────┬─────────────┬───────────┬────────────────────┤
/// │ corpus           │ level │ input bytes │ compressed│ ratio              │
/// ├──────────────────┼───────┼─────────────┼───────────┼────────────────────┤
/// │ repetitive       │ 3     │       65536 │      3146 │   4.8%             │
/// └──────────────────┴───────┴─────────────┴───────────┴────────────────────┘
/// ```
fn ratio_table(c: &mut Criterion) {
    const SIZE: usize = 64 * 1024;
    let levels = active_levels();
    let cases = benchmark_cases(SIZE, &levels);
    let mode_label = if full_benchmarks_enabled() {
        "full"
    } else {
        "default"
    };

    // Build and print the table to stderr (visible during `cargo bench`).
    eprintln!();
    eprintln!("┌──────────────────────────────────────────────────────────────────────────────┐");
    eprintln!(
        "│ Compression ratio summary ({} KiB input, {} mode)                         │",
        SIZE / 1024,
        mode_label
    );
    eprintln!("├──────────────────────┬───────┬─────────────┬────────────┬──────────┤");
    eprintln!(
        "│ {:<20} │ {:>5} │ {:>11} │ {:>10} │ {:>8} │",
        "corpus", "level", "input bytes", "compressed", "ratio"
    );
    eprintln!("├──────────────────────┼───────┼─────────────┼────────────┼──────────┤");

    for case in &cases {
        eprintln!(
            "│ {:<20} │ {:>5} │ {:>11} │ {:>10} │ {:>7.1}% │",
            case.corpus_name,
            case.level,
            case.input_len(),
            case.compressed_len(),
            case.ratio_percent()
        );
    }
    eprintln!("└──────────────────────┴───────┴─────────────┴────────────┴──────────┘");
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
    let levels = active_levels();
    let cases = benchmark_cases(SIZE, &levels);
    let mut group = c.benchmark_group("compress/level_sweep");
    group.throughput(Throughput::Bytes(SIZE as u64));

    for case in &cases {
        group.bench_with_input(case.benchmark_id(), &case.input, |b, input| {
            b.iter(|| compress(black_box(input), case.level).unwrap())
        });
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
    const SIZE: usize = 64 * 1024;
    let levels = active_levels();
    let cases = benchmark_cases(SIZE, &levels);
    let mut group = c.benchmark_group("decompress");

    for case in &cases {
        group.throughput(Throughput::Bytes(case.input_len() as u64));
        group.bench_with_input(case.benchmark_id(), &case.compressed, |b, compressed| {
            b.iter(|| decompress(black_box(compressed)).unwrap())
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
