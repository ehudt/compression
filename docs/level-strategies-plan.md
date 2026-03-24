# Plan: Implement Reference zstd Compression Strategies

This plan brings zstd_rs's encoder in line with the reference zstd implementation's
per-level compression strategies. Today, zstd_rs uses a single greedy hash-chain
match finder at all levels, varying only `search_depth` and `hash_log`. The
reference implementation uses 9 distinct strategies across levels 1-22, with
dramatically different algorithms, data structures, and parameter spaces.

Each step below is self-contained enough that an agent can design and implement
it independently, given the prior steps are complete.

## How to execute this plan

**Work one step at a time: design in detail → implement → validate → then
design the next step.** Do not plan all steps in detail upfront.

Reasons:

- Later steps depend on design choices made in earlier steps. The parsing
  interface that Step 1 establishes shapes how Steps 3-6 integrate. Planning
  Step 5 in detail before Step 3 is implemented means guessing at interfaces
  that don't exist yet.
- Each step changes the codebase enough to invalidate detailed plans for later
  steps. If Step 2 reveals that `MatchFinder` needs restructuring for
  cross-block history, that ripples into Steps 3-6.
- Benchmarking results between steps inform priorities. If Step 3 (lazy
  matching) delivers most of the ratio gain, Step 6 (optimal parsing) may not
  justify its complexity. You can't know until you measure.
- Each step is a natural commit and validation boundary. All tests must pass,
  acceptance tests confirm interop, and benchmarks show expected behavior
  before moving to the next step.

**Per-step workflow:**

1. Read this plan's description for the current step.
2. Read the relevant source files and reference zstd sources cited.
3. Create a detailed design and implementation plan for that step only. Write
   it to `docs/steps/step-N-design.md` (e.g., `docs/steps/step-1-design.md`).
4. Implement, test, benchmark, commit.
5. Update this document: mark the step as done (add `[DONE]` to the step
   heading), note any design decisions that affect later steps, and adjust
   subsequent step descriptions if needed.
6. Move to the next step.

## Plan management

**Sub-plans**: Each step's detailed design goes in `docs/steps/step-N-design.md`.
These are created just-in-time when starting a step, not upfront. Include:
specific function signatures, data structure layouts, module organization, and
any design decisions resolved during the design phase.

**Progress tracking**: Mark completed steps by adding `[DONE]` to the step
heading in this file (e.g., `## Step 1: Parameter table and strategy enum [DONE]`).
Add a brief summary below the heading noting what was actually done, any
deviations from the original description, and design decisions that affect
later steps. Keep sub-plan files after completion — they serve as documentation.

**Commit discipline**: Each step should result in one or more commits. The final
commit for a step must leave `cargo test` and `cargo test --test acceptance`
passing.

## Reference: zstd level parameter table (>256KB inputs)

From [`lib/compress/clevels.h`](https://github.com/facebook/zstd/blob/dev/lib/compress/clevels.h):

| Level | WLog | CLog | HLog | SLog | MML | TLen | Strategy |
|-------|------|------|------|------|-----|------|----------|
| 1     | 19   | 13   | 14   | 1    | 7   | 0    | fast     |
| 2     | 20   | 15   | 16   | 1    | 6   | 0    | fast     |
| 3     | 21   | 16   | 17   | 1    | 5   | 0    | dfast    |
| 4     | 21   | 18   | 18   | 1    | 5   | 0    | dfast    |
| 5     | 21   | 18   | 19   | 3    | 5   | 2    | greedy   |
| 6     | 21   | 18   | 19   | 3    | 5   | 4    | lazy     |
| 7     | 21   | 19   | 20   | 4    | 5   | 8    | lazy     |
| 8     | 21   | 19   | 20   | 4    | 5   | 16   | lazy2    |
| 9     | 22   | 20   | 21   | 4    | 5   | 16   | lazy2    |
| 10    | 22   | 21   | 22   | 5    | 5   | 16   | lazy2    |
| 11    | 22   | 21   | 22   | 6    | 5   | 16   | lazy2    |
| 12    | 22   | 22   | 23   | 6    | 5   | 32   | lazy2    |
| 13    | 22   | 22   | 22   | 4    | 5   | 32   | btlazy2  |
| 14    | 22   | 22   | 23   | 5    | 5   | 32   | btlazy2  |
| 15    | 22   | 23   | 23   | 6    | 5   | 32   | btlazy2  |
| 16    | 22   | 22   | 22   | 5    | 5   | 48   | btopt    |
| 17    | 23   | 23   | 22   | 5    | 4   | 64   | btopt    |
| 18    | 23   | 23   | 22   | 6    | 3   | 64   | btultra  |
| 19    | 23   | 24   | 22   | 7    | 3   | 256  | btultra2 |

Key: WLog = windowLog, CLog = chainLog, HLog = hashLog, SLog = searchLog,
MML = minMatch, TLen = targetLength.

## Current state (what we have)

- **MatchConfig** (`src/encoder/lz77.rs:18-27`): 4 fields — `min_match`,
  `max_match`, `search_depth`, `hash_log`. `for_level()` maps to 4 tiers
  (levels 1-3, 4-7, 8-12, 13-22) varying only `search_depth` (8→512) and
  `hash_log` (14→20).
- **Match finder**: Single greedy hash-chain algorithm. Hash table (`Vec<u32>`)
  + chain array (`Vec<u32>`, size = WINDOW_SIZE). No lazy evaluation.
- **Window size**: Hardcoded `WINDOW_LOG = 17` (128 KiB) in `lz77.rs:74-75`.
  Frame header emits a fixed byte `56u8` in `frame.rs:54`.
- **Block size**: `MAX_BLOCK_SIZE = 128 KiB` in `frame.rs:21`.
- **Data flow**: `MatchConfig` → `MatchFinder::find_match()` → `Event` enum
  (Literals/Match) → `SequenceCollector` → `EncodedSequence` → FSE/Huffman
  encoding in `block.rs`.

---

## Step 1: Parameter table and strategy enum [DONE]

**Summary**: Added `Strategy` enum (Fast through BtUltra2) and expanded
`MatchConfig` with `window_log`, `chain_log`, `search_log`, `target_length`,
and `strategy` fields. Removed the `search_depth` field in favor of a derived
`search_depth()` method (`1 << search_log`). Rewrote `for_level()` to return
reference table values for levels 1-19; levels 20-22 use level-19 values.
Updated `frame.rs` to compute the window descriptor byte from `cfg.window_log`
(clamped to 17 until Step 2). `Strategy` is re-exported from `encoder/mod.rs`.
Added 3 unit tests covering all parameter fields and strategies for levels
1-19, plus the 20-22 clamp behavior. Design doc at `docs/steps/step-1-design.md`.

**Design decisions affecting later steps**:
- `MatchConfig` name kept (not renamed to `CompressionParams`).
- `Default` now returns level-5 (Greedy) parameters.
- `frame.rs` window clamp is at `cfg.window_log.min(17)` — Step 2 removes `min(17)`.

**Goal**: Replace the flat `MatchConfig` with a richer configuration struct that
mirrors zstd's per-level parameters, and introduce a `Strategy` enum. Wire the
new config through the encoder without changing any algorithms yet — all levels
continue to use the existing greedy hash-chain, but the plumbing is ready.

**What to do**:

1. Define a `Strategy` enum in `src/encoder/mod.rs` (or a new
   `src/encoder/strategy.rs`):
   ```
   Fast, DFast, Greedy, Lazy, Lazy2, BtLazy2, BtOpt, BtUltra, BtUltra2
   ```

2. Expand `MatchConfig` (or replace it with `CompressionParams`) to include:
   - `window_log: usize`
   - `chain_log: usize`
   - `hash_log: usize`
   - `search_log: usize` (log2 of max search attempts)
   - `min_match: usize`
   - `target_length: usize`
   - `strategy: Strategy`

3. Rewrite `for_level()` to return parameters matching the reference table above
   for levels 1-19. Levels 20-22 can use level 19's values for now.

4. All call sites that consume `MatchConfig` should accept the new struct. For
   now, they can ignore the new fields and use only the ones that map to current
   behavior (`hash_log`, `search_depth` derived from `search_log`, `min_match`).

5. Update `frame.rs` to read `window_log` from the config instead of hardcoding
   `56u8`, but **clamp it to 17 for now** so behavior doesn't change yet. Add a
   comment marking where Step 2 will uncap it.

**Files touched**: `src/encoder/lz77.rs`, `src/encoder/mod.rs`,
`src/encoder/block.rs`, `src/frame.rs`.

**New tests required**: Unit tests for `for_level()` verifying that each level
1-19 returns the expected parameters from the reference table. Test that the
`Strategy` enum round-trips correctly and that all levels map to the correct
strategy.

**Validation**:
- `cargo test` passes with no behavior change (byte-identical output at all levels).
- `cargo test --test acceptance` passes.
- No new warnings.

**Reference**: [`lib/compress/clevels.h`](https://github.com/facebook/zstd/blob/dev/lib/compress/clevels.h),
[`lib/zstd.h` strategy enum](https://github.com/facebook/zstd/blob/dev/lib/zstd.h).

---

## Step 2: Variable window sizes [DONE]

**Summary**: `MatchFinder` is now dynamically sized from `window_log` and
`chain_log`. Removed `WINDOW_LOG`/`WINDOW_SIZE` constants; `window_size` and
`chain_mask` are per-instance fields. `MatchFinder` is created once per frame
and passed by `&mut` reference to `encode_block` and `parse_ranges`, enabling
cross-block match history. `encode_block` signature changed to
`(full_data, start, end, finder)`. `parse_ranges` similarly takes absolute
start/end positions into `full_data`. Window descriptor byte in `frame.rs` now
uses `cfg.window_log` directly (no `min(17)` clamp). `offset_code()` changed to
direct computation (no lookup table), removing the hardcoded 128 KiB offset
limit. Debug validation (`validate_sequences`) seeded with prior window history
so cross-block match offsets are correctly verified. Design doc at
`docs/steps/step-2-design.md`.

**Design decisions affecting later steps**:
- `encode_block` no longer takes `cfg` — the finder carries config. If later
  steps need cfg directly in encode_block, pass it back or expose via finder.
- `MatchFinder::window_size()` public method exposed for validation use.
- `chain` is sized by `chain_log` (not `window_log`); at levels 15 and 19
  `chain_log > window_log`, so the chain array is slightly oversized for the
  greedy algorithm — this is correct and intentional (Step 5 BT will use it).

**Goal**: Allow the encoder to use window sizes larger than 128 KiB, controlled
by the `window_log` parameter from Step 1.

**What to do**:

1. Remove the hardcoded `WINDOW_LOG = 17` / `WINDOW_SIZE = 1 << 17` constants
   in `lz77.rs`. Make `MatchFinder` accept `window_log` at construction time
   and size its chain array accordingly.

2. Size the hash table from `hash_log` (already parameterized) and the chain
   table from `window_log` (or `chain_log` — in reference zstd, `chainLog`
   controls the chain table size while `windowLog` controls the history window;
   decide whether to unify or separate these).

3. Update `frame.rs` to encode the correct window descriptor byte based on
   `window_log`. The zstd frame format encodes window size as
   `(1 << (exponent + 10)) * (1 + mantissa/8)` in a single byte. Implement
   this calculation. Reference: `docs/zstd-format.md` in this repo, and
   [RFC 8878 Section 3.1.1.1.2](https://www.rfc-editor.org/rfc/rfc8878#section-3.1.1.1.2).

4. `MAX_BLOCK_SIZE` stays at 128 KiB (the zstd spec caps block content at
   128 KiB regardless of window size). Verify that block splitting still works
   correctly when window > block size — the match finder needs to keep history
   across blocks within the window.

5. Consider memory usage: window_log=23 means 8 MiB chain table of `u32` =
   32 MiB. This is fine for a compressor but worth documenting.

**Key design decision**: How to handle cross-block match history. Currently,
`MatchFinder` is likely reset per block. With larger windows, the match finder
must persist across blocks within a frame, maintaining a sliding window of
history. Study how the current `parse_ranges()` flow works and whether
`MatchFinder` already persists or needs restructuring.

**Files touched**: `src/encoder/lz77.rs`, `src/frame.rs`.

**New tests required**:
- Unit test for window descriptor encoding: verify the byte produced for each
  `window_log` value (10-23) matches what the zstd spec expects.
- Integration test: round-trip compress/decompress at a level with
  `window_log > 17` (e.g., level 9, `window_log=22`) using input larger than
  128 KiB to exercise cross-block match history.
- If cross-block history required `MatchFinder` restructuring, add a test that
  verifies matches span block boundaries.

**Validation**:
- `cargo test` and `cargo test --test acceptance` pass.
- Acceptance tests confirm system `zstd -d` can decompress frames with
  `window_log > 17`.
- Weighted benchmark at levels 9+ shows ratio improvement >= 1% on
  compressible data (text, source, xml categories) compared to pre-step
  baseline, due to larger match windows.
- No throughput regression > 5% at any level on the weighted benchmark.

**Reference**: [RFC 8878 Section 3.1.1.1.2](https://www.rfc-editor.org/rfc/rfc8878#section-3.1.1.1.2),
[`lib/compress/zstd_compress_internal.h`](https://github.com/facebook/zstd/blob/dev/lib/compress/zstd_compress_internal.h).

---

## Step 3: Lazy and lazy2 matching [DONE]

**Summary**: `parse_ranges` now dispatches on `strategy`: greedy (extracted to
`parse_ranges_greedy`) for Fast/DFast/Greedy; `parse_ranges_lazy` with
`max_lookahead=1` for Lazy; `parse_ranges_lazy` with `max_lookahead=2` for Lazy2
and all BT strategies (temporary fallback until Step 5). The lazy algorithm calls
`find_match` at lookahead positions (which inserts them), picks the best match via
the `prefer_match` heuristic (`4×length_gain > offset_bit_delta`), and skips only
uninserted positions to avoid chain self-loops. `target_length` early-accept is
implemented for both the initial match and lookahead matches. Integration test
`lazy2_ratio_better_than_greedy` confirms level 8 compresses better than level 5
on repetitive text. Design doc at `docs/steps/step-3-design.md`.

**Design decisions affecting later steps**:
- BtLazy2/BtOpt/BtUltra/BtUltra2 all use `max_lookahead=2` (lazy2) until Step 5
  replaces them with real binary-tree match finding.
- `prefer_match` heuristic is defined once and shared; Step 5's BT finder will
  reuse it for the lazy2 decision layer on top of BT search.
- `parse_ranges_greedy` and `parse_ranges_lazy` are private; the public interface
  is unchanged.

**Goal**: Implement lazy match evaluation for levels 6-12 (strategies `lazy`
and `lazy2`), where the encoder checks whether deferring a match by 1-2
positions yields a better result.

**What to do**:

1. Add a new match-selection layer between `find_match()` and `Event` emission.
   The current flow is: for each position, find best match, emit it or a
   literal. The new flow for lazy/lazy2:
   - Find match M0 at position P.
   - Find match M1 at position P+1.
   - (lazy2 only) Find match M2 at position P+2.
   - Compare using a cost heuristic: prefer longer matches, penalize larger
     offsets. Reference formula:
     `prefer_new = (4 * (new_len - old_len)) > (offset_bits_old - offset_bits_new)`
   - If a later position wins, emit the skipped positions as literals and use
     the later match.

2. This should be implemented as an alternative parsing strategy in `lz77.rs`,
   selected by the `Strategy` field. The `parse()` / `parse_ranges()` entry
   points should dispatch based on strategy:
   - `Greedy` (and `Fast`, `DFast` for now): current behavior.
   - `Lazy`: one-position lookahead.
   - `Lazy2`: two-position lookahead.

3. The `target_length` parameter should influence the decision: when
   `target_length > 0`, once a match exceeds `target_length`, skip the lazy
   check and accept immediately (the match is already good enough).

4. `search_log` should control the number of chain entries examined per search:
   `max_attempts = 1 << search_log`. This replaces the current `search_depth`
   which was a direct count.

**Key design decision**: Whether to implement lazy matching as a wrapper around
`find_match()` (calling it multiple times per position) or as a modified parsing
loop. The wrapper approach is simpler and more composable. The reference
implementation uses a single monolithic function
(`ZSTD_compressBlock_lazy_generic`) that interleaves searching and decision-making.

**Files touched**: `src/encoder/lz77.rs` (primary), `src/encoder/block.rs`
(if Event/parsing interface changes).

**New tests required**:
- Unit tests for the lazy decision heuristic: given two matches with known
  lengths and offsets, verify the correct one is chosen.
- Integration tests: round-trip at levels 6, 8, and 12 with varied input
  (repetitive text, mixed content) to exercise lazy and lazy2 paths.
- Test that `target_length` early-accept works: with a very low
  `target_length`, verify the encoder skips lazy evaluation for long matches.

**Validation**:
- `cargo test` and `cargo test --test acceptance` pass.
- Weighted benchmark at level 8 (lazy2) shows ratio improvement >= 3%
  compared to level 5 (greedy) on compressible categories (text, source, xml).
- Throughput regression at levels 6-12 is no more than 30% compared to greedy
  at the same level parameters (lazy does 2-3x more match searches per
  position, so some slowdown is expected and correct).
- Level 6 ratio is strictly better than level 5 ratio on compressible data.

**Reference**: [`lib/compress/zstd_lazy.c`](https://github.com/facebook/zstd/blob/dev/lib/compress/zstd_lazy.c),
specifically `ZSTD_compressBlock_lazy_generic()`. Also see the match comparison
logic in `ZSTD_count()` and `ZSTD_BtFindBestMatch_selectMLS()`.

---

## Step 4: Fast and double-fast strategies [DONE]

**Summary**: Added `parse_ranges_fast` (single `hash_table` lookup, no chain) and
`parse_ranges_dfast` (long `chain`-repurposed hash using 8-byte key, then short
`hash_table` with 4-byte key). Both use anchor-relative skip: on a miss, advance
by `1 + (pos - lit_start) >> search_log`. Added `hash8` (64-bit prime, `chain_log`
bits), `lookup_fast`, and `lookup_dfast` methods to `MatchFinder`. `parse_ranges`
dispatch updated: Fast → `parse_ranges_fast`, DFast → `parse_ranges_dfast`. Design
doc at `docs/steps/step-4-design.md`.

**Design decisions affecting later steps**:
- `chain` is repurposed as the long hash table for DFast (indexed by hash value, not
  position). This is exclusive with chain-link usage in Greedy/Lazy/BT modes.
- Fast/DFast do not insert positions inside matches (speed over ratio). Cross-block
  history still works at the match-start granularity.
- `lookup_fast` / `lookup_dfast` are `MatchFinder` methods and do not update chain
  links, so they can safely coexist with the chain-based `find_match`.

**Goal**: Implement the `fast` and `dfast` strategies for levels 1-4, which are
simpler than the current greedy approach and should be faster.

**What to do**:

1. **Fast strategy** (levels 1-2): Single hash table lookup, no chain walking.
   Hash the current position, check the single stored position, accept if match
   length >= `min_match`. No chain traversal at all. This is the simplest
   possible match finder. Note `min_match` is 7 at level 1 and 6 at level 2 —
   larger than the current fixed 3, which means fewer (but longer) matches.

2. **Double-fast strategy** (levels 3-4): Two hash tables — one for short
   matches (4-byte key) and one for long matches (8-byte key). Check the long
   table first; if it yields a match >= `min_match`, use it. Otherwise check
   the short table. Still no chain walking.

3. Add these as dispatch cases alongside `Greedy`, `Lazy`, `Lazy2` in the
   parsing entry point.

4. Both strategies use `step` (how many positions to skip on a miss). Reference
   zstd uses `step = 1 + (position >> search_log)` — positions deep into the
   input are skipped more aggressively. Implement this.

**Key design decision**: Whether `fast` and `dfast` need their own `MatchFinder`
variant or can reuse the existing one with chain walking disabled. A separate
lightweight struct is probably cleaner since they don't need a chain array at
all.

**Files touched**: `src/encoder/lz77.rs` (new match finder variants).

**New tests required**:
- Unit tests for the `fast` match finder: verify single-lookup behavior, that
  matches shorter than `min_match` are rejected, and that the step-skip logic
  works correctly.
- Unit tests for `dfast`: verify the two-table lookup priority (long table
  checked first).
- Integration tests: round-trip at levels 1-4 with varied input sizes.

**Validation**:
- `cargo test` and `cargo test --test acceptance` pass.
- Weighted benchmark at level 1 shows throughput improvement >= 10% compared
  to pre-step baseline (fast strategy eliminates chain walking).
- Ratio at levels 1-2 may worsen by up to 15% on compressible data due to
  higher `min_match` — this is correct behavior matching reference zstd.
- Ratio at levels 3-4 (dfast) should be within 5% of pre-step baseline.

**Reference**: [`lib/compress/zstd_fast.c`](https://github.com/facebook/zstd/blob/dev/lib/compress/zstd_fast.c),
`ZSTD_compressBlock_fast_generic()` and `ZSTD_compressBlock_doubleFast_generic()`.

---

## Step 5: Binary tree match finder

**Goal**: Implement a binary tree (BT) match finder for levels 13-15
(`btlazy2`), replacing hash chains with a sorted binary tree that provides
O(log N) match search per position.

**What to do**:

1. Implement a new match-finder data structure (`BtMatchFinder` or similar)
   that maintains a binary tree indexed by string content at each position:
   - **Storage**: Two arrays of `u32` (left child, right child), sized by
     `chain_log`. Each position in the window is a potential tree node.
   - **Insertion** (`insert_and_find()`): Insert the current position into the
     tree. Walk down comparing bytes at the current position against bytes at
     each node's position. Go left if current < node, right if current > node.
     Track the best match found during traversal.
   - **Search bound**: Limit tree traversal to `1 << search_log` comparisons.

2. The key insight: insertion and searching happen simultaneously. When you
   insert position P, you traverse the tree comparing P's content against
   existing nodes, which naturally finds the best match. This is why it's
   called "DUBT" (Dicho Unsigned Binary Tree) in the reference.

3. Combine BT match finding with lazy2 evaluation (from Step 3) to implement
   the `btlazy2` strategy. The match finder changes but the lazy decision
   logic stays the same.

4. The tree must handle the sliding window correctly: nodes older than the
   window must be treated as invalid during traversal (check position against
   a `lowLimit`).

**Key design decision**: Whether to use a single trait/enum for all match
finders (hash-chain, fast, BT) or keep them as separate types dispatched at
the `parse()` level. A trait like `MatchFinder` with `find_matches()` would
be cleanest, but the reference implementation uses quite different calling
conventions for each. Consider what gives the best performance without
excessive abstraction.

**Files touched**: `src/encoder/lz77.rs` (new BT data structure + integration),
possibly a new file `src/encoder/bt_match.rs` if the code is substantial.

**New tests required**:
- Unit tests for BT insertion and search: verify that inserting N positions and
  searching returns the correct longest match. Test with known collision
  patterns to exercise tree balancing.
- Unit test for window eviction: verify that positions older than the window
  are not returned as matches.
- Integration tests: round-trip at levels 13-15 with large (>256 KiB) input.

**Validation**:
- `cargo test` and `cargo test --test acceptance` pass.
- Weighted benchmark at level 13 (btlazy2) shows ratio within 2% of level 12
  (lazy2 with hash chains) — BT changes the search structure, not the
  fundamental strategy, so ratio should be similar.
- Throughput at level 13 should not regress more than 20% compared to level 12.
  (BT search is O(log N) per lookup vs O(depth) for chains, so it may be
  faster or slower depending on data — neither extreme is a failure.)

**Reference**: [`lib/compress/zstd_lazy.c`](https://github.com/facebook/zstd/blob/dev/lib/compress/zstd_lazy.c)
(BT functions: `ZSTD_insertBt1()`, `ZSTD_BtFindBestMatch_selectMLS()`).
Also [`lib/compress/zstd_compress_internal.h`](https://github.com/facebook/zstd/blob/dev/lib/compress/zstd_compress_internal.h)
for the match state structures.

---

## Step 6: Optimal parsing

**Goal**: Implement cost-based optimal parsing for levels 16-19 (`btopt`,
`btultra`, `btultra2`), using dynamic programming to find the globally optimal
sequence of literals and matches for each block.

**What to do**:

1. **Cost model**: Build a pricing function that estimates the bit cost of:
   - A literal: based on observed literal frequency statistics (approximate
     entropy).
   - A match: sum of offset code cost + match length code cost. Use the FSE
     predefined table weights as initial estimates, then update from observed
     frequencies.
   - Maintain running frequency tables for literals, match lengths, offsets,
     and literal lengths. Rescale periodically to prevent overflow and adapt
     to local statistics.

2. **DP algorithm** (forward-pass optimal parsing):
   - Allocate a price table: `price[i]` = minimum cost to encode positions
     `0..i`.
   - Initialize: `price[0] = 0`, all others = infinity.
   - For each position `i` in order:
     - **Literal extension**: `price[i+1] = min(price[i+1], price[i] + literal_cost(data[i]))`.
     - **Match extension**: For each match M found by the BT match finder at
       position `i`, for each valid length `len` from `min_match` to `M.length`:
       `price[i+len] = min(price[i+len], price[i] + match_cost(M.offset, len))`.
     - Track back-pointers to reconstruct the optimal sequence.
   - Process in chunks (reference uses ~`ZSTD_OPT_NUM = 256` positions at a
     time, not the entire block).

3. **Back-trace**: After filling the price table for a chunk, walk back from
   the end to reconstruct the optimal Event sequence (literals and matches).

4. **btopt vs btultra differences**:
   - `btopt`: Uses integer-approximated bit costs.
   - `btultra`: Uses fractional-bit costs (fixed-point arithmetic) for more
     precise decisions.
   - `btultra2`: Additionally does exhaustive search initialization and
     re-evaluates with updated statistics.

   Implement `btopt` first. `btultra`/`btultra2` refinements can be added
   incrementally as the cost model is tuned.

5. **Integration**: The optimal parser produces the same `Event` stream as
   other strategies. It uses the BT match finder from Step 5 to enumerate
   candidate matches at each position, then the DP selects which to use.

**Key design decisions**:
- Chunk size for the DP window (256 is the reference default).
- How to initialize frequency statistics for the cost model (use predefined
  FSE table weights, or a first-pass scan of the block).
- Whether to implement `btultra`/`btultra2` as separate strategies or as
  flags on the `btopt` implementation.
- Memory budget: the price table + back-pointers + frequency tables add
  meaningful per-block overhead.

**Files touched**: New file `src/encoder/optimal.rs` (DP parser + cost model),
`src/encoder/lz77.rs` (dispatch for btopt/btultra strategies),
`src/encoder/block.rs` (if the Event interface needs adaptation).

**New tests required**:
- Unit tests for the cost model: verify literal and match pricing against
  hand-calculated examples. Test that frequency rescaling preserves relative
  ordering.
- Unit tests for the DP: given a small input with known match candidates,
  verify the optimal parser selects the expected sequence (not necessarily
  the greedy one).
- Integration tests: round-trip at levels 16, 18, and 19 with varied input.
  Include a test with highly compressible data where optimal parsing should
  visibly outperform greedy/lazy.

**Validation**:
- `cargo test` and `cargo test --test acceptance` pass.
- Weighted benchmark at level 16 (btopt) shows ratio improvement >= 2%
  compared to level 15 (btlazy2) on compressible categories.
- Ratio at levels 16-19 is within 10% of reference zstd at the same levels
  on the Silesia corpus (our encoder may not match reference exactly, but
  should be in the same tier).
- Throughput at level 16+ will be significantly slower than lazy2 — this is
  expected. Verify throughput is within 3x of reference zstd at the same
  level (if we're 10x slower, the implementation has a bug or inefficiency).

**Reference**: [`lib/compress/zstd_opt.c`](https://github.com/facebook/zstd/blob/dev/lib/compress/zstd_opt.c),
specifically `ZSTD_compressBlock_opt_generic()`. Also
[`lib/compress/zstd_opt.h`](https://github.com/facebook/zstd/blob/dev/lib/compress/zstd_opt.h)
for the optimal match structures and cost tables.

---

## Step ordering and dependencies

```
Step 1: Parameter table & strategy enum
  │
  ├──> Step 2: Variable window sizes
  │      │
  │      ├──> Step 3: Lazy/lazy2 matching
  │      │      │
  │      │      ├──> Step 5: Binary tree match finder (uses lazy2 from Step 3)
  │      │      │      │
  │      │      │      └──> Step 6: Optimal parsing (uses BT from Step 5)
  │      │      │
  │      └──> Step 4: Fast/dfast strategies (independent of Steps 3, 5, 6)
  │
```

Steps 3 and 4 are independent of each other and can be done in either order.
Step 5 depends on Step 3 (btlazy2 combines BT + lazy2). Step 6 depends on
Step 5 (optimal parsing uses BT match finding).

## Incremental value

Each step delivers independently useful improvements:

- **After Step 1**: Clean parameter model, ready for future work.
- **After Step 2**: Better ratios at higher levels due to larger history windows.
- **After Step 3**: Significantly better ratios at levels 6-12 from lazy matching.
- **After Step 4**: Faster compression at levels 1-4.
- **After Step 5**: Better match finding quality at levels 13-15.
- **After Step 6**: Near-reference compression ratios at levels 16-19.
