//! Benchmarks for zstd_rs compression and decompression.
//!
//! Run with: `cargo bench`
//! Full sweep: `ZSTD_RS_FULL_BENCHES=1 cargo bench`
//!
//! Default mode is intentionally small and fast. It measures only a few
//! representative cases so a developer can quickly tell whether a change
//! clearly improved, regressed, or did nothing to performance.
//!
//! Full mode preserves the exhaustive corpus/level sweep for deliberate runs.

use std::env;
#[cfg(feature = "profiling")]
use std::path::Path;
use std::time::Duration;

#[cfg(feature = "profiling")]
use criterion::profiler::Profiler;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use zstd_rs::{compress, decompress};

#[cfg(feature = "profiling")]
const BENCHES_ENV_VAR: &str = "ZSTD_RS_PROFILE_BENCHES";
const FULL_BENCHES_ENV_VAR: &str = "ZSTD_RS_FULL_BENCHES";

fn criterion_config() -> Criterion {
    let mut criterion = Criterion::default();

    if !full_benchmarks_enabled() {
        criterion = criterion
            .sample_size(10)
            .warm_up_time(Duration::from_millis(250))
            .measurement_time(Duration::from_secs(1));
    }

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

fn full_levels() -> Vec<i32> {
    compression_levels().collect()
}

fn fast_levels() -> Vec<i32> {
    vec![1, 3, 9, 19, 22]
}

struct BenchCase {
    corpus_name: &'static str,
    level: i32,
    input: Vec<u8>,
    compressed: Vec<u8>,
}

impl BenchCase {
    fn compression_id(&self) -> BenchmarkId {
        BenchmarkId::new(self.corpus_name, self.level)
    }

    fn decompression_id(&self) -> BenchmarkId {
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

fn fast_cases() -> Vec<BenchCase> {
    const SIZE: usize = 64 * 1024;

    benchmark_cases(SIZE, &fast_levels())
        .into_iter()
        .filter(|case| {
            matches!(
                (case.corpus_name, case.level),
                ("repetitive", 3) | ("binary_structured", 3) | ("random", 1) | ("all_zeros", 3)
            )
        })
        .collect()
}

fn print_ratio_table(cases: &[BenchCase], mode_label: &str, size: usize) {
    eprintln!();
    eprintln!("┌──────────────────────────────────────────────────────────────────────────────┐");
    eprintln!(
        "│ Compression ratio summary ({} KiB input, {} mode)                         │",
        size / 1024,
        mode_label
    );
    eprintln!("├──────────────────────┬───────┬─────────────┬────────────┬──────────┤");
    eprintln!(
        "│ {:<20} │ {:>5} │ {:>11} │ {:>10} │ {:>8} │",
        "corpus", "level", "input bytes", "compressed", "ratio"
    );
    eprintln!("├──────────────────────┼───────┼─────────────┼────────────┼──────────┤");

    for case in cases {
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
}

fn bench_fast(c: &mut Criterion) {
    const SIZE: usize = 64 * 1024;
    let cases = fast_cases();
    print_ratio_table(&cases, "fast", SIZE);

    let mut compress_group = c.benchmark_group("fast/compress");
    compress_group.throughput(Throughput::Bytes(SIZE as u64));
    for case in &cases {
        compress_group.bench_with_input(case.compression_id(), &case.input, |b, input| {
            b.iter(|| compress(black_box(input), case.level).unwrap())
        });
    }
    compress_group.finish();

    let mut decompress_group = c.benchmark_group("fast/decompress");
    for case in &cases {
        decompress_group.throughput(Throughput::Bytes(case.input_len() as u64));
        decompress_group.bench_with_input(
            case.decompression_id(),
            &case.compressed,
            |b, compressed| b.iter(|| decompress(black_box(compressed)).unwrap()),
        );
    }
    decompress_group.finish();

    let repetitive = cases
        .iter()
        .find(|case| case.corpus_name == "repetitive" && case.level == 3)
        .expect("missing repetitive fast case");
    let random = cases
        .iter()
        .find(|case| case.corpus_name == "random" && case.level == 1)
        .expect("missing random fast case");

    let mut roundtrip_group = c.benchmark_group("fast/roundtrip");
    roundtrip_group.throughput(Throughput::Bytes(SIZE as u64));
    roundtrip_group.bench_function("repetitive_level3", |b| {
        b.iter(|| {
            let compressed = compress(black_box(&repetitive.input), repetitive.level).unwrap();
            decompress(black_box(&compressed)).unwrap()
        });
    });
    roundtrip_group.bench_function("random_level1", |b| {
        b.iter(|| {
            let compressed = compress(black_box(&random.input), random.level).unwrap();
            decompress(black_box(&compressed)).unwrap()
        });
    });
    roundtrip_group.finish();
}

fn bench_full(c: &mut Criterion) {
    const SIZE: usize = 64 * 1024;
    let levels = full_levels();
    let cases = benchmark_cases(SIZE, &levels);
    print_ratio_table(&cases, "full", SIZE);

    let mut compress_group = c.benchmark_group("compress/level_sweep");
    compress_group.throughput(Throughput::Bytes(SIZE as u64));
    for case in &cases {
        compress_group.bench_with_input(case.compression_id(), &case.input, |b, input| {
            b.iter(|| compress(black_box(input), case.level).unwrap())
        });
    }
    compress_group.finish();

    let mut size_scaling_group = c.benchmark_group("compress/size_scaling");
    for size in [1024usize, 4 * 1024, 16 * 1024, 64 * 1024, 256 * 1024] {
        let repetitive = corpus_repetitive(size);
        size_scaling_group.throughput(Throughput::Bytes(size as u64));
        size_scaling_group.bench_with_input(
            BenchmarkId::new("repetitive", size),
            &repetitive,
            |b, input| b.iter(|| compress(black_box(input), 3).unwrap()),
        );

        let random = corpus_random(size);
        size_scaling_group.bench_with_input(
            BenchmarkId::new("random", size),
            &random,
            |b, input| b.iter(|| compress(black_box(input), 3).unwrap()),
        );

        let binary = corpus_binary_structured(size);
        size_scaling_group.bench_with_input(
            BenchmarkId::new("binary_structured", size),
            &binary,
            |b, input| b.iter(|| compress(black_box(input), 3).unwrap()),
        );
    }
    size_scaling_group.finish();

    let mut decompress_group = c.benchmark_group("decompress");
    for case in &cases {
        decompress_group.throughput(Throughput::Bytes(case.input_len() as u64));
        decompress_group.bench_with_input(
            case.decompression_id(),
            &case.compressed,
            |b, compressed| b.iter(|| decompress(black_box(compressed)).unwrap()),
        );
    }
    decompress_group.finish();

    let repetitive = corpus_repetitive(SIZE);
    let random = corpus_random(SIZE);
    let mut roundtrip_group = c.benchmark_group("roundtrip");
    roundtrip_group.throughput(Throughput::Bytes(SIZE as u64));
    roundtrip_group.bench_function("repetitive_level3", |b| {
        b.iter(|| {
            let compressed = compress(black_box(&repetitive), 3).unwrap();
            decompress(black_box(&compressed)).unwrap()
        });
    });
    roundtrip_group.bench_function("random_level1", |b| {
        b.iter(|| {
            let compressed = compress(black_box(&random), 1).unwrap();
            decompress(black_box(&compressed)).unwrap()
        });
    });
    roundtrip_group.finish();
}

fn benchmark_suite(c: &mut Criterion) {
    if full_benchmarks_enabled() {
        bench_full(c);
    } else {
        bench_fast(c);
    }
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = benchmark_suite
}
criterion_main!(benches);
