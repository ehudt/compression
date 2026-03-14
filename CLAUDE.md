# CLAUDE.md — Claude Code guide for zstd_rs

This file gives Claude Code-focused guidance. For cross-agent guidance shared with Codex, see `AGENTS.md`.

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
cargo bench              # Criterion benchmarks (speed + ratio)
cargo run --example basic       # demo
cargo run --bin zstd_rs -- compress 3 input.txt out.zst
cargo run --bin zstd_rs -- decompress out.zst result.txt
```

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

## FSE (Finite State Entropy) — what it is and what is missing

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

### What is missing

**The FSE sequence *encoder*.**  `encoder/block.rs:encode_block` currently
emits 0 sequences and puts the entire block into the literals section.  The
LZ77 events from `encoder/lz77.rs:parse()` are computed but discarded.

To implement the sequence encoder you need to:

1. Call `encoder/lz77.rs:parse(data, cfg)` to get `Vec<Event>`.
2. Collect `(lit_len, match_len, offset)` triples and the literal bytes.
3. Map each triple to `(ll_code, ll_extra, ml_code, ml_extra, of_code,
   of_extra)` using the lookup tables in `tables/sequences.rs`.
4. Build FSE encode tables from the predefined norms (already in
   `build_encode_table` in `fse.rs`).
5. Encode the bitstream **in reverse** (last sequence first):
   - For each sequence (reverse order):
     a. Write `ll_extra_bits` bits of `ll_extra`
     b. Write `ml_extra_bits` bits of `ml_extra`
     c. Write `of_code` bits of `of_extra`
     d. If not the last sequence to encode (i.e., not the first in the
        original order): advance encode states for LL, ML, OF and write
        the state-advance bits.
   - Write initial OF, ML, LL states (each `accuracy_log` bits).
6. `BitWriter::finish()` reverses the bytes, producing the correct stream.
7. Write mode byte `0x00` (all predefined tables) and sequence count prefix.

The `decoder/sequences.rs:decode_sequence_bitstream` reads bits in this
exact order and can serve as the ground-truth spec for the encoder.

**Key pitfall:** `build_encode_table` returns entries with `delta_find_state`.
The correct encode transition is:
```
nb = accuracy_log - floor(log2(n))          // n = norm count for symbol
if state >= find_state_min:
    nb -= 1                                  // sometimes one fewer bit
output (state - find_state_min) bits
new_encoder_state = delta_find_state + (state >> nb) + find_state_min_of_next_cell
```
Cross-check against `decoder/sequences.rs` on every test case.

---

## Compression ratio: current state

Because the sequence encoder emits 0 sequences, compression relies entirely
on Huffman entropy coding of the raw bytes.

| Data type | Typical ratio |
|---|---|
| Single repeated byte (`'a' * 10 000`) | < 0.1% of original |
| Short English text (repetitive) | 60–80% (no LZ match bonus) |
| Truly random bytes | ~100% (falls back to raw block) |

With a working FSE sequence encoder, English text should reach 25–40%.

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
