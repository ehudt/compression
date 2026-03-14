# CLAUDE.md — Agent guide for zstd_rs

This file gives an AI agent everything it needs to work productively in this
codebase without reading every source file first.

---

## Project at a glance

Pure-Rust implementation of [Zstandard](https://facebook.github.io/zstd/)
compression (`src/`, library crate `zstd_rs`).  The public API is three
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
cargo test --test acceptance   # interoperability tests against system zstd (see below)
cargo bench              # Criterion benchmarks (speed + ratio)
cargo run --example basic       # demo
cargo run --bin zstd_rs -- compress 3 input.txt out.zst
cargo run --bin zstd_rs -- decompress out.zst result.txt
```

### Acceptance tests

`tests/acceptance.rs` checks interoperability with the system `zstd` binary in
two directions:

- **Our compress → `zstd -d`**: 10 tests (empty, single byte, all-zeros,
  repetitive text, sequential bytes, pseudo-random, multi-block, level sweep
  1–19, all-256-byte-values).
- **`zstd` compress → our decompress**: 10 tests over the same corpus at
  levels 1, 3, 9, 19.

**Prerequisite:** the `zstd` CLI (v1.4+) must be in `PATH`.

```bash
# Install on Debian/Ubuntu
sudo apt-get install zstd

# Install on macOS
brew install zstd
```

If the binary is absent the tests skip with an explanatory message rather
than failing, so the suite remains usable in environments without the tool.

Tests must pass before committing.  The project has zero warnings in the
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
    block.rs             Compressed block encoder (Huffman literals + FSE sequences)
    lz77.rs              Hash-chain LZ77 parser / match finder
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

## Zstd frame format (brief)

A zstd-compressed file is one or more **frames**, each structured as:

```
[Magic 4B][Frame Header][Block...][Optional checksum 4B]
```

**Frame Header** (written by `frame.rs:compress_with_config`):
- FHD byte: `FCS_flag=2` (4-byte content size), no dict, checksum bit
- Window descriptor byte: 56 (= 128 KiB window)
- 4-byte content size (u32 LE)

**Block header** (3 bytes):
- bit[0]: last-block flag
- bits[2:1]: block type — 0=raw, 1=RLE, 2=compressed
- bits[23:3]: block size in bytes

Block payload for type 2 (compressed):
```
[Literals section][Sequences section]
```

---

## Literals section encoding

The size field in the first byte(s) uses **different packing depending on
`size_format` (bits [3:2] of byte 0)**:

| `size_format` | Header bytes | Size bits in byte 0 | Shift to decode |
|---|---|---|---|
| 0 | 1 | bits [7:3] (5 bits) | `byte0 >> 3` |
| 1 | 2 | bits [7:4] (4 bits) | `byte0 >> 4` |
| 2 | 1 | same as 0 | `byte0 >> 3` |
| 3 | 3 | bits [7:4] (4 bits) | `byte0 >> 4` |

**Encoder constants** (`src/encoder/block.rs`):
- raw sf=0: `byte0 = (n << 3) as u8`
- raw sf=1: `byte0 = 0x04 | ((n & 0xF) << 4) as u8`
- raw sf=3: `byte0 = 0x0C | ((n & 0xF) << 4) as u8`
- RLE sf=0: `byte0 = 0x01 | ((n << 3) as u8)`
- RLE sf=1: `byte0 = 0x05 | ((n & 0xF) << 4) as u8`
- RLE sf=3: `byte0 = 0x0D | ((n & 0xF) << 4) as u8`
- Compressed sf=2 (4-byte header): `byte0 = 0x0A | ((regen & 0xF) << 4) as u8`

If you add a new sf value, update **both** `encoder/block.rs` and
`decoder/literals.rs:decode_size_raw` / `decode_size_compressed`.

---

## Huffman coding invariants

`HuffmanTable::from_weights` (`src/huffman.rs`):

- `max_bits` is **NOT** `max_weight`.  It is computed from the weight sum:
  `max_bits = next_power_of_two(sum_weight).ilog2()`
  where `sum_weight = Σ 2^(w_i − 1)` for active symbols.
  This must equal `2^max_bits` for a complete tree.
- Canonical codes are generated starting from `next_code[1] = 0`.
  `bl_count[0]` (absent symbols) **must not** influence code generation.
  The loop starts at `bits = 2`, not `bits = 1`.
- When building the decode lookup table (`HuffmanTable::decode`), each symbol
  fills `1 << (max_bits - len)` consecutive entries.  If any `code >= (1 << len)`
  the table overflows — this indicates invalid lengths from `build_lengths`.

---

## FSE (Finite State Entropy) — what it is and how this repo uses it

### What FSE is

FSE (also called tANS — table-based Asymmetric Numeral Systems) is a
near-optimal entropy coder.  It is zstd's replacement for Huffman coding in
the **sequences section** of each compressed block.

A sequence is a triple `(literal_length, match_length, offset)` produced by
the LZ77 pass.  zstd uses FSE to entropy-code the three symbol streams
(LL codes, ML codes, OF codes) jointly into a single backward bitstream.

The FSE state machine works as follows:

- A **decode table** maps each state (integer in `[0, table_size)`) to a
  `(symbol, num_bits, base_line)` triple.  To decode: read `num_bits` bits,
  new state = `base_line + bits_read`.
- An **encode table** maps each symbol to `(delta_find_state, num_bits,
  find_state_min)`.  To encode symbol S from state s: output the lower
  `num_bits` bits of s, new state = `(s >> num_bits) + delta_find_state`.
- The encode and decode tables are derived from the same **normalized
  probability distribution** (stored in each compressed block header, or
  taken from a predefined default).

The sequence bitstream is written **in reverse** — the encoder processes
sequences last-to-first and writes bits into a backward-growing buffer,
which the decoder reads forward (after the sentinel bit).

### What is implemented

- `fse.rs`: `normalize_counts`, `build_decode_table`, `build_encode_table`,
  `read_distribution_table`, `BitReader`, `BitWriter`.
- `tables/sequences.rs`: the predefined FSE norms for LL / ML / OF
  (`LITERALS_LENGTH_DEFAULT_NORM`, `MATCH_LENGTH_DEFAULT_NORM`,
  `OFFSET_DEFAULT_NORM`) and the extra-bits lookup tables.
- `decoder/sequences.rs`: full FSE decoder including state initialization,
  per-sequence state advance, extra-bit decoding, repeat-offset handling,
  and sequence execution.

### Current encoder behavior

`encoder/block.rs:encode_block` now emits real sequences:

1. `encoder/lz77.rs:parse(data, cfg)` produces literal runs and matches.
2. `collect_sequences()` keeps literal bytes separate from sequence commands.
3. Lengths/offsets are mapped to `(ll_code, ml_code, of_code)` plus their
   extra bits using the tables in `tables/sequences.rs`.
4. `encode_sequences()` writes:
   - sequence count
   - mode byte `0x00` (predefined LL/OF/ML tables)
   - initial LL / OF / ML states
   - per-sequence offset extra bits, match-length extra bits, literal-length
     extra bits, then FSE state transitions for every sequence except the last
5. `BitWriter::finish()` reverses the buffered bytes to match the decoder's
   backwards bitstream convention.

The encoder derives a valid state path by inverting the predefined decode
tables (`build_state_path()` + `inverse_transition()`) instead of maintaining a
separate FSE encoder-table implementation.

Two important current constraints:

- Offsets are always emitted through the non-repeat path: `raw_offset = offset + 3`.
  This is simpler than modeling zstd's repeat-offset encoder state machine.
- `validate_sequences()` round-trips the encoded sequences through the local
  decoder and falls back to a literals-only block if reconstruction does not
  match the original input.

---

## Compression ratio: current state

Compression now combines Huffman-coded literals with LZ77/FSE sequence coding.
That substantially improves text and repetitive-data compression versus the
previous literals-only path.

| Data type | Typical ratio |
|---|---|
| Single repeated byte (`'a' * 10 000`) | < 0.1% of original |
| Short English text (repetitive) | materially better than literals-only mode |
| Truly random bytes | ~100% (falls back to raw block) |

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
  error paths.  New features should add cases there.
- Run `cargo test` before every commit.
