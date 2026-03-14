# zstd_rs

A from-scratch implementation of the [Zstandard (zstd)](https://facebook.github.io/zstd/)
compression algorithm in pure Rust, written without any C dependencies.

## Features

- **Full zstd frame format** — magic number, frame header, window descriptor, content checksum
- **Huffman-coded literals** — single-stream compressed literals with canonical Huffman codes
- **RLE and raw literal blocks** — automatic selection for best compression
- **Multi-block support** — large inputs are split into 128 KiB blocks
- **Content checksum** — XXHash-32 verification on decompression
- **Compression levels 1–22** — matching zstd's level conventions
- **LZ77 match finder** — hash-chain based (foundation for FSE sequence encoding, planned)

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

## AI agent documentation

- `AGENTS.md` is the canonical cross-agent guide (Codex + Claude Code).
- `CLAUDE.md` contains Claude Code-specific framing and points back to `AGENTS.md`.

## Running tests

```bash
cargo test              # Unit + integration tests
cargo bench             # Criterion benchmarks
cargo run --example basic
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
    block.rs        – Compressed block encoder (Huffman literals + 0 sequences)
    lz77.rs         – Hash-chain LZ77 match finder
  decoder/
    mod.rs
    literals.rs     – Literal section decoder (raw / RLE / Huffman)
    sequences.rs    – Sequence section decoder (FSE)
  tables/
    sequences.rs    – Default FSE distribution tables + extra-bits lookup tables
```

## Compression approach

The encoder currently encodes all data as Huffman-coded literals with 0 LZ77
sequences.  This gives excellent compression ratios on highly repetitive data
(via Huffman entropy coding) but does not exploit long-range redundancy the way
the full zstd LZ77 + FSE sequence path would.

The LZ77 match finder (`encoder/lz77.rs`) and all FSE infrastructure (`fse.rs`)
are fully implemented and the decoder supports the complete FSE sequence format.
Wiring the encoder's LZ77 output through the FSE sequence encoder is the primary
remaining work item.

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
| FSE sequence encoder | 🔧 Planned |

## License

MIT
