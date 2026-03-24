# Step 3 Design: Lazy and lazy2 matching

## Goal

Implement lazy match evaluation for levels 6-12 (strategies `Lazy` and `Lazy2`).
At each position, instead of immediately accepting the first match found (greedy),
the encoder checks whether deferring the match by 1 (lazy) or 1-2 (lazy2) positions
yields a better result. The skipped positions become literals.

## Design decisions

### Dispatch in `parse_ranges`

`parse_ranges` now dispatches on `finder.cfg.strategy`:
- `Greedy | Fast | DFast` → current greedy loop (extracted into `parse_ranges_greedy`)
- `Lazy` → new `parse_ranges_lazy(..., max_lookahead=1)`
- `Lazy2 | BtLazy2 | BtOpt | BtUltra | BtUltra2` → `parse_ranges_lazy(..., max_lookahead=2)`

BtLazy2 through BtUltra2 fall back to lazy2 for now — steps 5-6 will replace them.

### Match preference heuristic

Reference zstd uses: prefer new match if the length gain (×4) outweighs the extra
offset cost. The offset cost is measured in offset-code bits:

```rust
fn prefer_match(new: Match, old: Match) -> bool {
    let new_bits = offset_code_bits(new.offset);
    let old_bits = offset_code_bits(old.offset);
    // Prefer new if length gain (×4) exceeds additional offset bit cost.
    4 * new.length as i32 > 4 * old.length as i32 + (new_bits as i32 - old_bits as i32)
}

fn offset_code_bits(offset: usize) -> usize {
    let raw = offset + 3;
    (usize::BITS - raw.leading_zeros()) as usize  // = floor(log2(raw)) + 1
}
```

This means: a +1 length gain justifies at most 4 extra offset bits; a shorter offset can
compensate for a slight length disadvantage.

### Avoiding double-insert self-loops

`find_match` always inserts the position into the hash table before searching. In lazy
mode, we call `find_match(pos)`, `find_match(pos+1)`, and (for lazy2) `find_match(pos+2)`.
All these positions get inserted.

After choosing `best_pos`, the match covers `best_pos .. best_pos+best.length`. We must
insert/skip positions in the match interior that weren't already inserted by lookahead:

```
already inserted: pos .. pos+lookahead_called
match interior:   best_pos+1 .. best_pos+best.length-1

skip_start = pos + lookahead_called + 1
skip_len   = best_pos + best.length - skip_start  (skip if > 0)
```

This avoids re-inserting positions already covered by lookahead find_match calls, which
would create self-loops in the chain array (chain[x & mask] = x → wastes search depth).

### `target_length` early-accept

When `target_length > 0` and the current match length ≥ `target_length`, accept
immediately without any lookahead. Also break out of the lookahead loop early if a
lookahead match exceeds `target_length`.

## Algorithm: `parse_ranges_lazy`

```
pos = start, lit_start = start
while pos < end:
    if pos+4 > end: break

    m0 = find_match(data, pos)   // inserts pos
    if m0 is None: pos += 1; continue

    if target_length > 0 && m0.length >= target_length:
        // Early accept — no lookahead
        emit literals(lit_start, pos)
        emit match(pos, m0.offset, m0.length)
        skip(data, pos+1, m0.length-1)
        pos += m0.length; lit_start = pos; continue

    best = m0, best_pos = pos, lookahead_called = 0

    for la in 1..=max_lookahead:
        la_pos = pos + la
        if la_pos+4 > end: break
        m = find_match(data, la_pos)   // inserts la_pos
        lookahead_called = la
        if m is Some:
            if target_length > 0 && m.length >= target_length:
                best = m, best_pos = la_pos; break  // early accept from lookahead
            if prefer_match(m, best):
                best = m, best_pos = la_pos

    emit literals(lit_start, best_pos)
    emit match(best_pos, best.offset, best.length)
    skip_start = pos + lookahead_called + 1
    match_end = best_pos + best.length
    if skip_start < match_end:
        skip(data, skip_start, match_end - skip_start)
    pos = match_end; lit_start = pos

emit literals(lit_start, end)
```

## Changes

### `src/encoder/lz77.rs`

1. Extract greedy loop from `parse_ranges` into `parse_ranges_greedy` (private).
2. Add `offset_code_bits(offset: usize) -> usize` (private).
3. Add `prefer_match(new: Match, old: Match) -> bool` (private).
4. Add `parse_ranges_lazy(full_data, start, end, finder, sink, max_lookahead: usize)` (private).
5. Replace `parse_ranges` body with a strategy dispatch to `parse_ranges_greedy` or
   `parse_ranges_lazy`.

No changes to `block.rs`, `frame.rs`, or `mod.rs`.

## New tests

1. **Unit: `prefer_match` heuristic** — verify correct preference in three cases:
   - Same length, smaller offset → prefer new.
   - Longer match, same offset → prefer new.
   - Slightly longer match with much larger offset → prefer old.

2. **Unit: lazy picks better match** — craft a short input where greedy emits a short
   match while lazy finds a longer one at pos+1. Verify lazy produces fewer sequences.

3. **Unit: `target_length` early-accept** — with a very low target_length, verify the
   encoder doesn't do lookahead for matches >= target_length (count `find_match` calls
   via event count comparison or parse with a tiny target_length and known input).

4. **Integration: round-trip levels 6, 8, 12** — compress and decompress, assert equality.

5. **Integration: ratio improves from level 5 to level 8** — on repetitive text, verify
   level-8 (lazy2) compressed size < level-5 (greedy) compressed size.

## Validation

- `cargo test` and `cargo test --test acceptance` pass.
- Level 8 compressed size < level 5 compressed size on compressible input.
- All 19 levels round-trip correctly.
