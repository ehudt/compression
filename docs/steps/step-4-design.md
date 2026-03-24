# Step 4 Design: Fast and double-fast strategies

## Goal

Implement `Fast` (levels 1-2) and `DFast` (levels 3-4): simpler, faster match finders
that avoid chain walking entirely. Both prioritize throughput over ratio.

## Design decisions

### No separate struct — reuse `MatchFinder` fields

Rather than introducing a new match-finder type, we reuse the existing arrays:
- **Fast**: uses `hash_table` only; `chain` is allocated but unused.
- **DFast**: uses `hash_table` for 4-byte keys (short matches); repurposes `chain`
  as a **second hash table** indexed by a 8-byte hash. `chain_log` already controls
  its size — exactly what the reference zstd uses for the "long" table.

This avoids adding new fields or abstractions. The key invariant: for Fast/DFast, no
code ever uses `chain` as a chain (no `chain[pos & chain_mask] = prev` links), so the
two uses are exclusive.

### Skip logic: anchor-relative step

On a miss, advancing by 1 would match reference throughput. The reference zstd does
better: `step = 1 + (pos - anchor) >> search_log`, where `anchor` is the start of
the current literal run (last match end). This means:
- Immediately after a match: step = 1 (search every position).
- After many misses: step grows geometrically, skipping large stretches quickly.

We implement this: `lit_start` serves as the anchor (it equals the start of the
current unmatched run). Skipped positions are **not** inserted into any table —
this trades slightly lower ratio for speed, which is the intent of fast strategy.

### `hash8`: 8-byte key for DFast long table

Uses a 64-bit multiplier and shifts to `chain_log` bits:
```rust
const HASH_PRIME_64: u64 = 0x9E3779B97F4A7C15;

fn hash8(&self, data: &[u8], pos: usize) -> usize {
    let bytes = u64::from_le_bytes(data[pos..pos+8].try_into().unwrap());
    (bytes.wrapping_mul(HASH_PRIME_64) >> (64 - self.cfg.chain_log)) as usize
}
```

The index directly into `chain[h]` (no masking needed since `chain.len() = 1 << chain_log`
and the hash already produces exactly `chain_log` bits).

### DFast lookup order: long table first

1. If `pos + 8 <= data.len()`: look up `chain[hash8(pos)]`. On hit, update `chain[h]`
   and `hash_table[hash4(pos)]`, return match if `length >= min_match`.
2. Fall through to `hash_table[hash4(pos)]`. Update it. Return match if `length >= min_match`.

Both tables are always updated regardless of match outcome (no stale entries).

### No interior insertions in matches

Positions inside a match are not inserted. Ratio may be slightly lower than greedy
for short inputs that would benefit from those entries, but the speed gain is the goal.
Cross-block history remains functional (the start of each match/literal is inserted).

## Changes

### `src/encoder/lz77.rs`

1. Add `HASH_PRIME_64` constant.
2. Add `hash8` method to `MatchFinder`.
3. Add `lookup_fast(data, pos) -> Option<Match>`: single `hash_table` lookup, inserts.
4. Add `lookup_dfast(data, pos) -> Option<Match>`: tries `chain` (long), then
   `hash_table` (short); updates both.
5. Add `parse_ranges_fast(full_data, start, end, finder, sink)`.
6. Add `parse_ranges_dfast(full_data, start, end, finder, sink)`.
7. Update `parse_ranges` dispatch: Fast → `parse_ranges_fast`, DFast → `parse_ranges_dfast`.

No changes to `block.rs`, `frame.rs`, or `mod.rs`.

## New tests

1. **Unit: `lookup_fast` single lookup** — verify no chain is walked; two positions with
   the same 4-byte hash, second lookup returns match from the first.
2. **Unit: `lookup_dfast` long table priority** — insert an 8-byte-matching position, then
   a different 4-byte-only match; verify dfast returns the 8-byte match.
3. **Integration: round-trip levels 1-4** — on varied inputs (repetitive, random,
   sequential); already partially covered by existing tests.
4. **Integration: fast levels compress correctly** — level 1 on 64 KB repetitive corpus
   produces smaller output than raw input.

## Validation

- `cargo test` and `cargo test --test acceptance` pass.
- All levels 1-22 round-trip correctly (extend existing level sweep tests).
- Level 1 compressed size < level 5 compressed size for highly compressible data
  (fast has higher min_match = 7, so fewer but longer matches — ratio may differ).
- No `cargo test` warnings.
