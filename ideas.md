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
- **Chunked overlapping match replay is a real decompression win.** On
  `d7d7172`, replacing the decoder's per-byte back-reference loop with
  chunked `Vec::extend_from_within()` copies plus a one-shot reserve kept
  ratio flat and compression roughly flat, while Silesia decompression moved
  from `1365.1 -> 1662.6 MB/s` at level `1`, `386.9 -> 473.9 MB/s` at level
  `3`, `308.3 -> 381.8 MB/s` at level `9`, and `350.3 -> 460.3 MB/s` at
  level `19`. Future decoder work should keep targeting byte-at-a-time replay
  and realloc-heavy paths before micro-optimizing table reads.
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
- **Selective DFast lookahead buys too little ratio for its speed cost.** On
  `d6ca7d5`, probing `pos+1` for short greedy DFast matches improved Silesia
  ratio from `2.052 -> 2.098` at level `3` and `2.142 -> 2.185` at level `4`,
  but compression fell from `209.6 -> 190.2 MB/s` and `189.5 -> 169.9 MB/s`.
  That is not a better trade than the matched-run reinsertion keep; future
  DFast ratio work needs a larger parser-quality gain than a one-byte lazy
  check can deliver.
- **DFast hash helper cleanup is still a borderline false positive.** Also on
  `d6ca7d5`, replacing the slice-to-array conversions in `hash4()` / `hash8()`
  with unaligned load helpers improved one Silesia run to `213.9/192.6 MB/s`
  at levels `3/4`, but the confirmation rerun came back at `211.9/185.5 MB/s`
  with flat ratio. The profile still shows hash helper overhead inside
  `skip_dfast()`, but the isolated cleanup remains too small and too noisy to
  clear the subsystem gate on this branch.
- **Making DFast miss skipping more aggressive pushes it toward the wrong tradeoff.**
  Also on `c63e52c`, raising DFast `search_log` from `1` to `2` improved ratio
  sharply on Silesia (`2.052 -> 2.285` at level `3`, `2.142 -> 2.359` at level
  `4`) and improved weighted ratio/compress, but it cratered Silesia
  compression (`207.1 -> 169.1 MB/s`, `188.7 -> 154.9 MB/s`) and cut Silesia
  decompression by roughly `11-13%`. This behaves like a broader parser
  strategy shift, not a balanced DFast recovery; future DFast work should aim
  to recover the matched-run reinsertion overhead without increasing overall
  parse aggressiveness.
- **Parser-side repeat-offset probing is a real ratio lever, but too expensive in the current Greedy/Lazy matcher.** On `60e9103`, letting Greedy/Lazy compare explicit repeat-offset matches against the hash-chain result improved Silesia ratio from `2.588 -> 2.633` at level `5`, `2.612 -> 2.633` at level `6`, `2.824 -> 2.843` at level `8`, and `2.987 -> 3.002` at level `12`, but compression fell by `8.8-11.7%` on levels `5/6/8`. A narrower retry that only probed zero-literal repeat chains recovered some speed (`60.3/60.3/23.7/6.2 MB/s` vs baseline `64.3/64.3/24.9/6.2`) but still missed the subsystem budget. Conclusion: the missing ratio is partly in repcode-aware parse choices, but probing repcodes opportunistically at parse time is too costly unless the matcher can surface them much more cheaply than an extra per-position compare-and-extend path.
- **Sparse DFast miss seeding is a real ratio lever.** On `ca71927`, seeding a
  few skipped miss positions into the DFast tables before anchor-relative jumps
  improved Silesia ratio from `2.052 -> 2.121` at level `3` and
  `2.142 -> 2.214` at level `4`, with compression changing from
  `208.6 -> 200.0 MB/s` and `184.1 -> 182.2 MB/s` on the confirmation rerun.
  A heavier variant that always seeded both the midpoint and landing position
  pushed ratio slightly higher (`2.128/2.220`) but cost more speed
  (`197.7/180.1 MB/s`). The better trade on this branch was to always seed the
  landing position and only add the midpoint when the jump reached 8 bytes.
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
- **A simplified FSE-compressed Huffman header is not a safe shortcut.** On
  `1c35506`, a first-pass encoder that tried to serialize Huffman weights with
  a locally inverted FSE table and a simplified NCount writer cleared a small
  unit round-trip and acceptance, but it panicked in release Silesia at level
  `1` with an out-of-bounds state in `decode_fse_weights()`. Future work on
  compressed Huffman weights should assume the full zstd format requirements
  matter here: spec-accurate NCount encoding and the two-state weight stream,
  not a single-state approximation.
- **A more reference-shaped compressed-weight retry still failed the real gate.**
  On `570fb6a`, a second attempt that added local NCount emission plus paired
  FSE encode/decode helpers still failed Silesia round-trip immediately on
  `mozilla` with `invalid implied Huffman weight`. The lesson is stronger than
  the original crash: this path needs exact parity with upstream
  `HUF_compressWeights()` / `FSE_decompress_wksp()`, not a local approximation
  that merely passes acceptance and small tests.
- **Lazy Huffman decode-table caching is below the bar on current Silesia.**
  Also on `570fb6a`, caching the per-table Huffman decode lookup so repeated
  literal decodes could reuse it moved Silesia decompression from
  `1690.2 -> 1662.2 MB/s` at level `1`, `458.8 -> 458.9 MB/s` at level `3`,
  and `380.4 -> 380.5 MB/s` at level `9`, with compression also slightly down
  at levels `3/9`. The decode-table rebuild cost is not the current
  decompression bottleneck; future decoder work should stay focused on
  back-reference replay and larger copy-path cuts.
- **One more search-depth notch is below the gate for Optimal BT.** On
  `af0f8cb`, raising the binary-tree search depth by one notch for levels
  `16-19` moved Silesia ratio only from `3.101 -> 3.102`, `3.161 -> 3.162`,
  and `3.167 -> 3.168`, with level-19 compression slipping from `3.3` to
  `3.2 MB/s`. The ratio change is too small to be visible on a per-file basis,
  so future high-level ratio work should target better parse scoring or literal
  coding completeness, not a uniform depth increase.
- **Borrowed raw/RLE literals are not a free decompression win here.** On
  `294aa8e`, keeping raw and RLE literal sections in a borrowed/compact form
  all the way into sequence execution looked like an allocation cut, but
  Silesia decompression regressed immediately from `1680.5 -> 1648.2 MB/s` at
  level `1` and `454.1 -> 436.2 MB/s` at level `3` before the run was
  stopped. The extra per-range dispatch in sequence execution cost more than
  the saved literal materialization on this branch; future decoder work should
  stay focused on copy/replay hot paths, not literal-source abstraction.
- **Lazy `target_length` is a parse-policy knob, not a safe hash-chain cutoff.**
  Also on `294aa8e`, stopping `find_match()` as soon as Lazy/Lazy2 found any
  match at or above `target_length` spiked level-6 compression from
  `63.3 -> 90.9 MB/s`, but it collapsed ratio from `2.612 -> 2.461` and cut
  decompression from `359.4 -> 329.0 MB/s`. The current lazy-family ratios
  still depend on continuing the local chain walk even after the parser would
  later "early accept" the final winner; future speed work needs a more
  selective pruning rule than treating `target_length` as a hard search stop.
- **Returning the “best bit-gain” BT candidate is the wrong lever for Optimal BT here.**
  On `c16ab89`, changing `bt_find_insert()` so `BtOpt/BtUltra/BtUltra2` kept
  the candidate with the best estimated `8*len - match_cost_bits()` score
  nudged Silesia ratio from `3.149 -> 3.157` at level `16`, `3.214 -> 3.226`
  at level `18`, and `3.228 -> 3.240` at level `19`, but compression fell to
  `4.9/3.7/3.0 MB/s` from the `5.2/4.0/3.2 MB/s` baseline. The BT traversal
  cost did not change, so the lost throughput likely came from picking shorter,
  cheaper matches that increased sequence count. Future Optimal BT work should
  preserve the longest-match candidate set and improve parse choice with richer
  DP inputs, not by replacing the BT’s primary candidate with a cost heuristic.
- **A single cheap Greedy/Lazy rep0 probe is still below the bar.** On
  `861a72f`, adding a zero-literal primary-repeat check to the Greedy/Lazy
  parser path kept Silesia ratio flat at levels `5/6/8/12`, while compression
  fell from `64.7 -> 61.0 MB/s` at level `5` and `64.6 -> 60.9 MB/s` at level
  `6`. The missing Greedy/Lazy ratio is not unlocked by one extra current-pos
  repcode hint; future repeat-aware work there needs a larger parse-quality win
  than a selective rep0 retry.
- **A wider Optimal BT DP horizon is a real ratio win.** Also on `861a72f`,
  doubling `OPT_CHUNK_SIZE` from `256` to `512` improved Silesia ratio from
  `3.133 -> 3.149` at level `16`, `3.194 -> 3.214` at level `18`, and
  `3.201 -> 3.228` at level `19`, while compression stayed flat at
  `5.2/4.0/3.2 MB/s` and a rerun reproduced the same ratios. That points to
  DP horizon, not just search depth, as a live leverage point for the
  high-level parser.
- **Repeat offsets become viable when confined to the ratio-first levels.** On
  `056a9d6`, re-enabling repeat-offset encoding only for the `Optimal BT`
  family (`16-19`) improved Silesia ratio from `3.101 -> 3.133` at level `16`,
  `3.161 -> 3.194` at level `18`, and `3.167 -> 3.201` at level `19`, while
  keeping compression flat-to-slightly-better (`5.1 -> 5.2`, `3.9 -> 4.0`,
  `3.1 -> 3.2 MB/s`). The earlier all-level repeat-offset attempts failed
  because the fast and mid levels could not afford the trade; the ratio-first
  family can. Keep the encoder-side repeat-offset state local until the block
  is actually emitted as compressed, or raw/RLE fallbacks will corrupt later
  blocks.
- **BtLazy2 can also afford repeat offsets, but only as an encoder-side gate.**
  On `acdd970`, widening the existing repeat-offset encoder gate from
  `Optimal BT` only to `BtLazy2 + Optimal BT` improved Silesia ratio from
  `2.875 -> 2.900` at level `13` and `2.885 -> 2.910` at level `15`, while
  keeping compression effectively flat (`17.5 -> 17.5/17.6 MB/s`,
  `16.3 -> 16.3 MB/s`) and only trimming decompression by about `0.4-0.7%` on
  confirmation. This is within the subsystem budget and suggests the earlier
  all-level repeat-offset failures were mostly caused by lower families rather
  than by `BtLazy2` itself.
- **DFast still responds to combined table-access cleanup, but not to stale short-table shortcuts.**
  On `93e9467`, replacing DFast's slice-to-array hash inputs with direct
  word loads, reusing the loaded word for both hash selection and candidate
  checks, and rewriting `skip_dfast()` with indexed loops improved weighted
  compress from `1208.8` to `1236.0 MB/s` (`+2.25%`) with flat weighted ratio,
  while confirmed Silesia compression improved from `198.7 -> 204.3 MB/s` at
  level `3` and `180.4 -> 186.5 MB/s` at level `4` with flat ratio. A follow-up
  that deferred short-hash updates until after the long-table probe missed
  reached `206.7/186.8 MB/s`, but it regressed Silesia ratio to `2.119/2.207`.
  Conclusion: DFast still has some speed headroom in redundant table-access and
  iterator overhead, but keeping the short table fresh on long-match hits is
  part of the current ratio floor.
- **Making sparse matched-run reinsertion less short-table-heavy still loses ratio.**
  On `df33081`, two follow-ups tried to cut DFast reinsertion work inside the
  sparse middle of matched runs: one refreshed only the long table there, and
  the other kept short-table updates only at the sparse boundaries. Both nudged
  level-3 compression up (`203.6 -> 207.3 MB/s` and `203.6 -> 205.3 MB/s`),
  but both also regressed Silesia ratio on levels `3/4` (`2.121 -> 2.117/2.119`
  and `2.214 -> 2.212`). Conclusion: on this branch, the sparse middle still
  needs short-table freshness often enough that selectively dropping those
  updates is not a balanced DFast trade. Future DFast speed work should target
  cheaper dual-table maintenance, not fewer short-table writes.
- **Current DFast cost is still mostly parser lookup, not collector copy overhead.**
  A fresh profile on `2ff55ce` for `dickens` at level `3` showed
  `lookup_dfast` at `45.49%`, `skip_dfast` at `13.68%`,
  `skip_dfast_miss_positions` at `1.34%`, sequence encoding at `22.73%`, and
  collector-side sink work split between sequence pushes (`5.79%`) and literal
  slice indexing (`3.81%`). A collector retry that preallocated the full
  literal buffer and copied literal runs directly only moved Silesia
  compression from `203.8 -> 205.1 MB/s` at level `3` while regressing level
  `4` from `185.4 -> 176.4 MB/s`. Conclusion: the visible collector overhead is
  not the main DFast bottleneck on this head; parser-side work still dominates.
- **Single-load DFast table-maintenance retries regressed on this head.**
  Also on `2ff55ce`, reusing one 8-byte load to derive both short and long
  hashes and splitting reinsertion into long-valid vs short-only loops kept
  ratio flat but dropped Silesia compression from `203.8 -> 198.9 MB/s` at
  level `3` and `185.4 -> 182.3 MB/s` at level `4`. Conclusion: this branch's
  DFast maintenance debt is not solved by refactoring the hash-load plumbing
  alone; future work should look for fewer parser probes or higher-yield match
  selection changes rather than another hash/reinsertion helper cleanup.
- **DFast long-hash validation still does not pay for itself on this head.**
  On `74e9910`, validating long-table hits with full 8-byte equality and
  extending matches from byte `8` slipped Silesia ratio from `2.121 -> 2.119`
  at level `3` and `2.214 -> 2.213` at level `4`, while compression stayed
  essentially flat at `204.4/186.3 MB/s` versus the `204.7/186.5 MB/s`
  baseline. Conclusion: even after the later DFast cleanups, rechecking the
  first long-match word is still below the gate.
- **Weakening the kept DFast miss-seeding heuristic is not a speed-recovery path here.**
  Also on `74e9910`, raising the extra midpoint-seed threshold from skipped
  runs of `8+` bytes to `16+` nudged level-3 ratio up slightly
  (`2.121 -> 2.127`) but cut compression from `204.7/186.5 MB/s` to
  `201.9/182.4 MB/s` on levels `3/4`. That says the current miss-seeding cost
  is not dominated by midpoint insertion on moderate jumps; changing the
  threshold just moved the tradeoff in the wrong direction.
- **Behavior-preserving arithmetic cleanup in DFast reinsertion is below the gate.**
  Still on `74e9910`, removing `checked_sub`/`Option::map`/`next_multiple_of`
  from `skip_dfast()` and `skip_dfast_miss_positions()` kept Silesia ratio flat
  but only moved compression from `204.7/186.5` to `204.5/186.4 MB/s`, while
  level-3 decompression slipped from `461.2` to `456.6 MB/s`. The `skip_dfast`
  hotspot reflects real reinsertion work, not just helper arithmetic; future
  DFast speed recovery still needs a larger structural cut.

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
- Retrying implicit-final direct Huffman headers without the full FSE-compressed-weight path is still below the gate. On `b8376e9`, allowing 129 active symbols via omitted final direct weights nudged Silesia ratio only from `1.216/2.121/2.822/3.167` to `1.219/2.122/2.827/3.172` on levels `1/3/9/19`, while compression slipped to `729.4/203.6/21.0/3.2 MB/s` from `748.5/203.9/20.7/3.3 MB/s` and decompression also dipped slightly. The +1-symbol direct-header extension alone does not unlock a visible win on this branch; future literal-header work should skip straight to full FSE-compressed weights or a larger block-level decision change.
- Abandoning raw-result compressed blocks is not a cross-cutting keep by itself. On `f87b074`, returning `None` early when the compressed-block path found no sequences or fell back to raw literals improved Silesia level-1 compression from `749.1 -> 851.9 MB/s`, but level `3` only moved to `207.6 MB/s`, level `9` stayed at `20.9 MB/s`, level `19` slipped to `3.2 MB/s`, and decompression regressed across the spread (`1667.3 -> 1564.6 MB/s` at level `1`). That shortcut seems to help the already-fast level-1 path more than the levels where the repo is actually behind; future pass-removal work should target long-file parser or block-assembly costs that matter beyond `Fast`.
- Encoder-side repeat offsets are not a free cross-cutting ratio win on this branch. On `281d54f`, teaching the block encoder to emit repeat-offset codes and threading the repeat-offset state across compressed blocks improved Silesia ratio only modestly (`1.216 -> 1.218`, `2.121 -> 2.134`, `2.822 -> 2.853`, `3.167 -> 3.201` on levels `1/3/9/19`), while compression regressed across the same spread (`749.4 -> 728.1 MB/s` at level `1`, `203.8 -> 202.0 MB/s` at level `3`, `21.0 -> 20.7 MB/s` at level `9`, `3.3 -> 3.2 MB/s` at level `19`) and decompression also dipped by roughly `1.4-2.1%`. The format feature is interoperable, but without a larger parse/cost-model change it does not clear the current gate; future repeat-offset work should be paired with sequence scoring or literal/sequence coding changes that can convert the new codes into a visibly larger ratio gain.

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

- **Repeat-offset-aware parser scoring still does not clear the cross-cutting gate by itself.**
  On `5288486`, retrying repeat offsets with both cross-block repcode emission
  and parser-side repeat-aware offset pricing nudged Silesia ratio from
  `1.216 -> 1.218` at level `1`, `2.121 -> 2.134` at level `3`, and
  `2.822 -> 2.854` at level `9`, but compression slipped from
  `751.8 -> 725.9 MB/s`, `204.2 -> 195.6 MB/s`, and `20.8 -> 20.5 MB/s`
  respectively, with decompression also down about `1-2%`. The extra ratio is
  real but still too small for the speed bill; future repeat-offset work needs
  a larger parse-quality change than simply repricing near-tie matches.
- **Even level-gated Optimal BT repcode pricing is still below the bar on this head.**
  On `d14e5a1`, threading the current repeat-offset state into the `Optimal BT`
  chunk DP and pricing repcode-eligible matches along the cheapest path nudged
  Silesia ratio only from `3.133 -> 3.136` at level `16`, `3.194 -> 3.199` at
  level `18`, and `3.201 -> 3.207` at level `19`, while compression slipped
  from `5.2 -> 5.1 MB/s`, `4.0 -> 3.9 MB/s`, and `3.2 -> 3.1 MB/s` and
  decompression regressed by about `0.4%` at level `16` and `4.8%` at levels
  `18/19`. The repeat-offset feature already harvested the easy gain when the
  encoder started emitting repcodes on levels `16-19`; parser-side repricing
  alone still does not unlock a visible enough ratio win. Future high-level
  ratio work should look for better candidate generation or literal coding
  gains, not more local repcode cost-model tuning.
- **BtLazy2 matched-run BT reinsertion is actively bad on this head.**
  On `e603b9f`, reinserting accepted BtLazy2 match interiors back into the
  binary tree with sparse `bt_find_insert()` calls cratered Silesia level-13
  ratio from `2.900 -> 2.812` and compression from `17.6 -> 9.9 MB/s`, with
  decompression also down about `11%`. This is worse than a simple speed/ratio
  trade; the extra inserts appear to perturb the BT state enough to damage parse
  quality. Future BtLazy2 work should avoid broad interior reinsertion unless it
  can be done with a much cheaper, more tree-stable insertion path.
- **BtLazy2 extra local probing remains below the bar.**
  Also on `e603b9f`, extending BtLazy2 lookahead from 2 to 3 positions improved
  Silesia ratio only from `2.900 -> 2.908` at level `13` and
  `2.910 -> 2.919` at level `15`, while compression fell from `17.6 -> 14.9`
  and `16.3 -> 13.9 MB/s`. A separate retry that raised BtLazy2 search depth by
  one notch only reached `2.910` ratio at level `13`, left level `15` flat, and
  still cost about `5.7%` compression at level `13`. Conclusion: the remaining
  BtLazy2 ratio gap is not going to close from one more local probe or one more
  search-depth notch; future wins likely need a different parser structure or a
  coding-side improvement.
- **BtLazy2 literal-aware local rescoring made the parse strictly worse.**
  On `da39c39`, replacing the simple BtLazy2 `prefer_match()` heuristic with a
  literal-bit-aware local cost estimate and selectively enabling a third
  lookahead for weak initial matches immediately regressed Silesia level `13`
  from `2.900 -> 2.864` ratio, `17.5 -> 14.0 MB/s` compression, and
  `440.1 -> 406.0 MB/s` decompression before level `15` even ran. The added
  local cost model over-penalized skipped literals and disrupted the parse more
  than the extra probe helped. Future BtLazy2 work should look for better
  candidate generation inside `bt_find_insert()` or a coding-side ratio gain,
  not more aggressive local match rescoring.
- **Persisting Huffman decode tables inside the table object regressed the overall pipeline.**
  On `7de3308`, storing the per-table Huffman decode lookup inside
  `HuffmanTable` looked like a plausible decompression win, but the weighted
  benchmark moved from `1239.9 -> 1182.3 MB/s` on compress and
  `4941.0 -> 4904.3 MB/s` on decompress with flat ratio. The likely issue is
  that every table construction/copy now drags a heap allocation and a larger
  object through the literals path, overwhelming the saved rebuild. Future
  Huffman decode caching should keep the hot decode structure out of the
  frequently-built table object, or reuse it at a higher level without adding
  per-table ownership and cloning cost.
- **Per-literals-section Huffman decode-table reuse is still below the gate.**
  Also on `7de3308`, reusing one Huffman decode lookup across an entire
  literals section and across all four Huffman streams avoided the larger
  `HuffmanTable` footprint, but the weighted benchmark still moved only to
  `4947.9 MB/s` on decompress (`+0.14%`) while weighted compress slipped from
  `1239.9 -> 1193.5 MB/s`. That is not a visible cross-cutting improvement.
  Conclusion: repeated Huffman lookup-table rebuilds are not a dominant decoder
  cost on this head; future decompression work should keep targeting larger
  copy/state-management paths rather than local literals-table reuse.
- **Lazy-family repeat offsets are cheap enough to keep when level-gated.**
  On `ba0b5ed`, enabling the existing repeat-offset encoder path for
  `Lazy`/`Lazy2` levels (`6-12`) improved Silesia ratio from
  `2.588 -> 2.612` at level `6`, `2.793 -> 2.824` at level `8`, and
  `2.957 -> 2.987` at level `12`, while leaving compression effectively flat
  (`64.4 -> 64.1 MB/s`, `25.2 -> 25.2/24.9 MB/s` on rerun, `6.3 -> 6.3 MB/s`)
  and only trimming decompression by about `0.5-0.7%`. Weighted sanity also
  improved slightly on compress (`1198.6 -> 1224.0 MB/s`) with flat ratio.
  The earlier all-level repeat-offset failure was mostly a fast-level budget
  problem; the mid-level Lazy family can absorb the format change.
- **Level 5 still cannot buy ratio with repeat offsets alone.** On `db7612c`,
  enabling repeat-offset emission for `Greedy` improved Silesia level-5 ratio
  from `2.588 -> 2.612`, but compression fell from `64.6 -> 63.5 MB/s` and
  decompression slipped from `361.0 -> 357.6 MB/s`. A follow-up that simply
  retuned level `5` onto the level-6 `Lazy` parameters reproduced the same
  `2.612` ratio while still landing at `64.2 MB/s` and `63.9 MB/s` on rerun.
  Conclusion: the remaining level-5 gap is not just "missing repcodes" or
  "wrong family label"; Greedy needs a parser-quality or block-assembly change
  that improves ratio without paying even a 1% speed tax.
- **Direct-only implied-final Huffman headers are still below the bar on this head.**
  On `043d757`, retrying direct Huffman header serialization with the final
  implied weight omitted nudged Silesia ratio only from
  `1.216/2.121/2.853/3.201` to `1.219/2.122/2.858/3.205` on levels `1/3/9/19`,
  with compression effectively flat and decompression regressing by about
  `0.5-4.1%`. That is still a sub-visible literal-header gain, not a real
  ratio recovery. Future literal work should skip straight to full
  FSE-compressed Huffman weights or a larger block/literal decision change.
- **Strategy-gated larger Optimal BT DP windows still need a faster evaluation loop.**
  On `5f2fbd2`, a local retry that kept `BtOpt` at a 512-position DP chunk but
  widened `BtUltra` to `768` and `BtUltra2` to `1024` improved the weighted
  sanity benchmark slightly (`1223.9 -> 1241.2 MB/s` compress,
  `4912.5 -> 4942.4 MB/s` decompress) with flat weighted ratio, but the
  high-level Silesia reruns were too slow to complete within the experiment
  budget. Since the patch specifically targets levels `16-19`, weighted alone
  is not trustworthy enough to keep. Future work here should first build a
  faster high-level measurement loop (for example a smaller-file or
  reduced-corpus confirmation step) before retrying larger DP horizons beyond
  the kept global `512`.
- **Literal-aware pricing alone does not move Optimal BT parses.** On
  `b68f308`, threading the pending literal-run length through the kept
  512-position Optimal BT DP and charging approximate literal-length extra bits
  left Silesia ratio pinned at `3.149/3.214/3.228` on levels `16/18/19`, with
  compression also unchanged at `5.2/4.0/3.2 MB/s`. The current high-level
  ratio gap is not just "the DP underprices long literal runs" when it still
  sees only the same single best-offset candidate per position. Future
  high-level parser work should focus on exposing better candidate sets or more
  exact repcode-aware state, not just refining the cost model on the existing
  candidate stream.
- **Equal-length BT closer-offset tie-breaking is below the bar.** Also on
  `b68f308`, making the BT finder prefer the smaller offset when two
  candidates had the same length kept Silesia ratio flat at
  `3.149/3.214/3.228` while slowing compression from `5.2 -> 5.1 MB/s`,
  `4.0 -> 3.9 MB/s`, and `3.2 -> 3.1 MB/s` on levels `16/18/19`. The missing
  high-level ratio is therefore not coming from arbitrary equal-length offset
  choice inside the current BT traversal; future work needs bigger leverage
  than local tie-breaking among already-found matches.
- **Backward-extending emitted Optimal BT matches is not a free ratio win.** On
  `e53390b`, letting the Optimal BT DP realization pull a chosen match backward
  across the immediately preceding literal run only nudged Silesia ratio from
  `3.149/3.214/3.228` to `3.152/3.218/3.233` on levels `16/18/19`, while level
  `19` decompression fell sharply from `462.8 -> 442.3 MB/s`. Even when the
  parser keeps the same candidate set, changing the realized match boundaries
  can reshape the sequence stream enough to hurt decode speed without producing
  a visible ratio gain. Future high-level work should be careful about
  post-DP match surgery unless it clearly changes the parser's candidate
  quality, not just literal/match boundaries.
- **BtUltra2-only wider DP windows are still below the visibility threshold.**
  Also on `e53390b`, widening only the `BtUltra2` DP chunk from `512` to `768`
  positions improved level-19 Silesia from `3.228 -> 3.238` ratio with
  `3.2 -> 3.3 MB/s` compression and `462.8 -> 463.9 MB/s` decompression, but a
  `+0.010` aggregate ratio move on the only affected level is still too small
  to count as visible. The kept global `512` window likely already captured
  most of the practical DP-horizon win; future Optimal BT work should look for
  better candidates per position rather than one more level-specific window
  increase.
- **Lazy-family target-length-bounded chain walks are too blunt.** On
  `586be84`, stopping `find_match()` as soon as Lazy/Lazy2 found any match at
  or above the parser `target_length` accelerated Silesia compression from
  `64.3 -> 79.6 MB/s` at level `6` and `20.3 -> 25.8 MB/s` at level `8`, but
  it cratered level-6 ratio from `2.612 -> 2.390` and still nudged levels
  `8/12` ratio down to `2.793/2.973`. The full chain walk is still finding
  materially better matches than the first "good enough" candidate on this
  branch; future Lazy-family speed work should cut reinsertion cost or miss
  handling before it short-circuits match-quality search that aggressively.
- **Removing the decoder's reserve pre-sum is below the real-world bar.** Also
  on `586be84`, threading total decoded match bytes out of sequence decode to
  avoid `execute_sequences()` summing match lengths in a second pass looked
  attractive in a CPU profile (~10% of samples on `dickens`/level 6), but
  exact Silesia decompression still regressed from `1683.7 -> 1659.6 MB/s` at
  level `1`, `456.9 -> 450.4 MB/s` at level `3`, and `380.4 -> 373.6 MB/s` at
  level `9`, with ratio flat and compression unchanged. The extra metadata and
  plumbing cost more than the removed iterator walk here; future decoder work
  should stay focused on bitreader/state-transition overhead or larger copy-path
  cuts, not bookkeeping that only moves one pre-pass.
- **DFast small-offset reinsertion pruning is not a safe speed-recovery shortcut on this head.**
  On `f6eb61d`, making `skip_dfast()` sparser only for long, very small-offset
  matches looked like a targeted way to cut the matched-run maintenance that
  profiling still shows at ~14% of level-3 samples, but exact Silesia kept
  ratio flat at `2.121/2.214` and dropped compression from `187.1 -> 181.5
  MB/s` at level `4` while leaving level `3` effectively unchanged
  (`202.3 -> 202.5 MB/s`). Even when the heuristic only touches the most
  redundant-looking runs, this branch still needs the existing reinsertion
  density to hold its DFast floor.
- **No-repeat offset-code specialization is below the gate for DFast.**
  Also on `f6eb61d`, splitting the sequence collector so non-repeat-offset
  levels could bypass the repeat-offset update machinery seemed plausible after
  a profile put `offset_code` at about `6%` of level-3 samples, but Silesia
  ratio stayed flat and compression slipped from `202.3 -> 200.0 MB/s` at
  level `3` with level `4` only reaching `186.4 MB/s` versus the `187.1 MB/s`
  baseline. The collector-side offset-code work is measurable, but not large
  enough in isolation to recover the DFast speed debt.
- **Pre-reserving sequence/FSE output buffers is also below the DFast gate.**
  Still on `f6eb61d`, reserving the sequence-section output `Vec` and the FSE
  `BitWriter` buffer up front improved Silesia compression only from
  `202.3 -> 205.9 MB/s` at level `3` while regressing level `4` from
  `187.1 -> 185.0 MB/s`, with ratio unchanged and decompression slightly down.
  The profile's realloc noise is real but too small and uneven to clear the
  subsystem gate; future DFast work should keep aiming at parser probe count or
  matched-run maintenance structure, not local buffer growth.
- **Greedy chain-walk indexing cleanup is real but still below the gate.**
  On `9c096c6`, a level-5 profile on `dickens` was dominated by slice indexing
  inside `MatchFinder::find_match`, so a first retry removed bounds-checked
  `Vec` indexing from the Greedy insert/chain-walk hot path. Exact Silesia kept
  ratio flat at `2.588` and only improved compression from `63.1 -> 64.0 MB/s`
  with decompression also essentially flat (`360.4 -> 361.7 MB/s`). The hotspot
  is real, but the isolated indexing cleanup still landed inside noise on
  Silesia; future level-5 work should look for larger parser or block-assembly
  cuts than just unchecked access in the existing walk.
- **A Greedy-specific fused matcher still stayed below the visibility bar.**
  Also on `9c096c6`, replacing the generic level-5 `find_match()` call with a
  Greedy-only fused insert-plus-chain-walk helper preserved the same parse but
  still only reached `64.0 MB/s` versus the `63.1 MB/s` baseline, with flat
  ratio and a small decompression slip (`360.4 -> 359.4 MB/s`). That makes two
  misses in the same hot-loop-cleanup family. The next Greedy retry should
  pivot away from compiler-shaping or helper-fusion changes and toward a larger
  parser-quality or block-decision lever.
- **BitReader reload is a real decompression lever when the slice-copy staging is removed.**
  On `4fc4c6f`, a cross-cutting decoder pass replaced the `BitReader`'s
  repeated 8-byte `copy_from_slice` staging with direct unaligned window loads
  on the common path, keeping the short-tail fallback only for partial windows.
  Exact Silesia kept ratio flat and improved decompression from
  `1629.5 -> 1736.1 MB/s` at level `1`, `456.7 -> 522.1 MB/s` at level `3`,
  `378.6 -> 428.3 MB/s` at level `9`, and `455.7 -> 520.2 MB/s` at level `19`,
  with only small compression movement and weighted sanity also slightly up on
  decompress (`4937.9 -> 4953.7 MB/s`). The earlier decoder wins from reducing
  copy/replay work were not exhausted; bitstream window reload overhead was
  another material cross-cutting bottleneck on this head.
- **Direct pointer replay on already-reserved output is another real decoder win.**
  On `3f56ea9`, replacing `extend_from_within()` inside `copy_match()` with a
  direct pointer copy into the `Vec`'s spare capacity preserved the existing
  chunked-overlap replay logic while avoiding repeated reserve/bounds
  machinery that still dominated the post-`BitReader` profile. Exact Silesia
  kept ratio flat and improved decompression from `1629.5 -> 1763.8 MB/s` at
  level `1`, `456.7 -> 523.9 MB/s` at level `3`, `378.6 -> 431.9 MB/s` at
  level `9`, and `455.7 -> 525.4 MB/s` at level `19`, with weighted
  decompression also up slightly (`4937.9 -> 4960.1 MB/s`) and full tests
  passing. The remaining decoder headroom is still in copy/state-management
  paths rather than literal ownership tricks or metadata pre-passes.
- **A conditional `read_bits()` reload fast path regressed badly against the current decoder head.**
  On `ef316af`, making `BitReader::read_bits()` skip the `reload()` call until
  `bits_consumed >= 8` looked like a plausible follow-up after the direct
  window-load keep, but exact Silesia on levels `1/3/9` came back well below
  the current head: decompression fell from `1763.8 -> 1653.8 MB/s` at level
  `1`, `523.9 -> 463.7 MB/s` at level `3`, and `431.9 -> 386.2 MB/s` at level
  `9`, with level `19` still running when the experiment was stopped. The
  extra hot-path branch costs more than the avoided helper call once the
  window load itself is already cheap; future `BitReader` work should avoid
  per-read conditionals and look for larger sequence-decode or literal-copy
  cuts instead.
- **Direct pointer copying for literal replay is below the bar after the kept decoder wins.**
  On `454cdde`, replacing `extend_from_slice()` for literal runs inside
  `execute_sequences()` with direct pointer writes into the already-reserved
  output buffer preserved behavior and matched the kept match-replay strategy,
  but exact Silesia stayed effectively flat against the current head:
  decompression moved from `1763.8 -> 1755.2 MB/s` at level `1`,
  `523.9 -> 522.2 MB/s` at level `3`, and `431.9 -> 433.5 MB/s` at level `9`,
  with level `19` still pending when the run was stopped. The remaining
  literal-copy overhead is too small once reserve sizing and match replay are
  already cheap; future decoder work should look for larger sequence-decode or
  block-level copy cuts rather than another `extend_from_slice()` rewrite.
- **Deferring `set_len()` inside chunked match replay regressed instead of helping.**
  On `bd37e78`, a follow-up to the kept pointer-based match replay tried to
  remove the per-chunk `Vec::set_len()` updates and advance the output length
  only once after the whole match had been copied. Exact Silesia on levels
  `1/3/9` came back consistently worse than the current head:
  decompression fell from `1763.8 -> 1743.7 MB/s` at level `1`,
  `523.9 -> 512.8 MB/s` at level `3`, and `431.9 -> 420.1 MB/s` at level `9`,
  with level `19` still pending when the run was stopped. The current replay
  loop benefits from publishing each copied chunk immediately so later overlap
  iterations can stay aligned with the `Vec` length; future replay work should
  pivot away from `set_len` reshaping and toward a different structural cut.
- **Lazy/Lazy2 matched-run reinsertion is already close to its ratio floor on this head.**
  On `b4b9820`, shortening the dense prefix/suffix in `skip()` from `8` bytes
  to `4` and widening the mid-run reinsertion step for `Lazy`/`Lazy2` looked
  like a plausible way to recover parser speed without touching the chain-walk
  itself, but exact Silesia regressed immediately: ratio slipped from
  `2.612 -> 2.610` at level `6` and `2.824 -> 2.821` at level `8`, while
  compression also fell from `62.2 -> 61.9 MB/s` and `23.8 -> 23.5 MB/s`.
  The current mid-level parser is still relying on those reinsertion points to
  recover later matches, so future Lazy-family work should target miss
  handling or cheaper search quality, not sparser matched-run table seeding.
- **Lazy `insert()` dominates the profile, but helper-fusion cleanup is still below the gate.**
  On `d57a7f8`, a fresh level-6 `dickens` CPU profile put `MatchFinder::insert`
  at about `82.6%` of leaf samples, so a follow-up folded the current-position
  hash/load directly into `find_match()` to avoid a separate `insert()` helper
  round-trip on every probe. Exact Silesia stayed effectively flat:
  compression moved from `62.2 -> 62.6 MB/s` at level `6` and
  `23.8 -> 23.9 MB/s` at level `8`, with ratio unchanged and decompression
  slightly down. The hotspot is real, but the isolated call-shaping cleanup is
  still too small to survive the real-corpus bar; future Lazy-family speed
  work should look for fewer probes or fewer insertions, not just a tighter
  implementation of the same per-position work.
