# Ideas

## Strategic learnings from experiments

These patterns emerged from the results tracked in `results.tsv`:

- **Micro-optimizations do not clear the Silesia gate.** Several inner-loop
  tweaks (16-byte chunk match extension, binary-search sequence length lookup,
  min_match 3->4) each improved the weighted synthetic benchmark by 1-4%, and
  even stacked they reached +4.15% on weighted compress -- but Silesia level-3
  compress moved only +0.84%. The two-gate system correctly rejected these.
- **Whole-pass removal is the credible path.** The largest compress-speed wins
  came from eliminating entire units of work: skipping sequence self-validation
  (~1.6x), restructuring LZ77 skip insertion into separate loops (~2x on
  all_zeros), and unaligned loads (~1.15x). These cleared both gates.
- **Event-free sequence collection is another credible path.** Replacing the
  encoder's `Event` enum handoff with a direct LZ77 sink, then precomputing
  literal/match/offset code lookups, improved weighted compress from `2189.7`
  to `2287.4 MB/s` (`+4.46%`) with flat ratio and confirmed on Silesia level 3
  from `51.1` to `62.9 MB/s` (`+23.09%`). The common thread is removing
  repeated per-sequence bookkeeping instead of tuning individual comparisons.
- **Format cleanups alone are too small.** Omitting the implied final Huffman
  weight from the direct header only moved weighted compress by ~1.9% and
  regressed weighted decompress by ~2.0%, so header-size-only changes are
  unlikely to clear the current gate without a larger pipeline win attached.
- **Low-level LZ77 search cuts are plateauing.** An early-exit + sparser
  reinsertion + lower-search-depth stack on top of `7fb6b45` only reached
  `+1.91%` weighted compress at best with flat ratio, then fell back to
  `+1.10%` on the third attempt. Small search-pruning stacks in the current
  matcher are not enough to clear the 3% weighted gate.
- **No-match shortcuts help, but not enough by themselves.** Skipping
  literal/Huffman work after an LZ77 parse with zero matches improved weighted
  compress by ~1.6% with a small decompress dip; stacking a sparse whole-block
  no-match precheck pushed decompress past the -1% gate. This path likely needs
  a more selective heuristic or a different companion win.
- **Weighted can false-positive on untouched paths.** A decoder-only change
  that streamed sequence decode directly into execution showed a +3.0% weighted
  compress gain, but Silesia level-1 compress only moved +0.6% while Silesia
  decompression regressed by ~2.3%. Treat weighted gains on metrics the patch
  does not directly touch as suspect until Silesia confirms them.
- **Literal bookkeeping can also false-positive on weighted.** Deferring
  literal materialization and folding frequency counting into a final copy pass
  improved weighted compress by `+7.11%`, but Silesia compress slipped from
  `50.9` to `50.4 MB/s` at level 1 and stayed below the gate at higher levels.
  Short synthetic corpora overstated the value of extra range bookkeeping in
  the block encoder.
- **Encoder-side FSE state choices affect decode speed.** Reworking sequence
  encoding to derive FSE transitions directly during the reverse write loop
  pushed weighted compress to roughly +4.6% and Silesia level-3 compress to
  81.1 MB/s, but the resulting streams decompressed 1.0-1.7% slower on
  Silesia despite identical ratios. Stream shape matters; validate decoder
  throughput even when an encoder refactor looks format-equivalent.
- **Inline reverse FSE transitions can be a real win when the stream shape stays the same.**
  Removing the per-block LL/OF/ML state-path vectors entirely and deriving the
  previous FSE states inline from the current suffix state preserved the
  existing bitstream layout, improved weighted compress from `2234.1` to
  `2346.5 MB/s` (`+5.03%`), and improved Silesia level-1 compress from `74.5`
  to `80.4 MB/s` (`+7.92%`) with flat ratio and flat decompression. The
  important part was deleting the staging buffers without changing symbol order
  or transition selection.
- **No-match raw-block short-circuits still need to beat the simpler keep.**
  Returning `None` from `encode_block()` when LZ77 found no sequences looked
  like a clean complement to the inline-transition win, but on top of the kept
  state it moved weighted compress from `2346.5` to `2344.7 MB/s` while adding
  API complexity. If a follow-up shortcut does not improve on the current keep,
  discard it even if it still clears the original baseline gate.
- **Random-data fast path was the single biggest win.** Sampling-based
  incompressible detection took random/1 compress from ~48 MiB/s to ~9.8 GiB/s
  (200x). This is a structural shortcut, not a micro-optimization.
- **The current low-level incompressible gate is already near the throughput/ratio frontier.**
  Two attempts to weaken it by demanding more sampled evidence before bailing
  out both improved weighted ratio from `0.2293` to `0.2253`, but they cut
  weighted compress roughly in half and weighted decompress by about a third.
  For the current synthetic mix, the existing `<=1 exact repeat in first 4 KiB`
  rule is crude but effective; do not relax it without a much stronger
  block-level classifier.
- **Cheap literal-section shortcuts are not the bottleneck.** Counting literal
  frequencies during parse collection and skipping the impossible `>128 symbol`
  Huffman path both regressed weighted compress by ~1.3% while barely moving
  ratio. The extra literal scan is not where the current encoder is paying its
  largest costs.
- **Literal-table reuse can false-positive on weighted too.** Skipping the
  rebuild from direct weights back into a normalized Huffman table looked great
  on the weighted suite (`2330.5 -> 2459.0 MB/s`, `+5.51%`), but exact Silesia
  comparisons only improved compress by `0.6-1.0%` on levels `1/3/9` and
  regressed level-19 compress plus level-1/19 decompress beyond the gate. The
  weighted harness overstated this small-file literal-header win; do not trust
  it without Silesia confirmation.
- **Option-tag overhead in cached FSE transitions is below the gate.** Packing
  the predefined FSE transition cache into dense `u32` entries preserved the
  bitstream once the field layout was fixed, but only moved weighted compress
  by `+0.25%`. That is below the current frontier; look for bigger cuts than
  removing `Option`/tuple overhead from the sequence encoder.
- **Naive scratch reuse can backfire.** A profiled attempt to reuse
  `MatchFinder` storage with generation-stamped hash/chain tables regressed
  weighted compress from `2301.3` to `2147.1 MB/s` (`-6.70%`) despite flat
  ratio. The extra memory footprint and branchy epoch checks cost more than the
  avoided zero-fill/allocation in the current design.
- **Decoder wins come from reducing copies.** Reading sequence back-references
  directly from output (instead of cloning history per block) improved Silesia
  decompress by ~3-5% across all levels and improved repetitive roundtrip by
  13-17%.
- **Block-level RLE matters.** Emitting RLE blocks for single-byte content
  improved all_zeros compress by ~35% and decompress from ~678 MiB/s to
  ~12 GiB/s (18x).
- **DFast-to-Greedy at shallow depth is too blunt.** On the current
  `fac345b` branch, changing levels `3-4` from `DFast` to `Greedy` while
  keeping `search_log=1` crushed the weighted synthetic ratio
  (`0.4163 -> 0.2294`) and even improved weighted compress
  (`1371.7 -> 1443.1 MB/s`), but Silesia level-3/4 compression throughput
  roughly halved and decompression fell by ~30%. This branch's poor weighted
  ratio is partly a parser-quality problem, but replacing DFast wholesale is
  not within budget; future DFast work should borrow selective parse-quality
  ideas without changing the strategy family outright.
- **DFast matched-run reinsertion buys real ratio at a real speed cost.** On
  `18338fb`, DFast was not reinserting any matched positions because the
  generic `skip()` helper assumes a per-position chain layout that DFast does
  not use. Adding a DFast-specific sparse reinsertion path for the matched run
  improved Silesia ratio from `1.872 -> 2.052` at level `3` and
  `1.944 -> 2.142` at level `4`, but it also dropped Silesia compression speed
  by `12-15%` and decompression by about `5%`, with weighted compress falling
  `16%`. This is a legitimate ratio-first lever for DFast, not a false
  positive, but it is expensive enough that follow-up work should focus on
  recovering the speed loss rather than adding even more parse work.
- **Comparing both DFast candidates at every position is below the bar.** A
  local change that evaluated both the 8-byte long-hash hit and the 4-byte
  short-hash hit before choosing a match nudged Silesia level-3 ratio from
  `1.872` to `1.880`, left level `4` flat at `1.944`, and cost about `8%`
  compression speed on both levels. The problem is not just candidate choice at
  one position; the larger missed opportunity was the absence of matched-run
  reinsertion.
- **DFast follow-ups need larger leverage than reinsertion tuning.** On
  `59ed008`, making the long-hash reinsertion path sparser inside matched runs
  reduced Silesia ratio from `2.052 -> 2.050` at level `3` and
  `2.142 -> 2.130` at level `4` while also missing any meaningful compression
  speed recovery. A second attempt that only checked the short-hash candidate
  when a long-hash hit was still short left ratio effectively flat
  (`2.052 -> 2.053` at level `3`, level `4` unchanged) but cost
  `3-4%` compression throughput. The current DFast state seems constrained by
  broader parse quality and reinsertion cost together; small local tweaks to
  long-table density or conditional second-choice checks are below the bar.
- **Cheaper DFast hashing still missed the subsystem gate.** On `ddfeb6f`,
  replacing the slice-to-array conversions inside `hash4()` / `hash8()` with
  unaligned loads improved the weighted benchmark from `1123.6` to
  `1154.2 MB/s` (`+2.72%`) with flat ratio, and the first Silesia run improved
  level-3/4 compression from `208.7/190.2` to `213.0/193.5 MB/s`. But the
  required rerun came back at `214.3/190.1 MB/s`, which confirmed level `3`
  only and left level `4` flat. A more aggressive variant that derived both
  DFast hashes from one 8-byte load regressed level-4 compression to
  `184.7 MB/s`. Conclusion: the current DFast speed debt is larger than the
  hash helper overhead alone; weighted overstates this win and future recovery
  work needs a bigger structural cut than cheaper fingerprint loading.
- **The current DFast hot path is split between lookup and matched-run reinsertion.**
  A CPU profile on `dickens` at level `3` (`c63e52c`) put about `46%` of
  samples in `MatchFinder::lookup_dfast` and another `~15%` in
  `skip_dfast -> insert_dfast_position -> hash4`, with sequence bit writing a
  separate `~23%`. That profile says the parser is still the main subsystem
  cost center, but it also says pure hash-helper cleanup is only shaving part
  of the debt.
- **DFast-specific 8-byte cleanup attempts stayed below the bar here.** On
  `c63e52c`, validating long-table candidates with 8-byte equality and
  extending matches from byte `8` nudged Silesia compression from
  `207.1 -> 210.8 MB/s` at level `3` and `188.7 -> 189.4 MB/s` at level `4`,
  but it also slipped ratio from `2.052 -> 2.051` and `2.142 -> 2.138`, with
  weighted compress falling slightly (`1113.5 -> 1109.2 MB/s`). A second
  attempt to derive both reinsertion hashes from one 8-byte load inside
  `skip_dfast()` was worse: Silesia compress fell to `196.9/177.4 MB/s` with
  flat ratio. Conclusion: on this branch, the DFast speed debt is not coming
  from one obviously redundant 8-byte check or reinsertion load.
- **Making DFast miss skipping more aggressive pushes it toward the wrong tradeoff.**
  Also on `c63e52c`, raising DFast `search_log` from `1` to `2` improved ratio
  sharply on Silesia (`2.052 -> 2.285` at level `3`, `2.142 -> 2.359` at level
  `4`) and improved weighted ratio/compress, but it cratered Silesia
  compression (`207.1 -> 169.1 MB/s`, `188.7 -> 154.9 MB/s`) and cut Silesia
  decompression by roughly `11-13%`. This behaves like a broader parser
  strategy shift, not a balanced DFast recovery; future DFast work should aim
  to recover the matched-run reinsertion overhead without increasing overall
  parse aggressiveness.
- **Backward match extension was below the gate here.** Extending emitted
  matches backward in the non-optimal parsers only moved weighted ratio from
  `0.4163` to `0.4151` and weighted compress from `1371.7` to `1377.9 MB/s`.
  That is too small on this branch; improving DFast ratio likely needs bigger
  changes than post-match anchor adjustment.
- **Implicit-final-weight direct headers are below the gate on high-ratio levels.**
  Reclaiming the direct Huffman header's omitted final weight slot was
  interoperable and covered one more active symbol, but Silesia levels
  `16/18/19` stayed flat at `3.101/3.161/3.167` ratio with effectively
  unchanged compression speed. On this branch, the missing ratio is not coming
  from the 128-vs-129 direct-header boundary alone; the full FSE-compressed
  Huffman-weight path or a larger parser/coding change is still required.
- **The same direct-header tweak still false-positives on weighted here.**
  On `ad5bb47`, omitting the explicit final direct Huffman weight improved the
  weighted composite compress score from `1067.9` to `1123.6 MB/s` (`+5.22%`)
  with flat weighted ratio, but exact Silesia comparison on levels `1/3/9/19`
  kept ratio pinned at `1.216/2.052/2.822/3.167` and regressed compression
  throughput by `1.8-5.2%` on levels `1/3/9`. Treat tiny literal-header
  cleanups as another weighted false-positive class on this branch unless a
  long-file benchmark shows visible per-file ratio movement.

## Remove a whole compress-side pass at level 3

**Status:** Partially validated on 2026-03-21
**Context:** A structural version of this idea did clear both gates: streaming LZ77 parse events directly into block literal/sequence collection removed the intermediate `Vec<Event>` pass and improved the weighted suite from `1994.2` to `2156.2 MB/s` (`+8.12%`) with flat ratio. Serial Silesia confirmed the win at level 3 from `49.4` to `50.9 MB/s` (`+3.04%`) while keeping ratio flat.

**Opportunity:** Continue looking for pass-elimination changes that remove allocations or second walks over block data. The main lesson is that structural cuts in the compression pipeline can survive Silesia, unlike several earlier inner-loop-only wins.

**Implementation notes:**
- Focus on code that runs across long compressible files, not just tiny hot loops: `src/encoder/block.rs`, `src/encoder/lz77.rs`, and block-level decisions in `src/frame.rs`.
- Streaming parse events worked; adjacent ideas worth trying next are scratch-buffer reuse across blocks and avoiding needless fallback copies when a block is abandoned.
- Porting the same structural win onto `origin/main` still cleared both gates
  when paired with precomputed length/offset lookup tables and cached FSE
  transition tables: weighted compress moved from `2191.5` to `2312.4 MB/s`
  (`+5.51%`), and Silesia level-1/3 compress moved from `50.9/51.1` to
  `75.7/75.5 MB/s` with flat ratio.
- Push further on the direct-sink path: repeated enum materialization and
  reverse table scans were worth removing, so adjacent wins likely live in
  other per-sequence bookkeeping that still runs on every match.
- Caching the predefined FSE encoder transition tables also cleared both
  gates: weighted compress moved from `2230.1` to `2317.3 MB/s` (`+3.01%`)
  and Silesia level-3 compress from `63.1` to `75.6 MB/s` (`+19.81%`) with
  flat ratio and within-gate decompression changes.
- Keep an eye on benchmark variance near the 3% weighted gate. This branch
  showed reruns between roughly `2294` and `2317 MB/s` after the accepted FSE
  cache change, so a single borderline weighted pass is not enough evidence by
  itself; lean on repeated runs and Silesia confirmation.
- Treat weighted-only wins as suspicious until Silesia confirms them; longer-file behavior matters more than short synthetic cases.
- Prefer changes that reduce allocations or whole passes over tweaks like wider compare chunks or lookup micro-optimizations.

## FSE-compressed Huffman weight encoding

**Status:** Not yet implemented
**Context:** The encoder only supports direct-mode Huffman weight encoding
(header_byte 128-255), limited to ~128 active symbols. Data with >128 distinct
byte values (executables, medical/image, pseudo-random) falls back to raw
literals because the Huffman table cannot be serialized.

**Opportunity:** Implementing FSE-compressed weight encoding would allow
Huffman-coded literals for high-entropy data with large alphabets. The decoder
already has `decode_fse_weights()` in `src/huffman.rs`.

**Implementation notes:**
- Weight distributions typically have few distinct values (weights 1-11), so
  the FSE table is small.
- Adopt the spec convention of omitting the last weight (implicit from tree
  completeness), giving +1 symbol capacity in both modes.
- Compare compressed-weight vs direct-mode header size and pick smaller.
- This could improve ratio on the Silesia corpus files that currently get raw
  literals (e.g., `x-ray`, `mr`, `nci`).

## ~~lzbench-style benchmark against reference zstd~~

**Status:** Done -- implemented as `examples/silesia_bench.rs`
(commits `c79ad30` through `ccf2358`). Reports ratio, compression MB/s, and
decompression MB/s per file and aggregate, with optional side-by-side against
system `zstd`. Run with:
```bash
cargo run --release --example silesia_bench -- --download --implementation both
```

## ~~Silesia round-trip failure on real corpus~~

**Status:** Fixed in commit `7364779` ("Fix Silesia Huffman literal round-trip
regression"). The `dickens` file at level 1 was triggering
`Huffman table error: max_bits out of range` due to a literal/Huffman header
bug. Silesia now round-trips cleanly at all tested levels.
