//! Squash-style compression benchmark suite.
//!
//! Inspired by the [Squash Compression Benchmark](https://quixdb.github.io/squash-benchmark/),
//! this suite measures compression throughput (MB/s), decompression throughput (MB/s),
//! and compression ratio across diverse data categories representative of real-world
//! workloads.
//!
//! Run with: `cargo bench --bench squash`
//! Full sweep: `ZSTD_RS_FULL_BENCHES=1 cargo bench --bench squash`

use std::env;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use zstd_rs::{compress, decompress};

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

fn criterion_config() -> Criterion {
    let mut c = Criterion::default();
    if !full_benchmarks_enabled() {
        c = c
            .sample_size(10)
            .warm_up_time(Duration::from_millis(250))
            .measurement_time(Duration::from_secs(1));
    }
    c
}

// ---------------------------------------------------------------------------
// Corpus generators — synthetic data modelling the Squash/Silesia categories
// ---------------------------------------------------------------------------

/// English prose text — models Silesia `dickens`, Canterbury `alice29.txt`.
fn corpus_text(size: usize) -> Vec<u8> {
    // Repeating natural-language sentences with varied vocabulary.
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

/// XML/HTML markup — models Silesia `xml`, structured markup with tags and attributes.
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

/// Source code — models Silesia `mozilla` (compiled), here we use C-like source text.
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

/// Binary executable — models compiled object code / shared libraries.
/// Mix of structured headers, jump tables, and pseudo-random instruction bytes.
fn corpus_executable(size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(size);
    let mut lcg: u64 = 0xDEAD_BEEF_CAFE_1234;

    // ELF-like header stub
    buf.extend_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0]);
    buf.extend_from_slice(&[0; 8]); // padding

    while buf.len() < size {
        let section = buf.len() % 256;
        match section {
            // Simulate alignment padding
            0..=15 => buf.push(0x00),
            // Simulate jump table entries (repeated 4-byte patterns)
            16..=47 => {
                let val = (buf.len() as u32).wrapping_mul(0x9E3779B9);
                buf.extend_from_slice(&val.to_le_bytes());
            }
            // Simulate string table
            48..=79 => {
                for &b in b"_func_" {
                    buf.push(b);
                }
                buf.push(((buf.len() / 6) % 10 + b'0' as usize) as u8);
                buf.push(0x00);
            }
            // Pseudo-random "instruction" bytes
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

/// Database records — models Silesia `osdb`, tabular structured binary data.
fn corpus_database(size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(size);
    let mut record_id: u32 = 1;
    // Fixed-width records: 4B id, 32B name, 8B value, 4B flags, 16B padding = 64B
    while buf.len() < size {
        buf.extend_from_slice(&record_id.to_le_bytes());

        // Name field — ASCII padded with nulls
        let name = format!("record_{:08}", record_id);
        let name_bytes = name.as_bytes();
        buf.extend_from_slice(&name_bytes[..name_bytes.len().min(32)]);
        for _ in name_bytes.len()..32 {
            buf.push(0x00);
        }

        // Value — slowly changing float-like pattern
        let val = (record_id as f64 * 1.618033988).to_le_bytes();
        buf.extend_from_slice(&val);

        // Flags — mostly zeros with occasional bits set
        let flags: u32 = if record_id % 7 == 0 { 0x01 } else { 0x00 };
        buf.extend_from_slice(&flags.to_le_bytes());

        // Padding
        buf.extend_from_slice(&[0x00; 16]);

        record_id += 1;
    }
    buf.truncate(size);
    buf
}

/// Medical imaging — models Silesia `mr` / `x-ray`, smooth gradients with noise.
fn corpus_medical_image(size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(size);
    let width = 512usize;
    let mut lcg: u64 = 0x1234_5678_9ABC_DEF0;

    for i in 0..size {
        let x = i % width;
        let y = i / width;
        // Smooth gradient with subtle noise — mimics grayscale medical images
        let base = ((x as f64 / width as f64) * 200.0
            + (y as f64 / width as f64) * 55.0) as u8;
        lcg = lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let noise = ((lcg >> 33) % 8) as u8;
        buf.push(base.wrapping_add(noise));
    }
    buf
}

/// Pseudo-random (incompressible) — baseline for overhead measurement.
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

/// JSON data — models API responses, config files, log structured data.
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

// ---------------------------------------------------------------------------
// Corpus registry
// ---------------------------------------------------------------------------

struct CorpusDef {
    name: &'static str,
    category: &'static str,
    generate: fn(usize) -> Vec<u8>,
}

const CORPORA: &[CorpusDef] = &[
    CorpusDef {
        name: "text",
        category: "text",
        generate: corpus_text,
    },
    CorpusDef {
        name: "xml",
        category: "markup",
        generate: corpus_xml,
    },
    CorpusDef {
        name: "source_code",
        category: "source",
        generate: corpus_source_code,
    },
    CorpusDef {
        name: "executable",
        category: "binary",
        generate: corpus_executable,
    },
    CorpusDef {
        name: "database",
        category: "structured",
        generate: corpus_database,
    },
    CorpusDef {
        name: "medical_image",
        category: "image",
        generate: corpus_medical_image,
    },
    CorpusDef {
        name: "json",
        category: "web",
        generate: corpus_json,
    },
    CorpusDef {
        name: "random",
        category: "noise",
        generate: corpus_random,
    },
];

// ---------------------------------------------------------------------------
// Benchmark cases
// ---------------------------------------------------------------------------

struct SquashCase {
    corpus_name: &'static str,
    category: &'static str,
    level: i32,
    input: Vec<u8>,
    compressed: Vec<u8>,
}

impl SquashCase {
    fn input_len(&self) -> usize {
        self.input.len()
    }

    fn compressed_len(&self) -> usize {
        self.compressed.len()
    }

    fn ratio(&self) -> f64 {
        self.compressed_len() as f64 / self.input_len() as f64
    }

    fn ratio_percent(&self) -> f64 {
        self.ratio() * 100.0
    }
}

fn build_cases(size: usize, levels: &[i32]) -> Vec<SquashCase> {
    let mut cases = Vec::new();
    for def in CORPORA {
        let data = (def.generate)(size);
        for &level in levels {
            let compressed = compress(&data, level).expect("compress failed in setup");
            cases.push(SquashCase {
                corpus_name: def.name,
                category: def.category,
                level,
                input: data.clone(),
                compressed,
            });
        }
    }
    cases
}

// ---------------------------------------------------------------------------
// Results table — squash-style presentation
// ---------------------------------------------------------------------------

fn print_squash_table(cases: &[SquashCase], size: usize) {
    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════════════════════════════╗");
    eprintln!(
        "║  Squash-style benchmark — {} KiB corpus size{} ║",
        size / 1024,
        " ".repeat(80 - 48 - format!("{}", size / 1024).len())
    );
    eprintln!("╠════════════════╤═══════════╤═══════╤═════════════╤════════════╤══════════════════╣");
    eprintln!(
        "║ {:<14} │ {:<9} │ {:>5} │ {:>11} │ {:>10} │ {:>16} ║",
        "corpus", "category", "level", "input bytes", "compressed", "ratio"
    );
    eprintln!("╟────────────────┼───────────┼───────┼─────────────┼────────────┼──────────────────╢");

    for case in cases {
        eprintln!(
            "║ {:<14} │ {:<9} │ {:>5} │ {:>11} │ {:>10} │ {:>15.1}% ║",
            case.corpus_name,
            case.category,
            case.level,
            case.input_len(),
            case.compressed_len(),
            case.ratio_percent()
        );
    }

    eprintln!("╚════════════════╧═══════════╧═══════╧═════════════╧════════════╧══════════════════╝");
    eprintln!();
}

// ---------------------------------------------------------------------------
// Benchmark entry points
// ---------------------------------------------------------------------------

fn bench_squash_fast(c: &mut Criterion) {
    const SIZE: usize = 64 * 1024;
    let levels = &[1, 3, 9, 19];
    let cases = build_cases(SIZE, levels);
    print_squash_table(&cases, SIZE);

    // Compression throughput
    let mut cg = c.benchmark_group("squash/compress");
    cg.throughput(Throughput::Bytes(SIZE as u64));
    for case in &cases {
        cg.bench_with_input(
            BenchmarkId::new(case.corpus_name, case.level),
            &case.input,
            |b, input| b.iter(|| compress(black_box(input), case.level).unwrap()),
        );
    }
    cg.finish();

    // Decompression throughput
    let mut dg = c.benchmark_group("squash/decompress");
    for case in &cases {
        dg.throughput(Throughput::Bytes(case.input_len() as u64));
        dg.bench_with_input(
            BenchmarkId::new(case.corpus_name, case.level),
            &case.compressed,
            |b, data| b.iter(|| decompress(black_box(data)).unwrap()),
        );
    }
    dg.finish();

    // Round-trip
    let mut rg = c.benchmark_group("squash/roundtrip");
    rg.throughput(Throughput::Bytes(SIZE as u64));
    // Pick one representative per category: text@3, executable@3, random@1
    for &(corpus, level) in &[("text", 3), ("executable", 3), ("random", 1)] {
        if let Some(case) = cases
            .iter()
            .find(|c| c.corpus_name == corpus && c.level == level)
        {
            rg.bench_function(
                format!("{}_level{}", case.corpus_name, case.level),
                |b| {
                    b.iter(|| {
                        let comp = compress(black_box(&case.input), case.level).unwrap();
                        decompress(black_box(&comp)).unwrap()
                    });
                },
            );
        }
    }
    rg.finish();
}

fn bench_squash_full(c: &mut Criterion) {
    const SIZE: usize = 256 * 1024;
    let levels: Vec<i32> = (1..=22).collect();
    let cases = build_cases(SIZE, &levels);
    print_squash_table(&cases, SIZE);

    // Compression throughput — all corpus/level combos
    let mut cg = c.benchmark_group("squash/compress");
    cg.throughput(Throughput::Bytes(SIZE as u64));
    for case in &cases {
        cg.bench_with_input(
            BenchmarkId::new(case.corpus_name, case.level),
            &case.input,
            |b, input| b.iter(|| compress(black_box(input), case.level).unwrap()),
        );
    }
    cg.finish();

    // Decompression throughput
    let mut dg = c.benchmark_group("squash/decompress");
    for case in &cases {
        dg.throughput(Throughput::Bytes(case.input_len() as u64));
        dg.bench_with_input(
            BenchmarkId::new(case.corpus_name, case.level),
            &case.compressed,
            |b, data| b.iter(|| decompress(black_box(data)).unwrap()),
        );
    }
    dg.finish();

    // Size scaling — test how throughput changes with input size
    let mut sg = c.benchmark_group("squash/size_scaling");
    for &sz in &[4 * 1024, 16 * 1024, 64 * 1024, 256 * 1024, 1024 * 1024] {
        sg.throughput(Throughput::Bytes(sz as u64));
        for def in CORPORA {
            let data = (def.generate)(sz);
            sg.bench_with_input(
                BenchmarkId::new(def.name, sz),
                &data,
                |b, input| b.iter(|| compress(black_box(input), 3).unwrap()),
            );
        }
    }
    sg.finish();

    // Round-trip all corpora at level 3
    let mut rg = c.benchmark_group("squash/roundtrip");
    rg.throughput(Throughput::Bytes(SIZE as u64));
    for def in CORPORA {
        let data = (def.generate)(SIZE);
        rg.bench_function(format!("{}_level3", def.name), |b| {
            b.iter(|| {
                let comp = compress(black_box(&data), 3).unwrap();
                decompress(black_box(&comp)).unwrap()
            });
        });
    }
    rg.finish();
}

fn squash_suite(c: &mut Criterion) {
    if full_benchmarks_enabled() {
        bench_squash_full(c);
    } else {
        bench_squash_fast(c);
    }
}

criterion_group! {
    name = squash;
    config = criterion_config();
    targets = squash_suite
}
criterion_main!(squash);
