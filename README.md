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
cargo bench             # Criterion benchmarks
cargo run --example basic
```

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
  compress 3 input.txt output.zst
```

The generated file is an SVG flamegraph. `--profile-repeat` reruns the in-memory
compression or decompression workload before writing the final output once, which
is useful when a single command is too short-lived to collect samples.

### Test profiling

Set `ZSTD_RS_PROFILE_TESTS` to an output directory. Each integration test writes its
own flamegraph as `<test-name>.svg`:

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

When that env var is not set, benches run exactly as before.

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
