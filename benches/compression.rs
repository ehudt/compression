//! Benchmarks for zstd_rs compression and decompression.
//!
//! Run with: `cargo bench`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use zstd_rs::{compress, decompress};

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
            8..=11 => ((i >> 2) as u8),
            _ => (i as u8),
        })
        .collect()
}

// ── Compression benchmarks ───────────────────────────────────────────────────

fn bench_compress_levels(c: &mut Criterion) {
    let data = corpus_repetitive(64 * 1024); // 64 KiB
    let mut group = c.benchmark_group("compress/64KiB_repetitive");
    group.throughput(Throughput::Bytes(data.len() as u64));

    for level in [1, 3, 6, 9] {
        group.bench_with_input(
            BenchmarkId::new("level", level),
            &level,
            |b, &lvl| {
                b.iter(|| compress(black_box(&data), lvl).unwrap());
            },
        );
    }
    group.finish();
}

fn bench_compress_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("compress/level3");

    for size in [1024usize, 16 * 1024, 64 * 1024, 256 * 1024] {
        let data = corpus_repetitive(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("repetitive", size),
            &data,
            |b, d| {
                b.iter(|| compress(black_box(d), 3).unwrap());
            },
        );

        let data_rand = corpus_random(size);
        group.bench_with_input(
            BenchmarkId::new("random", size),
            &data_rand,
            |b, d| {
                b.iter(|| compress(black_box(d), 3).unwrap());
            },
        );
    }
    group.finish();
}

fn bench_compress_corpus_types(c: &mut Criterion) {
    const SIZE: usize = 64 * 1024;
    let mut group = c.benchmark_group("compress/corpus_type");
    group.throughput(Throughput::Bytes(SIZE as u64));

    let rep = corpus_repetitive(SIZE);
    group.bench_function("repetitive", |b| {
        b.iter(|| compress(black_box(&rep), 3).unwrap())
    });

    let rnd = corpus_random(SIZE);
    group.bench_function("random", |b| {
        b.iter(|| compress(black_box(&rnd), 3).unwrap())
    });

    let bin = corpus_binary_structured(SIZE);
    group.bench_function("binary_structured", |b| {
        b.iter(|| compress(black_box(&bin), 3).unwrap())
    });

    group.finish();
}

// ── Decompression benchmarks ─────────────────────────────────────────────────

fn bench_decompress(c: &mut Criterion) {
    let mut group = c.benchmark_group("decompress");

    for size in [16 * 1024usize, 64 * 1024, 256 * 1024] {
        let data = corpus_repetitive(size);
        let compressed = compress(&data, 3).unwrap();
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("repetitive", size),
            &compressed,
            |b, c| {
                b.iter(|| decompress(black_box(c)).unwrap());
            },
        );

        let data_rand = corpus_random(size);
        let compressed_rand = compress(&data_rand, 1).unwrap();
        group.bench_with_input(
            BenchmarkId::new("random", size),
            &compressed_rand,
            |b, c| {
                b.iter(|| decompress(black_box(c)).unwrap());
            },
        );
    }
    group.finish();
}

// ── End-to-end round-trip ────────────────────────────────────────────────────

fn bench_roundtrip(c: &mut Criterion) {
    let data = corpus_repetitive(64 * 1024);
    c.benchmark_group("roundtrip")
        .throughput(Throughput::Bytes(data.len() as u64))
        .bench_function("64KiB_repetitive_level3", |b| {
            b.iter(|| {
                let compressed = compress(black_box(&data), 3).unwrap();
                decompress(black_box(&compressed)).unwrap()
            });
        });
}

criterion_group!(
    benches,
    bench_compress_levels,
    bench_compress_sizes,
    bench_compress_corpus_types,
    bench_decompress,
    bench_roundtrip,
);
criterion_main!(benches);
