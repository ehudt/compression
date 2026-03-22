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
- **Random-data fast path was the single biggest win.** Sampling-based
  incompressible detection took random/1 compress from ~48 MiB/s to ~9.8 GiB/s
  (200x). This is a structural shortcut, not a micro-optimization.
- **Decoder wins come from reducing copies.** Reading sequence back-references
  directly from output (instead of cloning history per block) improved Silesia
  decompress by ~3-5% across all levels and improved repetitive roundtrip by
  13-17%.
- **Block-level RLE matters.** Emitting RLE blocks for single-byte content
  improved all_zeros compress by ~35% and decompress from ~678 MiB/s to
  ~12 GiB/s (18x).

## Remove a whole compress-side pass at level 3

**Status:** Active research direction
**Context:** See strategic learnings above. The weighted benchmark is a useful
signal for fast iteration, but Silesia confirmation is required before keeping.

**Strong candidates:**
- Stream LZ77 output directly into sequence/literal collection (avoid
  materializing intermediate parse events -- tried once as "stream parse events
  directly" and it regressed; needs a different approach)
- Reuse block scratch buffers across blocks (tried as "preallocate parse events
  and block literal/sequence buffers" -- regressed; may need profile-guided
  approach to find the real allocation hotspots)
- Smarter early raw-block fallback that preserves ratio on structured inputs

**Already tried and failed:**
- Lower LZ77 search depth from 8 to 6 (regressed repetitive/binary_structured)
- Shorter incompressible sample window (regressed repetitive/binary_structured)
- Early exit after good-enough match length (regressed compress speed)
- Sparser reinsertion step 12 for long skips (noisy, ambiguous gains)
- Step 16 reinsertion for long matches (regressed compress by ~5%)

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
