# Step 6 Design: Optimal parsing (btopt/btultra/btultra2)

## Goal

Replace the lazy2 fallback for levels 16-19 with a cost-based optimal parser that
uses dynamic programming to find the globally optimal literal/match split for each
256-position chunk.

## Design decisions

### DP over 256-position chunks

The input is processed in `OPT_CHUNK_SIZE = 256` position chunks. For each chunk:
1. **Collection phase**: call `bt_find_insert` for every position to populate the BT
   and record the best match at each position.
2. **Forward DP**: fill `price[0..chunk_len+1]` where `price[i]` = minimum bit cost
   to encode positions `[pos, pos+i)`.
3. **Backtrack**: follow `from_len`/`from_off` pointers from `chunk_len` back to 0
   to recover the optimal sequence of literals and matches.
4. **Emit**: call sink with literals and matches in order.

All four BT strategies (BtOpt/BtUltra/BtUltra2) use the same DP — they differ only
in `search_depth` and `min_match`, so better parameters naturally yield better results.

### Cost model

Integer approximation in units of bits:

```
LITERAL_BITS = 8            // uniform 8-bit literal cost
SEQ_OVERHEAD = 16           // approximate FSE state bits per sequence
match_cost(offset, length)  = SEQ_OVERHEAD + offset_code_bits(offset) + match_length_extra_bits(length)
```

`offset_code_bits` reuses the existing function. `match_length_extra_bits` returns the
number of extra bits for the ML code (0 for lengths 3-34, 1 for 35-66, etc.).

The 16-bit sequence overhead captures that emitting a match requires FSE state
transitions even when extra bits are zero. This makes short matches with large offsets
less attractive, nudging the DP toward longer matches.

### Multiple match lengths per position

If the BT finds a match of length L at offset O, the DP considers all lengths
`[min_match, min(L, chunk_remaining)]` at that same offset. This is the key advantage
over greedy/lazy: the DP can choose a shorter version of a match if it enables a
better match immediately following.

### All positions inserted during collection phase

`bt_find_insert` inserts every position in the chunk, including those that will end up
inside a match in the optimal solution. This is correct and desirable:
- Every position becomes a valid BT candidate for future chunks/blocks.
- The insertion order mirrors the reference zstd forward pass.

### `lit_start` tracking across chunks

`lit_start` persists across chunks. Within each chunk, only match events explicitly
update `lit_start`. The final `if lit_start < end { sink.literals(...) }` handles any
trailing literal run.

### No new file

All code added to `src/encoder/lz77.rs`. The functions are private helpers; no public
interface changes.

## Changes

### `src/encoder/lz77.rs`

1. Add `OPT_CHUNK_SIZE: usize = 256` constant.
2. Add `match_length_extra_bits(length: usize) -> u32` (free function).
3. Add `match_cost_bits(offset: usize, length: usize) -> u32` (free function).
4. Add `parse_ranges_optimal(full_data, start, end, finder, sink)` (private).
5. Update `parse_ranges` dispatch: `BtOpt | BtUltra | BtUltra2` → `parse_ranges_optimal`.

## New tests

1. **Unit: optimal DP prefers longer match** — on data where a 1-literal deferral
   enables a much longer match, verify optimal picks the longer match.
2. **Unit: reconstruct original** — BtOpt parse must reconstruct input exactly.
3. **Integration: round-trip levels 16-19** — compress and decompress all 4 levels.
4. **Integration: btopt ratio >= btlazy2** — level 16 size ≤ level 15 size on
   compressible data (within tolerance).

## Validation

- `cargo test` and `cargo test --test acceptance` pass.
- All 22 levels round-trip correctly.
- No warnings.
