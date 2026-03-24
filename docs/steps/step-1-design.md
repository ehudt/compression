# Step 1 Design: Parameter table and strategy enum

## Goal

Replace the flat `MatchConfig` with a richer struct mirroring zstd's per-level
parameters, and introduce a `Strategy` enum. Wire the new config through the
encoder without changing any algorithms — all levels continue to use the
existing greedy hash-chain, but the plumbing is ready for Steps 2-6.

## Changes

### `src/encoder/lz77.rs`

**New `Strategy` enum** (added before `MatchConfig`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Fast, DFast, Greedy, Lazy, Lazy2, BtLazy2, BtOpt, BtUltra, BtUltra2,
}
```

**Expanded `MatchConfig`** — replaces `search_depth` with `search_log`, adds
five new fields:

```rust
pub struct MatchConfig {
    pub window_log: usize,    // log2(window size), controls history
    pub chain_log: usize,     // log2(chain/BT table size)
    pub hash_log: usize,      // log2(hash table size)
    pub search_log: usize,    // log2(max search attempts per position)
    pub min_match: usize,     // minimum match length to emit
    pub target_length: usize, // early-accept threshold (0 = disabled)
    pub strategy: Strategy,   // algorithm family
    pub max_match: usize,     // kept; zstd max is 131_074
}

impl MatchConfig {
    /// Derived: actual search attempt count = 1 << search_log.
    pub fn search_depth(&self) -> usize { 1 << self.search_log }
}
```

`Default` impl uses level-5 (greedy) parameters so existing tests that
construct `MatchConfig::default()` still produce valid output.

**`for_level()` rewrite** — maps levels 1-19 to reference table values;
levels 20-22 use level 19's values:

| Level | WLog | CLog | HLog | SLog | MML | TLen | Strategy |
|-------|------|------|------|------|-----|------|----------|
| 1     | 19   | 13   | 14   | 1    | 7   | 0    | Fast     |
| 2     | 20   | 15   | 16   | 1    | 6   | 0    | Fast     |
| 3     | 21   | 16   | 17   | 1    | 5   | 0    | DFast    |
| 4     | 21   | 18   | 18   | 1    | 5   | 0    | DFast    |
| 5     | 21   | 18   | 19   | 3    | 5   | 2    | Greedy   |
| 6     | 21   | 18   | 19   | 3    | 5   | 4    | Lazy     |
| 7     | 21   | 19   | 20   | 4    | 5   | 8    | Lazy     |
| 8     | 21   | 19   | 20   | 4    | 5   | 16   | Lazy2    |
| 9     | 22   | 20   | 21   | 4    | 5   | 16   | Lazy2    |
| 10    | 22   | 21   | 22   | 5    | 5   | 16   | Lazy2    |
| 11    | 22   | 21   | 22   | 6    | 5   | 16   | Lazy2    |
| 12    | 22   | 22   | 23   | 6    | 5   | 32   | Lazy2    |
| 13    | 22   | 22   | 22   | 4    | 5   | 32   | BtLazy2  |
| 14    | 22   | 22   | 23   | 5    | 5   | 32   | BtLazy2  |
| 15    | 22   | 23   | 23   | 6    | 5   | 32   | BtLazy2  |
| 16    | 22   | 22   | 22   | 5    | 5   | 48   | BtOpt    |
| 17    | 23   | 23   | 22   | 5    | 4   | 64   | BtOpt    |
| 18    | 23   | 23   | 22   | 6    | 3   | 64   | BtUltra  |
| 19    | 23   | 24   | 22   | 7    | 3   | 256  | BtUltra2 |

**`MatchFinder::find_match()`** — replace `self.cfg.search_depth` with
`1 << self.cfg.search_log` (or call `self.cfg.search_depth()`).

**Unit tests** — `#[cfg(test)]` block in `lz77.rs` gains:
- `test_for_level_params`: checks every level 1-19 returns expected
  window_log, chain_log, hash_log, search_log, min_match, target_length.
- `test_for_level_strategy`: checks every level maps to the expected Strategy.
- `test_for_level_clamp`: levels 20-22 return same params as level 19.

### `src/encoder/mod.rs`

Re-export `Strategy` alongside `MatchConfig`:
```rust
pub use lz77::{MatchConfig, Strategy};
```

### `src/frame.rs`

**Window descriptor** — replace hardcoded `56u8` with computed byte, but
clamp `window_log` to 17 so behavior is unchanged for all current levels.
After Step 2 removes the clamp, larger windows will work automatically.

```rust
// Clamp to 17 until Step 2 enables variable window sizes.
let window_log = cfg.window_log.min(17);
// zstd window descriptor: exponent = window_log - 10, mantissa = 0
// byte = (exponent << 3) | mantissa
let window_byte = ((window_log - 10) as u8) << 3;
out.push(window_byte);
```

**`looks_incompressible()`** — replace `cfg.search_depth > 8` with
`cfg.search_depth() > 8` (trivial rename).

## Design decisions

- **Keep `MatchConfig` name**: renaming to `CompressionParams` would touch
  many call sites including the public API. Rename can happen later if needed.
- **Remove `search_depth` field, add `search_depth()` method**: the field
  is derived (`1 << search_log`) so storing it would be redundant and
  could get out of sync.
- **`Default` uses level-5 parameters**: existing tests that call
  `MatchConfig::default()` get greedy strategy with reasonable parameters.
  Level 5 is the first level using the greedy algorithm that all the current
  code implements.
- **Hash table size at high levels**: level 12 uses `hash_log=23` (8M
  entries, 32 MB). This matches the reference but is large. Acceptable for
  now; we can add memory caps later.
- **No behavior change for `strategy` field yet**: all parse paths still use
  the greedy hash-chain. The `strategy` field is stored but ignored in the
  dispatcher until Steps 3-6.

## Validation

- `cargo test` — all tests pass (round-trip correctness, not byte-identical).
- `cargo test --test acceptance` — passes (still produces valid zstd frames).
- No new warnings.
- New unit tests cover every level 1-19 parameter and strategy.
