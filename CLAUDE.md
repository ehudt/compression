# CLAUDE.md — Agent guide for zstd_rs

Pure-Rust [Zstandard](https://facebook.github.io/zstd/) compression library
(`src/`, crate `zstd_rs`). Public API in `src/lib.rs`:

```rust
compress(data: &[u8], level: i32) -> Result<Vec<u8>>   // level 1-22
decompress(data: &[u8])            -> Result<Vec<u8>>
compress_bound(data_len: usize)    -> usize              // worst-case output size
```

---

## Essential commands

```bash
cargo build              # compile library + binary
cargo test               # unit + integration + doctests (must all pass)
cargo test --test integration  # integration tests only
cargo test --test acceptance   # interop tests against system zstd
cargo bench              # fast signal benchmark (speed + ratio)
cargo bench --bench weighted   # weighted composite benchmark
cargo bench --bench squash     # squash-style multi-corpus benchmark
cargo run --example basic      # demo
cargo run --bin zstd_rs -- compress 3 input.txt out.zst
cargo run --bin zstd_rs -- decompress out.zst result.txt
cargo test --features profiling       # verify profiling build path
cargo bench --features profiling --no-run  # compile Criterion + pprof
```

**Never run more than one benchmark at a time.** Concurrent benchmarks cause
CPU contention and unreliable numbers.

See `docs/benchmarking.md` for full benchmark/profiling details (Silesia,
squash, signal, profiling workflow, env vars).

### Acceptance tests

`tests/acceptance.rs` checks interop with the system `zstd` binary:

- **Our compress -> `zstd -d`**: 10 tests (empty, single byte, all-zeros,
  repetitive text, sequential bytes, pseudo-random, multi-block, level sweep
  1-19, all-256-byte-values).
- **`zstd` compress -> our decompress**: 10 tests at levels 1, 3, 9, 19.

Requires `zstd` CLI (v1.4+) in `PATH`. Tests skip gracefully if absent.

```bash
sudo apt-get install zstd   # Debian/Ubuntu
brew install zstd            # macOS
```

Tests must pass before committing. Zero warnings policy -- do not introduce new warnings.

---

## Repository layout

```
src/
  lib.rs                 Public API (compress / decompress / compress_bound)
  main.rs                CLI binary
  profiling.rs           Optional pprof session wrapper for CLI/tests
  error.rs               ZstdError enum — all error variants live here
  xxhash.rs              XXHash-32 content checksum
  frame.rs               Frame encoder + decoder (outermost layer)
  fse.rs                 Finite State Entropy tables + bit I/O
  huffman.rs             Canonical Huffman coding for literals
  encoder/
    mod.rs               re-exports MatchConfig
    block.rs             Compressed block encoder (Huffman literals + FSE sequences)
    lz77.rs              Hash-chain LZ77 parser / match finder
  decoder/
    mod.rs               decode_block() dispatcher
    literals.rs          Literal section decoder (raw / RLE / Huffman)
    sequences.rs         Sequence section decoder (FSE) + sequence execution
  tables/
    sequences.rs         Predefined FSE norms; LL/ML/offset extra-bits tables
tests/
  integration.rs         23 end-to-end round-trip and error-case tests
benches/
  compression.rs         Criterion speed benchmarks + ratio summary
  squash.rs              Squash-style multi-corpus benchmark (8 data categories)
examples/
  basic.rs               Simple compress/decompress demo
  silesia_bench.rs       Silesia corpus benchmark vs system zstd + SVG/JSON/MD
```

---

## Compression ratio: current state

Compression combines Huffman-coded literals with LZ77/FSE sequence coding.

| Data type | Typical ratio |
|---|---|
| Single repeated byte (`'a' * 10 000`) | < 0.1% of original |
| Short English text (repetitive) | materially better than literals-only mode |
| Truly random bytes | ~100% (falls back to raw block) |

---

## Error handling rules

- All public functions return `Result<_, ZstdError>` from `error.rs`.
- Corrupt input returns `ZstdError::CorruptData` or a specific variant.
- Never panic on malformed input -- return an error.
- Decoder must handle: wrong magic, truncated stream, bad checksum, invalid block type.

---

## Testing policy

- Every new module gets at least one `#[cfg(test)]` block.
- Encoder/decoder pairs need round-trip tests asserting byte-for-byte equality.
- `tests/integration.rs` covers sizes 0, 1, 255, 256, 1024, 10000, 100000 bytes;
  all fast levels; and all error paths. New features should add cases there.
- Run `cargo test` before every commit.

---

## Format & algorithm references

These docs have detailed encoding tables, invariants, and implementation notes:

- `docs/zstd-format.md` -- frame structure, block headers, literals section encoding, Huffman invariants
- `docs/fse-details.md` -- FSE/tANS theory, decoder/encoder implementation, current constraints
- `docs/benchmarking.md` -- all benchmark types, Silesia setup, profiling workflow
