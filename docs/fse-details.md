# FSE (Finite State Entropy) details

FSE (also called tANS -- table-based Asymmetric Numeral Systems) is a
near-optimal entropy coder.  Zstd uses it instead of Huffman coding in
the **sequences section** of each compressed block.

---

## How FSE works

A sequence is a triple `(literal_length, match_length, offset)` produced by
the LZ77 pass.  Zstd uses FSE to entropy-code the three symbol streams
(LL codes, ML codes, OF codes) jointly into a single backward bitstream.

State machine:

- **Decode table**: maps each state in `[0, table_size)` to
  `(symbol, num_bits, base_line)`.  To decode: read `num_bits` bits,
  new state = `base_line + bits_read`.
- **Encode table**: maps each symbol to `(delta_find_state, num_bits,
  find_state_min)`.  To encode symbol S from state s: output the lower
  `num_bits` bits of s, new state = `(s >> num_bits) + delta_find_state`.
- Both tables derive from the same **normalized probability distribution**
  (stored in block headers or taken from predefined defaults).

The sequence bitstream is written **in reverse** -- the encoder processes
sequences last-to-first; the decoder reads forward after the sentinel bit.

---

## Implementation in this repo

- `fse.rs`: `normalize_counts`, `build_decode_table`, `build_encode_table`,
  `read_distribution_table`, `BitReader`, `BitWriter`.
- `tables/sequences.rs`: predefined FSE norms for LL / ML / OF
  (`LITERALS_LENGTH_DEFAULT_NORM`, `MATCH_LENGTH_DEFAULT_NORM`,
  `OFFSET_DEFAULT_NORM`) and extra-bits lookup tables.
- `decoder/sequences.rs`: full FSE decoder -- state initialization,
  per-sequence state advance, extra-bit decoding, repeat-offset handling,
  sequence execution.

---

## Current encoder behavior

`encoder/block.rs:encode_block` emits real sequences:

1. `encoder/lz77.rs:parse(data, cfg)` produces literal runs and matches.
2. `collect_sequences()` separates literal bytes from sequence commands.
3. Lengths/offsets are mapped to `(ll_code, ml_code, of_code)` plus extra
   bits via tables in `tables/sequences.rs`.
4. `encode_sequences()` writes: sequence count, mode byte `0x00` (predefined
   tables), initial LL/OF/ML states, then per-sequence offset/match-length/
   literal-length extra bits and FSE state transitions for all but the last.
5. `BitWriter::finish()` reverses bytes for the backwards bitstream convention.

The encoder derives state paths by inverting predefined decode tables
(`build_state_path()` + `inverse_transition()`) rather than maintaining
separate FSE encoder tables.

**Current constraints:**

- Offsets always use the non-repeat path: `raw_offset = offset + 3`
  (simpler than modeling zstd's repeat-offset state machine).
- `validate_sequences()` round-trips encoded sequences through the local
  decoder; falls back to literals-only block on mismatch.
