# Step 5 Design: Binary tree match finder

## Goal

Replace the hash-chain match finder with a DUBT (Dicho Unsigned Binary Tree) for
levels 13-15 (`BtLazy2`). The BT provides O(log N) bounded worst-case search with
better match quality than a chain for the same search depth budget.

## Design decisions

### Interleave left/right in existing `chain` array

Rather than adding a new field, the `chain` array is reallocated to double size for
BT strategies: `chain.len() = 2 * (1 << chain_log)`. Node at position `p` uses:
- `chain[2 * (p & chain_mask)]` — left child
- `chain[2 * (p & chain_mask) + 1]` — right child

This is exactly what reference zstd does (`chainTable[2*(idx&mask)]`). No new fields
needed. The existing `chain_mask = (1 << chain_log) - 1` is unchanged.

For non-BT strategies the array stays `1 << chain_log` entries. BtOpt/BtUltra/BtUltra2
(Step 6 candidates) also get the larger allocation now so Step 6 can use BT without
another reallocation.

### DUBT algorithm: insert and search simultaneously

`bt_find_insert(pos)` traverses from `hash_table[hash4(pos)]` (the current root) down
the tree, comparing `data[pos..]` against each candidate's suffix. At each step:
- Measure `common = match_length_full(data, cand, pos, max_len)` bytes in common.
- If `data[cand + common] >= data[pos + common]` (or pos's data ends first): cand is
  "larger" → link it into the building node's right (larger) subtree; continue down
  cand's left branch.
- Else: cand is "smaller" → link it into the building node's left subtree; continue
  down cand's right branch.

At the end, terminate dangling left/right pointers with `u32::MAX`.

The key invariant: we maintain two "write targets" (`smaller_write`, `larger_write`)
that are indices into the `chain` array where the next smaller/larger candidate should
be stored. They start at pos's own left/right slots and advance with each traversal
step.

### `match_length_full`: full comparison from byte 0

`match_length` (existing) starts at 4, assuming the caller verified the first 4 bytes
match. BT needs a comparison from byte 0 because candidates are found via hash (same
4-byte prefix) AND traversal (arbitrary suffix). We add:

```rust
fn match_length_full(data: &[u8], a: usize, b: usize, max_len: usize) -> usize {
    if max_len >= 4 && load_u32(data, a) == load_u32(data, b) {
        match_length(data, a, b, max_len)   // fast path: first 4 bytes match
    } else {
        // Byte-by-byte for the short prefix (at most 3 bytes in common).
        let mut len = 0;
        while len < max_len.min(3) && data[a + len] == data[b + len] { len += 1; }
        len
    }
}
```

This covers both the "same hash, extend" case (uses fast `match_length`) and the
"traversal comparison" case (short common prefix).

### Window validity check

If `pos - cand > chain_mask` the candidate's BT slot has been recycled; break.
If `pos - cand > window_size` the candidate is outside the match window; break.
Both are combined: `if cand >= pos || pos - cand > max_offset || pos - cand > chain_mask`.

### No interior match insertions

As with Fast/DFast, positions inside a match are not inserted into the BT. The ratio
impact is small because BT finds long matches anyway. Skipping interior insertions
avoids O(length × search_depth) overhead per match.

### `parse_ranges_bt_lazy2`

Identical control flow to `parse_ranges_lazy(max_lookahead=2)` but uses
`bt_find_insert` instead of `find_match`. After choosing `best_pos`, positions
`pos..=pos+lookahead_called` are already in the BT (inserted by bt_find_insert calls).
Interior match positions are not inserted.

### BtOpt / BtUltra / BtUltra2 still fall back to lazy2

These remain on `parse_ranges_lazy(max_lookahead=2)` until Step 6. Their `chain` is
now BT-sized (double), but the lazy2 path only uses `chain[pos & chain_mask]` (lower
half), so no overlap with the BT left/right slots.

## Changes

### `src/encoder/lz77.rs`

1. `MatchFinder::new`: size `chain` as `2 * (1 << chain_log)` for BT strategies.
   Update doc comment on `chain` field.
2. Add `match_length_full(data, a, b, max_len) -> usize` (free function).
3. Add `bt_find_insert(&mut self, data, pos) -> Option<Match>` to `MatchFinder`.
4. Add `parse_ranges_bt_lazy2(full_data, start, end, finder, sink)` (private).
5. `parse_ranges`: dispatch `BtLazy2` → `parse_ranges_bt_lazy2`.

## New tests

1. **Unit: BT finds same match as greedy on simple repetitive data** — verify BT
   returns a valid match (not necessarily longer; just correct).
2. **Unit: BT respects window bounds** — insert positions outside `window_size`; verify
   they are not returned as match candidates.
3. **Unit: BT slot cycling** — insert more positions than `chain_mask`; verify no
   stale/cycled slots are returned.
4. **Integration: round-trip levels 13-15** — compress and decompress; assert equality.
5. **Integration: BtLazy2 ratio >= lazy2** — on repetitive text, level 13 compressed
   size ≤ level 12 compressed size.

## Validation

- `cargo test` and `cargo test --test acceptance` pass.
- All 22 levels round-trip correctly.
- Level 13 compressed size ≤ level 12 on compressible data (BT better than chain).
