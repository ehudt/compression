# Step 2 Design: Variable window sizes

## Goal

Allow the encoder to use window sizes larger than 128 KiB, controlled by
`window_log` from `MatchConfig`. The key work is: (a) make `MatchFinder`
dynamic on `window_log`/`chain_log`, (b) persist the `MatchFinder` across
blocks in a frame so cross-block matches are found, and (c) remove the
`min(17)` clamp in `frame.rs`.

## Design decisions

### Chain table size: `chain_log`, not `window_log`

Reference zstd keeps these separate: `chainLog` sizes the chain/BT table,
`windowLog` sizes the history window (max match offset). We do the same.
Chain array has `1 << chain_log` slots indexed by `pos & chain_mask`.
`find_match` limits candidate offsets to `window_size = 1 << window_log`.

Some high levels have `chain_log > window_log` (e.g. level 15: 23 vs 22,
level 19: 24 vs 23). This wastes a little memory for the greedy algorithm
but matches the reference table; Step 5 (BT) will use the extra size.

### Cross-block history via absolute positions

Currently `parse_ranges` creates a new `MatchFinder` per call and works with
block-local (0-based) positions. With windows larger than 128 KiB, a single
block cannot fill the window, so cross-block matches are needed.

Solution: make `MatchFinder` externally owned and passed by `&mut` reference.
Use **absolute positions** (offset from frame start) throughout:

- `parse_ranges(full_data, start, end, finder, sink)` — processes range
  `[start, end)` of `full_data`, using `finder` for history.
- Internally passes `&full_data[..end]` as `data` to `find_match` / `skip`;
  positions in `[0, end)` are valid indices, history from `[0, start)` is
  naturally accessible.
- `SequenceCollector.data = full_data` — `self.data[start..end]` correctly
  extracts literal bytes using absolute indices.
- `validate_sequences` receives `&full_data[start..end]` as the block slice.
- `frame.rs` creates `MatchFinder::new(&cfg)` once per frame, passes it to
  each `encode_block` call.

`parse` and `parse_with_sink` (used in tests) are updated to create their
own temporary `MatchFinder` and call `parse_ranges(data, 0, data.len(), ...)`.

### Offset code: direct computation, no table

`build_offset_code_table` in `block.rs` has `MAX_OFFSET = 128 KiB` hardcoded.
With `window_log=23` offsets can reach 8 MB; a table of that size would be
16 MB. The formula is trivial, so change `offset_code()` to compute directly:

```rust
fn offset_code(offset: usize) -> Option<(usize, u32)> {
    if offset == 0 { return None; }
    let raw = offset + 3;
    let code = usize::BITS as usize - 1 - raw.leading_zeros() as usize;
    if code >= OFFSET_DEFAULT_NORM.len() { return None; }
    Some((code, (raw - (1 << code)) as u32))
}
```

`OFFSET_DEFAULT_NORM` has 29 entries (codes 0-28). Max code 28 covers
`raw_offset` up to `2^29 - 1 ≈ 512 MB`, far beyond our max window.

## Changes

### `src/encoder/lz77.rs`

Remove `WINDOW_LOG` / `WINDOW_SIZE` constants.

Update `MatchFinder`:
```rust
pub struct MatchFinder {
    cfg: MatchConfig,
    hash_table: Vec<u32>,
    chain: Vec<u32>,      // size = 1 << cfg.chain_log
    chain_mask: usize,    // (1 << cfg.chain_log) - 1  [was window_mask]
    window_size: usize,   // 1 << cfg.window_log  [new field]
}
```

`MatchFinder::new(cfg: &MatchConfig)`:
- `chain` size = `1 << cfg.chain_log`
- `chain_mask = (1 << cfg.chain_log) - 1`
- `window_size = 1 << cfg.window_log`

`find_match(&mut self, data: &[u8], pos: usize) -> Option<Match>`:
- `max_offset = self.window_size`  (was `WINDOW_SIZE`)
- `data.len()` is now `end` (callers pass `&full_data[..end]`)

`insert` / `skip` / `find_match`:
- `pos & self.chain_mask`  (was `pos & self.window_mask`)

`parse_ranges` new signature:
```rust
pub fn parse_ranges(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
    sink: impl ParseSink,
)
```
Internally: `let data = &full_data[..end];` then loops `pos` from `start` to `end`.

`parse` / `parse_with_sink` — updated to create a temp `MatchFinder` and
delegate to `parse_ranges(data, 0, data.len(), &mut finder, ...)`.

### `src/encoder/block.rs`

`encode_block` new signature:
```rust
pub fn encode_block(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut lz77::MatchFinder,
    cfg: &super::MatchConfig,
) -> Result<Vec<u8>>
```

`collect_sequences` updated similarly. `SequenceCollector { data: full_data, ... }`.

`validate_sequences(original, literals, seq)` called with
`original = &full_data[start..end]`.

`offset_code()` changed to direct computation (remove `build_offset_code_table`
and its 128 KiB limit).

### `src/frame.rs`

- `MatchFinder` created once per frame: `let mut finder = MatchFinder::new(&cfg);`
- Each block calls `encode_block(input, pos, block_end, &mut finder, &cfg)?`
- Window descriptor: remove `min(17)` clamp — use `cfg.window_log` directly
- `MatchFinder` import added

## New tests

1. **Unit: window descriptor encoding** — verify `window_byte(log)` for each
   `window_log` in 10-23 matches `((log - 10) as u8) << 3`.

2. **Integration: cross-block round-trip** — compress 512 KiB of repetitive
   text at level 9 (`window_log=22`), decompress, verify equality. Input
   spans multiple blocks; level-9 window covers 4 MiB.

3. **Unit: cross-block history** — craft a short input where the first block
   contains a pattern and the second block references it. Verify a match
   event is produced spanning the boundary.

## Validation

- `cargo test` and `cargo test --test acceptance` pass.
- Acceptance tests confirm `zstd -d` can decompress our frames with
  `window_log > 17`.
- Ratio improvement expected at levels 9+ compared to pre-step baseline.
