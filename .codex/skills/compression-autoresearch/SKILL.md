---
name: compression-autoresearch
description: Use when the goal is autonomous research on this repo's compression engine, especially to improve compression ratio and throughput through repeated code changes, fast Criterion benchmarks, targeted profiling, and correctness/interop gates.
---

# Compression Autoresearch

Run an autonomous experiment loop for this repository's Rust zstd implementation. The research target is the two-way tradeoff that matters here:

- Better compression ratio
- Better throughput

Never optimize one while silently breaking the other, and never keep a change that compromises frame correctness or interoperability.

## Repo-Specific Scope

Read these files before changing code:

- `README.md` for the intended development loop and profiling commands
- `benches/compression.rs` for the fast benchmark cases and full sweep mode
- `tests/acceptance.rs` for the interop contract with the system `zstd`
- `tests/integration.rs` for round-trip and sanity expectations

Likely hot paths / leverage points:

- `src/encoder/lz77.rs` for match finding
- `src/encoder/block.rs` for block construction and sequence emission
- `src/huffman.rs` and `src/fse.rs` for entropy coding
- `src/frame.rs` for block/frame-level decisions
- `src/decoder/*` only when a format or round-trip change requires it

Do not start by editing the benchmark harness or tests unless the task is explicitly about measurement or missing coverage.

## Setup

1. Propose a fresh branch name of the form `autoresearch/<tag>` and create it from the current mainline.
2. Confirm the worktree is otherwise clean enough to experiment safely.
3. Use the tracked `results.tsv` file as the default experiment log.
4. Create a unique temp log directory outside the repo, for example with `LOG_DIR=$(mktemp -d /tmp/compression-autoresearch-XXXXXX)`, and write all benchmark/test logs there.
5. Download the Silesia corpus (needed for integration tests to compile):
   ```bash
   cargo run --release --example silesia_bench -- --download --implementation ours --levels 1 2>/dev/null
   ```
   This populates `~/silesia/` which `tests/integration.rs` depends on at compile time via `include_bytes!`.
6. Establish the baseline before making any code changes.

Suggested TSV header:

```tsv
commit	status	ratio_notes	compress_notes	decompress_notes	roundtrip_notes	description
```

Free-form notes are acceptable because Criterion output is textual and case-based; do not force a fake single scalar if the data does not justify one.

## Baseline Commands

Run the baseline exactly as the repo is today:

```bash
LOG_DIR=$(mktemp -d /tmp/compression-autoresearch-XXXXXX)
cargo bench --bench weighted > "$LOG_DIR/bench.log" 2>&1
cargo test --test acceptance -- --nocapture > "$LOG_DIR/acceptance.log" 2>&1
```

For thorough validation of a promising change, use the full Silesia run with our
implementation only:

```bash
cargo run --release --example silesia_bench -- \
  --download \
  --implementation ours \
  > "$LOG_DIR/silesia.log" 2>&1
```

Use the weighted benchmark as the default signal for iterating during research.
It produces three composite scores across 8 data categories (text, json, xml,
source code, database, executable, medical image, random), weighted by
real-world frequency:

- **weighted_ratio** — weighted average compression ratio (lower is better)
- **weighted_compress_mb_s** — weighted average compression throughput
- **weighted_decompress_mb_s** — weighted average decompression throughput

Only when you are ready to commit/keep a change, run the Silesia benchmark to
get the official numbers for the record.

Useful extracts:

```bash
grep -A5 "\\[weighted-benchmark\\]" "$LOG_DIR/bench.log"
tail -n 40 "$LOG_DIR/acceptance.log"
```

## Acceptance Criteria — Two-Gate System

Think BIG. Do not keep micro-optimizations. The goal is meaningful algorithmic
wins, not shuffling code for a 0.5% blip.

### Gate 1: Weighted benchmark (fast iteration signal)

A change is **promising enough to validate** only if ALL of these are true:

- `cargo bench --bench weighted` completes successfully
- `cargo test --test acceptance -- --nocapture` passes, or skips only because `zstd` is unavailable
- **At least one** composite score (weighted_ratio, weighted_compress_mb_s,
  weighted_decompress_mb_s) improves by **≥ 3%** compared to the current baseline
- **No** composite score regresses by **> 1%**

If the weighted benchmark shows < 3% improvement on every metric, **discard
the change immediately**. Do not run Silesia. Do not keep it "just in case."
Go back to the drawing board and try a bigger idea.

If a change is ambiguous or borderline, run the full weighted sweep for a
clearer signal before deciding:

```bash
ZSTD_RS_FULL_BENCHES=1 cargo bench --bench weighted > "$LOG_DIR/bench-full.log" 2>&1
```

### Gate 2: Silesia benchmark (real-data confirmation)

Only run this gate after a change clears Gate 1. Run the full Silesia
benchmark with our implementation:

```bash
cargo run --release --example silesia_bench -- \
  --download \
  --implementation ours \
  > "$LOG_DIR/silesia.log" 2>&1
```

Keep the change only if:

- The Silesia benchmark completes successfully
- The **same metric** that triggered Gate 1 shows **≥ 2% improvement** on the
  real Silesia corpus compared to the baseline Silesia run
- No other metric regresses by > 1% on Silesia

If Silesia does not confirm the improvement at ≥ 2%, **discard the change**.
The weighted benchmark was a false signal — move on.

### Tradeoff rules

- A ratio win on `repetitive` or `binary_structured` data is valuable only if throughput does not regress badly on the fast cases.
- A throughput win is valuable only if compressed size does not materially worsen on compressible inputs.
- Regressing `random/1` badly is usually a red flag, because it exercises the near-incompressible path.
- Simpler code wins ties.

## Profiling Loop

Use profiling when the fast benchmark shows a regression, a suspicious plateau, or a likely hotspot.

### Benchmark profiling

```bash
ZSTD_RS_PROFILE_BENCHES=1 cargo bench --profile profiling --features profiling --bench compression
```

This writes per-benchmark flamegraph artifacts under Criterion output directories.

### CLI profiling

Use the CLI path when you want a focused compress or decompress hotspot on a real file:

```bash
cargo run --profile profiling --features profiling -- \
  --profile-cpu profiles/compress.svg \
  --profile-repeat 200 \
  --profile-min-ms 500 \
  --profile-hz 1000 \
  compress 3 input.txt output.zst
```

### Test profiling

```bash
mkdir -p profiles/tests
ZSTD_RS_PROFILE_TESTS=profiles/tests \
  cargo test --profile profiling --features profiling --test integration -- --nocapture
```

Profile first, then change code. Do not cargo-cult micro-optimizations.

## Experiment Loop

Loop autonomously once setup is complete:

1. Inspect the current commit and the last accepted result.
2. Choose one concrete idea — think big, aim for algorithmic wins.
3. Edit only the code needed for that idea.
4. Run the weighted benchmark and save the log.
5. **Gate 1 check**: compare composite scores against the baseline.
   - If any score regresses > 1%, or no score improves ≥ 3% → **discard immediately**.
   - If ≥ 3% improvement with no regression > 1% → proceed.
6. Run acceptance tests (`cargo test --test acceptance -- --nocapture`).
7. **Gate 2**: run the full Silesia benchmark with `--implementation ours`.
   - If the improved metric does not confirm ≥ 2% on Silesia → **discard**.
   - If confirmed → keep.
8. If needed, run `cargo test` or the full benchmark sweep for extra confidence.
9. Log the outcome in `results.tsv`, including the percentage changes.
10. Keep the commit only if it cleared both gates; otherwise revert to the previous accepted commit.

The very first run is always the untouched baseline.

## Good Experiment Ideas

- Improve LZ77 match search quality without exploding search cost
- Reduce wasted work in candidate scanning, chain traversal, or lazy matching
- Make block decisions smarter: raw vs RLE vs compressed
- Improve literal or sequence coding decisions when entropy work is not paying off
- Reduce allocator churn, copying, or temporary buffer traffic in hot paths
- Special-case obvious incompressible inputs so they exit quickly

## Bad Experiment Ideas

- Chasing tiny wins by adding large amounts of brittle code
- Breaking zstd interoperability to get prettier ratios
- Changing tests or benches to hide regressions
- Treating one corpus as the only truth
- Keeping a change because one sample improved while the rest got worse

## Decision Heuristics

Prefer this order of evidence:

1. Correctness and interoperability
2. Weighted benchmark scores (ratio, compress MB/s, decompress MB/s)
3. Full Silesia results for our implementation (final gate before committing)
4. Ratio table changes on compressible inputs
5. Profiling evidence that explains the result
6. Full benchmark sweep when the fast signal is promising but incomplete

If you cannot explain a result, do not trust it yet.

## Logging

Use one TSV row per experiment, including discarded ideas. Suggested status values:

- `baseline`
- `keep`
- `discard`
- `crash`

Descriptions should be brief and concrete, for example:

- `baseline`
- `shorter hash-chain walk on random path`
- `reuse literal scratch buffer`
- `raw-block fallback earlier for incompressible input`

## Safety

- Redirect long command output to log files; do not flood context with raw Criterion output.
- Write temp benchmark/test logs under `/tmp` or another untracked directory, not in the repo root.
- Do not commit `bench.log`, `bench-full.log`, `acceptance.log`, `silesia.log`, or profile artifacts.
- If acceptance tests fail because `zstd` is missing, say so explicitly; that weakens confidence in any keep/discard decision.
- If a change touches frame encoding, literals, sequences, or checksums, run broader tests before keeping it.

## Stop Condition

The loop is autonomous. Do not stop to ask whether to continue once the user has started a research run. Keep iterating until the user interrupts or a hard blocker makes further progress non-credible.
