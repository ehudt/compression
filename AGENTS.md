# AGENTS.md — Agent guide for zstd_rs

This file is the canonical AI-agent guide for this repository.
It is written to be compatible with both **Codex** and **Claude Code**.

> If you also update `CLAUDE.md`, keep the operational guidance aligned.

---

## Project at a glance

Pure-Rust implementation of [Zstandard](https://facebook.github.io/zstd/)
compression (`src/`, library crate `zstd_rs`). The public API is three
functions in `src/lib.rs`:

```rust
compress(data: &[u8], level: i32) -> Result<Vec<u8>>   // level 1-22
decompress(data: &[u8])            -> Result<Vec<u8>>
compress_bound(data_len: usize)    -> usize              // worst-case output size
```

---

## Essential commands

```bash
cargo build              # compile library + binary
cargo test               # unit tests + integration tests + doctests (must all pass)
cargo test --test integration  # integration tests only
cargo bench              # Criterion benchmarks (speed + ratio)
cargo run --example basic       # demo
cargo run --bin zstd_rs -- compress 3 input.txt out.zst
cargo run --bin zstd_rs -- decompress out.zst result.txt
```

Tests must pass before committing. The project has zero warnings in the
default configuration; do not introduce new warnings.

---

## Repository layout

```
src/
  lib.rs                 Public API (compress / decompress / compress_bound)
  main.rs                CLI binary
  error.rs               ZstdError enum — all error variants live here
  xxhash.rs              XXHash-32 content checksum
  frame.rs               Frame encoder + decoder (outermost layer)
  fse.rs                 Finite State Entropy tables + bit I/O
  huffman.rs             Canonical Huffman coding for literals
  encoder/
    mod.rs               re-exports MatchConfig
    block.rs             Compressed block encoder (Huffman literals, 0 sequences)
    lz77.rs              Hash-chain LZ77 match finder
  decoder/
    mod.rs               decode_block() dispatcher
    literals.rs          Literal section decoder (raw / RLE / Huffman)
    sequences.rs         Sequence section decoder (FSE) + sequence execution
  tables/
    sequences.rs         Predefined FSE norms; literal-length / match-length /
                         offset extra-bits lookup tables
tests/
  integration.rs         23 end-to-end round-trip and error-case tests
benches/
  compression.rs         Criterion speed benchmarks + compression-ratio tests
examples/
  basic.rs               Simple compress/decompress demo
```

---

## Error handling rules

- All public functions return `Result<_, ZstdError>` from `error.rs`.
- Corrupt input returns `ZstdError::CorruptData` or a specific variant.
- Do **not** panic on malformed input — return an error.
- Decoder must handle all cases: wrong magic, truncated stream, bad
  checksum, invalid block type.

---

## Testing policy

- Every new module gets at least one `#[cfg(test)]` block with basic
  unit tests.
- Encoder/decoder pairs must be tested with a round-trip that asserts
  byte-for-byte equality.
- The integration test file (`tests/integration.rs`) covers sizes 0,
  1, 255, 256, 1 024, 10 000, 100 000 bytes; all fast levels; and all
  error paths. New features should add cases there.
- Run `cargo test` before every commit.
