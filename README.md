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
cargo bench             # Fast Criterion benchmarks (speed + ratio)
ZSTD_RS_FULL_BENCHES=1 cargo bench  # Exhaustive all-level benchmark sweep
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

By default, `cargo bench` runs a reduced representative suite over levels
`1, 3, 9, 19, 22` and prints a compression-ratio summary for the timed cases.
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

## Status

| Component | Status |
|-----------|--------|
| Frame format | ✅ Complete |
| Huffman literals | ✅ Complete |
| RLE / raw literals | ✅ Complete |
| Content checksum (xxhash32) | ✅ Complete |
| FSE tables (decode) | ✅ Complete |
| Sequence decoder | ✅ Complete |
| LZ77 match finder | ✅ Complete |
| FSE sequence encoder | ✅ Complete |

## License

MIT
