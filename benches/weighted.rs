//! Weighted composite benchmark — one score per metric.
//!
//! Measures compression ratio, compression throughput, and decompression
//! throughput across 8 synthetic corpora, then computes a single weighted
//! average for each metric.  More common real-world data types receive
//! higher weight.
//!
//! Run with: `cargo bench --bench weighted`
//! Full sweep: `ZSTD_RS_FULL_BENCHES=1 cargo bench --bench weighted`

use std::env;
use std::hint::black_box;
use std::time::{Duration, Instant};

use zstd_rs::{compress, decompress};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const CORPUS_SIZE: usize = 64 * 1024;
const CORPUS_SIZE_FULL: usize = 256 * 1024;
const BENCH_LEVEL: i32 = 3;
const MIN_ITERS: u32 = 10;
const MIN_DURATION: Duration = Duration::from_millis(500);
const WARMUP_ITERS: u32 = 3;

const FULL_BENCHES_ENV_VAR: &str = "ZSTD_RS_FULL_BENCHES";

fn full_benchmarks_enabled() -> bool {
    match env::var(FULL_BENCHES_ENV_VAR) {
        Ok(value) => {
            let value = value.trim();
            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Weighted corpus definitions
// ---------------------------------------------------------------------------

struct WeightedCorpus {
    name: &'static str,
    weight: f64,
    generate: fn(usize) -> Vec<u8>,
}

const WEIGHTED_CORPORA: &[WeightedCorpus] = &[
    WeightedCorpus {
        name: "text",
        weight: 0.20,
        generate: corpus_text,
    },
    WeightedCorpus {
        name: "json",
        weight: 0.20,
        generate: corpus_json,
    },
    WeightedCorpus {
        name: "xml",
        weight: 0.15,
        generate: corpus_xml,
    },
    WeightedCorpus {
        name: "source_code",
        weight: 0.12,
        generate: corpus_source_code,
    },
    WeightedCorpus {
        name: "database",
        weight: 0.12,
        generate: corpus_database,
    },
    WeightedCorpus {
        name: "executable",
        weight: 0.08,
        generate: corpus_executable,
    },
    WeightedCorpus {
        name: "medical_image",
        weight: 0.08,
        generate: corpus_medical_image,
    },
    WeightedCorpus {
        name: "random",
        weight: 0.05,
        generate: corpus_random,
    },
];

// ---------------------------------------------------------------------------
// Corpus generators — identical to squash.rs, keep in sync
// ---------------------------------------------------------------------------

fn corpus_text(size: usize) -> Vec<u8> {
    const SENTENCES: &[&str] = &[
        "It was the best of times, it was the worst of times, it was the age of wisdom. ",
        "The quick brown fox jumps over the lazy dog near the riverbank at sunset. ",
        "Alice was beginning to get very tired of sitting by her sister on the bank. ",
        "Call me Ishmael. Some years ago, never mind how long precisely, I went to sea. ",
        "In a hole in the ground there lived a hobbit. Not a nasty dirty wet hole. ",
        "To be or not to be, that is the question whether it is nobler in the mind. ",
        "It is a truth universally acknowledged that a single man must be in want. ",
        "All happy families are alike; every unhappy family is unhappy in its own way. ",
        "Far out in the uncharted backwaters of the unfashionable end of the spiral. ",
        "The sky above the port was the color of television tuned to a dead channel. ",
    ];
    let mut buf = Vec::with_capacity(size);
    let mut i = 0;
    while buf.len() < size {
        buf.extend_from_slice(SENTENCES[i % SENTENCES.len()].as_bytes());
        i += 1;
    }
    buf.truncate(size);
    buf
}

fn corpus_xml(size: usize) -> Vec<u8> {
    const FRAGMENTS: &[&str] = &[
        "<record id=\"1001\" type=\"entry\">\n  <field name=\"title\">Benchmark Data</field>\n  <field name=\"value\">42</field>\n</record>\n",
        "<item category=\"alpha\" priority=\"high\">\n  <name>Widget A</name>\n  <description>A high-quality widget for testing purposes</description>\n  <price currency=\"USD\">19.99</price>\n</item>\n",
        "<log level=\"INFO\" timestamp=\"2025-01-15T10:30:00Z\">\n  <message>Processing request from client 192.168.1.100</message>\n  <duration unit=\"ms\">234</duration>\n</log>\n",
        "<config version=\"2.0\">\n  <setting key=\"max_threads\">8</setting>\n  <setting key=\"buffer_size\">4096</setting>\n  <setting key=\"compression\">enabled</setting>\n</config>\n",
    ];
    let mut buf = Vec::with_capacity(size);
    buf.extend_from_slice(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root>\n");
    let mut i = 0;
    while buf.len() < size {
        buf.extend_from_slice(FRAGMENTS[i % FRAGMENTS.len()].as_bytes());
        i += 1;
    }
    buf.truncate(size);
    buf
}

fn corpus_source_code(size: usize) -> Vec<u8> {
    const SNIPPETS: &[&str] = &[
        "int main(int argc, char **argv) {\n    if (argc < 2) {\n        fprintf(stderr, \"Usage: %s <file>\\n\", argv[0]);\n        return 1;\n    }\n    FILE *fp = fopen(argv[1], \"rb\");\n    if (!fp) { perror(\"fopen\"); return 1; }\n    fclose(fp);\n    return 0;\n}\n\n",
        "void process_buffer(const uint8_t *buf, size_t len) {\n    size_t offset = 0;\n    while (offset < len) {\n        uint32_t header = read_u32(buf + offset);\n        size_t chunk = header & 0xFFFF;\n        offset += 4 + chunk;\n    }\n}\n\n",
        "struct Node {\n    int key;\n    struct Node *left;\n    struct Node *right;\n};\n\nstruct Node *insert(struct Node *root, int key) {\n    if (!root) {\n        root = malloc(sizeof(struct Node));\n        root->key = key;\n        root->left = root->right = NULL;\n        return root;\n    }\n    if (key < root->key) root->left = insert(root->left, key);\n    else root->right = insert(root->right, key);\n    return root;\n}\n\n",
        "#define MAX_ENTRIES 1024\n#define HASH_MASK   0x3FF\n\nstatic uint32_t table[MAX_ENTRIES];\n\nvoid hash_insert(uint32_t value) {\n    uint32_t idx = value & HASH_MASK;\n    while (table[idx] != 0) { idx = (idx + 1) & HASH_MASK; }\n    table[idx] = value;\n}\n\n",
    ];
    let mut buf = Vec::with_capacity(size);
    let mut i = 0;
    while buf.len() < size {
        buf.extend_from_slice(SNIPPETS[i % SNIPPETS.len()].as_bytes());
        i += 1;
    }
    buf.truncate(size);
    buf
}

fn corpus_executable(size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(size);
    let mut lcg: u64 = 0xDEAD_BEEF_CAFE_1234;

    buf.extend_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0]);
    buf.extend_from_slice(&[0; 8]);

    while buf.len() < size {
        let section = buf.len() % 256;
        match section {
            0..=15 => buf.push(0x00),
            16..=47 => {
                let val = (buf.len() as u32).wrapping_mul(0x9E3779B9);
                buf.extend_from_slice(&val.to_le_bytes());
            }
            48..=79 => {
                for &b in b"_func_" {
                    buf.push(b);
                }
                buf.push(((buf.len() / 6) % 10 + b'0' as usize) as u8);
                buf.push(0x00);
            }
            _ => {
                lcg = lcg
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                buf.push((lcg >> 33) as u8);
            }
        }
    }
    buf.truncate(size);
    buf
}

fn corpus_database(size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(size);
    let mut record_id: u32 = 1;
    while buf.len() < size {
        buf.extend_from_slice(&record_id.to_le_bytes());

        let name = format!("record_{:08}", record_id);
        let name_bytes = name.as_bytes();
        buf.extend_from_slice(&name_bytes[..name_bytes.len().min(32)]);
        for _ in name_bytes.len()..32 {
            buf.push(0x00);
        }

        let val = (record_id as f64 * 1.618033988).to_le_bytes();
        buf.extend_from_slice(&val);

        let flags: u32 = if record_id % 7 == 0 { 0x01 } else { 0x00 };
        buf.extend_from_slice(&flags.to_le_bytes());

        buf.extend_from_slice(&[0x00; 16]);

        record_id += 1;
    }
    buf.truncate(size);
    buf
}

fn corpus_medical_image(size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(size);
    let width = 512usize;
    let mut lcg: u64 = 0x1234_5678_9ABC_DEF0;

    for i in 0..size {
        let x = i % width;
        let y = i / width;
        let base = ((x as f64 / width as f64) * 200.0 + (y as f64 / width as f64) * 55.0) as u8;
        lcg = lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let noise = ((lcg >> 33) % 8) as u8;
        buf.push(base.wrapping_add(noise));
    }
    buf
}

fn corpus_json(size: usize) -> Vec<u8> {
    const ENTRIES: &[&str] = &[
        r#"  {"id": 1, "name": "Alice Johnson", "email": "alice@example.com", "active": true, "score": 94.5},"#,
        r#"  {"id": 2, "name": "Bob Smith", "email": "bob@example.com", "active": false, "score": 87.2},"#,
        r#"  {"id": 3, "name": "Carol White", "email": "carol@example.com", "active": true, "score": 91.0},"#,
        r#"  {"id": 4, "name": "Dave Brown", "email": "dave@example.com", "active": true, "score": 78.3},"#,
        r#"  {"id": 5, "name": "Eve Davis", "email": "eve@example.com", "active": false, "score": 95.8},"#,
    ];
    let mut buf = Vec::with_capacity(size);
    buf.extend_from_slice(b"[\n");
    let mut i = 0;
    while buf.len() < size {
        buf.extend_from_slice(b"\n");
        buf.extend_from_slice(ENTRIES[i % ENTRIES.len()].as_bytes());
        i += 1;
    }
    buf.truncate(size);
    buf
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

// ---------------------------------------------------------------------------
// Timing harness
// ---------------------------------------------------------------------------

struct TimingResult {
    iterations: u32,
    elapsed: Duration,
    bytes_per_iter: usize,
}

impl TimingResult {
    fn throughput_mb_s(&self) -> f64 {
        let bytes_total = self.bytes_per_iter as f64 * self.iterations as f64;
        let seconds = self.elapsed.as_secs_f64();
        bytes_total / seconds / (1024.0 * 1024.0)
    }
}

fn bench_loop<F: FnMut()>(mut f: F, bytes_per_iter: usize) -> TimingResult {
    for _ in 0..WARMUP_ITERS {
        f();
    }

    let mut iterations = 0u32;
    let start = Instant::now();
    loop {
        f();
        iterations += 1;
        let elapsed = start.elapsed();
        if iterations >= MIN_ITERS && elapsed >= MIN_DURATION {
            return TimingResult {
                iterations,
                elapsed,
                bytes_per_iter,
            };
        }
    }
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct CorpusResult {
    name: &'static str,
    weight: f64,
    input_size: usize,
    compressed_size: usize,
    ratio: f64,
    compress_mb_s: f64,
    decompress_mb_s: f64,
}

fn measure_all_corpora(size: usize, level: i32) -> Vec<CorpusResult> {
    let mut results = Vec::with_capacity(WEIGHTED_CORPORA.len());

    for wc in WEIGHTED_CORPORA {
        let data = (wc.generate)(size);
        let compressed = compress(&data, level).expect("compress failed");
        let ratio = compressed.len() as f64 / data.len() as f64;

        let compress_timing = bench_loop(
            || {
                let _ = compress(black_box(&data), level).unwrap();
            },
            data.len(),
        );

        let decompress_timing = bench_loop(
            || {
                let _ = decompress(black_box(&compressed)).unwrap();
            },
            data.len(),
        );

        results.push(CorpusResult {
            name: wc.name,
            weight: wc.weight,
            input_size: data.len(),
            compressed_size: compressed.len(),
            ratio,
            compress_mb_s: compress_timing.throughput_mb_s(),
            decompress_mb_s: decompress_timing.throughput_mb_s(),
        });
    }

    results
}

fn compute_weighted_scores(results: &[CorpusResult]) -> (f64, f64, f64) {
    let mut weighted_ratio = 0.0;
    let mut weighted_compress = 0.0;
    let mut weighted_decompress = 0.0;

    for r in results {
        weighted_ratio += r.weight * r.ratio;
        weighted_compress += r.weight * r.compress_mb_s;
        weighted_decompress += r.weight * r.decompress_mb_s;
    }

    (weighted_ratio, weighted_compress, weighted_decompress)
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn print_results(results: &[CorpusResult], size: usize, level: i32) {
    let (w_ratio, w_compress, w_decompress) = compute_weighted_scores(results);

    // Human-readable table
    eprintln!();
    eprintln!("┌────────────────────────────────────────────────────────────────────────────────┐");
    eprintln!(
        "│ Weighted benchmark — {} KiB, level {:<43}│",
        size / 1024,
        level
    );
    eprintln!("├────────────────┬────────┬──────────┬───────────────┬─────────────────────────┤");
    eprintln!(
        "│ {:<14} │ {:>6} │ {:>8} │ {:>13} │ {:>23} │",
        "corpus", "weight", "ratio", "compress MB/s", "decompress MB/s"
    );
    eprintln!("├────────────────┼────────┼──────────┼───────────────┼─────────────────────────┤");

    for r in results {
        eprintln!(
            "│ {:<14} │ {:>5.0}% │ {:>7.1}% │ {:>13.1} │ {:>23.1} │",
            r.name,
            r.weight * 100.0,
            r.ratio * 100.0,
            r.compress_mb_s,
            r.decompress_mb_s
        );
    }

    eprintln!("├────────────────┴────────┼──────────┼───────────────┼─────────────────────────┤");
    eprintln!(
        "│ WEIGHTED SCORE         │ {:>7.1}% │ {:>13.1} │ {:>23.1} │",
        w_ratio * 100.0,
        w_compress,
        w_decompress
    );
    eprintln!("└────────────────────────┴──────────┴───────────────┴─────────────────────────┘");
    eprintln!();

    // Machine-parseable block
    eprintln!("[weighted-benchmark]");
    eprintln!("level={}", level);
    eprintln!("corpus_size={}", size);
    eprintln!("weighted_ratio={:.4}", w_ratio);
    eprintln!("weighted_compress_mb_s={:.1}", w_compress);
    eprintln!("weighted_decompress_mb_s={:.1}", w_decompress);
    eprintln!("[/weighted-benchmark]");
    eprintln!();
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    // Verify weights sum to 1.0
    let weight_sum: f64 = WEIGHTED_CORPORA.iter().map(|wc| wc.weight).sum();
    debug_assert!(
        (weight_sum - 1.0).abs() < 1e-9,
        "weights must sum to 1.0, got {}",
        weight_sum
    );

    let full = full_benchmarks_enabled();
    let size = if full { CORPUS_SIZE_FULL } else { CORPUS_SIZE };
    let levels: Vec<i32> = if full {
        vec![1, 3, 9, 19]
    } else {
        vec![BENCH_LEVEL]
    };

    for &level in &levels {
        let results = measure_all_corpora(size, level);
        print_results(&results, size, level);
    }
}
