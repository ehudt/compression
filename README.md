# zstd_rs

A from-scratch implementation of the [Zstandard (zstd)](https://facebook.github.io/zstd/)
compression algorithm in pure Rust, written without any C dependencies.

## Features

- **Full zstd frame format** — magic number, frame header, window descriptor, content checksum
- **Huffman-coded literals** — single-stream compressed literals with canonical Huffman codes
- **FSE-coded sequences** — literal-length, match-length, and offset streams use zstd's predefined FSE tables
- **RLE and raw literal blocks** — automatic selection for best compression
- **Multi-block support** — large inputs are split into 128 KiB blocks
- **Content checksum** — XXHash-32 verification on decompression
- **Compression levels 1–22** — matching zstd's level conventions
- **LZ77 match finder** — hash-chain based, wired into sequence generation for compressed blocks

## Quick start

```rust
use zstd_rs::{compress, decompress};

let original = b"Hello, world! The quick brown fox.";
let compressed = compress(original, 3)?;
let decompressed = decompress(&compressed)?;
assert_eq!(decompressed, original);
```

## Usage

```bash
# Compress a file
cargo run --bin zstd_rs -- compress 3 input.txt output.zst

# Decompress
cargo run --bin zstd_rs -- decompress output.zst result.txt
```

## Running tests

```bash
cargo test              # Unit + integration tests
cargo bench             # Default Criterion benches, including the squash-style suite
cargo bench --bench squash  # Squash-style corpus benchmark only
ZSTD_RS_FULL_BENCHES=1 cargo bench --bench squash  # Exhaustive squash sweep
cargo run --release --example silesia_bench -- --download  # Silesia corpus comparison + SVG output
cargo run --example basic
```

### Acceptance tests (interoperability with the system `zstd` tool)

The acceptance tests in `tests/acceptance.rs` verify round-trip correctness
against the reference C implementation in both directions:

1. **Our compress → `zstd -d`** — our output must be a valid zstd stream.
2. **`zstd` compress → our decompress** — we must correctly decode reference output.

**Prerequisite:** install the `zstd` CLI (v1.4+).

```bash
# Debian / Ubuntu
sudo apt-get install zstd

# macOS
brew install zstd

# Run acceptance tests only
cargo test --test acceptance

# Run alongside all other tests
cargo test
```

Tests skip gracefully (with a message) if `zstd` is not found in `PATH`.

## Profiling

Profiling is opt-in and has no runtime impact on normal builds. The repo exposes:

- a `profiling` Cargo feature that compiles in CPU sampling support
- a `profiling` Cargo profile that keeps release-like optimization while preserving symbols

### Command profiling

Build with profiling support, then pass `--profile-cpu` before the subcommand:

```bash
cargo run --profile profiling --features profiling -- \
  --profile-cpu profiles/compress.svg \
  --profile-repeat 200 \
  --profile-min-ms 500 \
  --profile-hz 1000 \
  compress 3 input.txt output.zst
```

The generated file is an SVG flamegraph. When `--profile-cpu` is enabled, the
CLI behaves more like a microbenchmark harness:

- it runs one unprofiled warmup iteration first
- it profiles only the steady-state in-memory loop
- it keeps running until both `--profile-repeat` and `--profile-min-ms` are satisfied

The final output file is still written only once, from the last measured
iteration. The same profile capture also writes:

- `*.folded` — folded stack samples in plain text
- `*.summary.txt` — a compact textual summary with top leaf symbols, top inclusive symbols, and top stacks

The default sampling rate is `1000 Hz`. On very small inputs, `--profile-min-ms`
and `--profile-hz` are often the simplest knobs to turn. If a capture is too
sparse to be useful, the CLI emits a warning and the summary file records it
explicitly.

### Test profiling

Set `ZSTD_RS_PROFILE_TESTS` to an output directory. Each integration test writes its
own profile artifacts as `<test-name>.svg`, `<test-name>.folded`, and
`<test-name>.summary.txt`:

```bash
mkdir -p profiles/tests
ZSTD_RS_PROFILE_TESTS=profiles/tests \
  cargo test --profile profiling --features profiling --test integration -- --nocapture
```

### Benchmark profiling

Set `ZSTD_RS_PROFILE_BENCHES=1` to enable Criterion's `pprof` profiler:

```bash
ZSTD_RS_PROFILE_BENCHES=1 \
  cargo bench --profile profiling --features profiling
```

By default, `cargo bench` runs a small signal benchmark intended for quick
performance checks. It covers four compression/decompression cases on 64 KiB
inputs:

- `all_zeros` at level 3
- `repetitive` at level 3
- `binary_structured` at level 3
- `random` at level 1

It also includes two round-trip measurements (`repetitive` level 3 and
`random` level 1) and prints a compression-ratio summary for those timed cases.

Set `ZSTD_RS_FULL_BENCHES=1` to run the exhaustive `1..=22` sweep instead:

```bash
ZSTD_RS_FULL_BENCHES=1 cargo bench
```

You can combine both env vars:

```bash
ZSTD_RS_PROFILE_BENCHES=1 ZSTD_RS_FULL_BENCHES=1 \
  cargo bench --profile profiling --features profiling
```

When `ZSTD_RS_PROFILE_BENCHES` is not set, benches run without `pprof`.
When it is set, each benchmark profile directory gets `flamegraph.svg`,
`flamegraph.folded`, and `flamegraph.summary.txt`.

### Squash-style benchmark

The repo also includes a dedicated Criterion bench at `benches/squash.rs`
inspired by the Squash Compression Benchmark / Silesia-style corpus mix. It
uses synthetic corpora chosen to approximate common data categories instead of
timing only a few hand-picked samples.

Run the focused suite with:

```bash
cargo bench --bench squash
```

The default fast mode uses 64 KiB corpora and benchmarks compression and
decompression at levels `1`, `3`, `9`, and `19` for these categories:

- `text`
- `xml`
- `source_code`
- `executable`
- `database`
- `medical_image`
- `json`
- `random`

Before Criterion timing starts, the bench prints a squash-style summary table
showing corpus name, category, compression level, input size, compressed size,
and compression ratio.

Set `ZSTD_RS_FULL_BENCHES=1` to switch the suite into its exhaustive mode:

```bash
ZSTD_RS_FULL_BENCHES=1 cargo bench --bench squash
```

Full mode increases corpus size to 256 KiB, sweeps all compression levels
`1..=22`, and adds:

- size-scaling compression measurements at `4 KiB`, `16 KiB`, `64 KiB`, `256 KiB`, and `1 MiB`
- round-trip benchmarks for every corpus at level `3`

Without `ZSTD_RS_FULL_BENCHES`, the squash suite keeps Criterion settings short
for quick iteration by reducing sample size and measurement time.

### Silesia benchmark against official `zstd`

For a direct side-by-side comparison with the system `zstd` implementation on
the Silesia corpus, run:

```bash
cargo run --release --example silesia_bench -- --download --implementation both
```

This benchmark:

- caches the Silesia corpus under `benches/data/silesia/`
- benchmarks both `zstd_rs` and the system `zstd` CLI at levels `1,3,9,19`
- writes a README-style markdown table to `docs/benchmarks/silesia-comparison.md`
- writes raw results to `docs/benchmarks/silesia-comparison.json`
- writes a two-panel SVG comparison chart to `docs/benchmarks/silesia-comparison.svg`

Required host tools:

- Rust toolchain (`cargo`, `rustc`)
- `zstd` CLI in `PATH` for the official implementation comparison
- `curl` and `python3` in `PATH` when using `--download`

Useful flags:

```bash
cargo run --release --example silesia_bench -- \
  --corpus-dir benches/data/silesia \
  --output-dir docs/benchmarks \
  --implementation both \
  --levels 1,3,9,19 \
  --min-bench-ms 1000
```

Common one-line variants:

```bash
# Both implementations on the same machine
cargo run --release --example silesia_bench -- --download --implementation both

# Official zstd only
cargo run --release --example silesia_bench -- --download --implementation official

# zstd_rs only
cargo run --release --example silesia_bench -- --download --implementation ours
```

To benchmark only the official implementation on the same machine:

```bash
cargo run --release --example silesia_bench -- \
  --download \
  --implementation official
```

## Architecture

```
src/
  lib.rs            – Public API (compress / decompress / compress_bound)
  main.rs           – CLI binary
  frame.rs          – Frame encoder/decoder (magic, header, blocks, checksum)
  error.rs          – ZstdError enum
  xxhash.rs         – XXHash-32 (content checksum)
  huffman.rs        – Huffman coding for literals
  fse.rs            – Finite State Entropy tables (FSE / tANS)
  encoder/
    mod.rs
    block.rs        – Compressed block encoder (Huffman literals + FSE sequences)
    lz77.rs         – Hash-chain LZ77 match finder
  decoder/
    mod.rs
    literals.rs     – Literal section decoder (raw / RLE / Huffman)
    sequences.rs    – Sequence section decoder (FSE)
  tables/
    sequences.rs    – Default FSE distribution tables + extra-bits lookup tables
```

## Compression approach

The encoder now follows the basic zstd compressed-block pipeline:

- parse the block with the hash-chain LZ77 matcher
- collect literal runs and `(literal_length, match_length, offset)` sequences
- Huffman-encode the literal section
- FSE-encode the sequence section with zstd's predefined LL/ML/OF tables

For compatibility and simplicity, the encoder currently emits offsets through
the non-repeat-offset path and uses predefined FSE tables rather than
per-block custom tables. If sequence encoding fails validation, the encoder
falls back to a literals-only block so decompression remains correct.

## License

MIT
