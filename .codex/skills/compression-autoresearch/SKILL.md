---
name: compression-autoresearch
description: Use when the goal is autonomous research on this repo's compression engine, especially to improve compression ratio and throughput through repeated code changes, fast Criterion benchmarks, targeted profiling, and correctness/interop gates.
---

# Compression Autoresearch

Run an autonomous experiment loop for this repository's Rust zstd implementation. The research target is the ratio/throughput tradeoff that matters here:

- Better compression ratio
- Better throughput

Never accept an unbudgeted tradeoff. Allowed regressions depend on the target
subsystem and must match `docs/level-subsystem-baselines.md`. Never keep a
change that compromises frame correctness or interoperability.

## Repo-Specific Scope

Read these files before changing code:

- `README.md` for the intended development loop and profiling commands
- `docs/benchmarking.md` for benchmark modes, Silesia usage, and where to find baseline context
- `docs/level-subsystem-baselines.md` for the current subsystem-level tradeoff expectations and reference comparisons
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

1. **Stay on the current branch.** Do not create, switch to, or push any branch. All commits go directly onto whatever branch is checked out when the run starts. Do not merge or push anywhere. If run 10 times in a row, the result should be 10 commits stacked on the starting HEAD.
2. Confirm the worktree is otherwise clean enough to experiment safely.
3. Use the tracked `results.tsv` file as the default experiment log.
4. Create a unique temp log directory outside the repo:
   ```bash
   LOG_DIR=$(mktemp -d /tmp/compression-autoresearch-XXXXXX)
   ```
   Write all benchmark/test logs there.
5. Download the Silesia corpus (needed for integration tests to compile):
   ```bash
   cargo run --release --example silesia_bench -- --download --implementation ours --levels 1 2>/dev/null
   ```
   This populates `~/silesia/` which `tests/integration.rs` depends on at compile time via `include_bytes!`.
6. Establish the baseline before making any code changes (see Baseline Commands).
7. Read `ideas.md` for promising directions and warnings from previous sessions.

Suggested TSV header:

```tsv
commit	status	ratio_notes	compress_notes	decompress_notes	roundtrip_notes	description
```

Free-form notes are acceptable because Criterion output is textual and case-based; do not force a fake single scalar if the data does not justify one.

## Baseline Commands

Run the baseline exactly as the repo is today:

```bash
cargo bench --bench weighted > "$LOG_DIR/bench.log" 2>&1
cargo test --test acceptance -- --nocapture > "$LOG_DIR/acceptance.log" 2>&1
```

The weighted benchmark is the default signal for iteration. It produces three composite scores across 8 data categories (text, json, xml, source code, database, executable, medical image, random), weighted by real-world frequency:

- **weighted_ratio** — weighted average compression ratio (lower is better)
- **weighted_compress_mb_s** — weighted average compression throughput
- **weighted_decompress_mb_s** — weighted average decompression throughput

Useful extracts:

```bash
grep -A5 "\\[weighted-benchmark\\]" "$LOG_DIR/bench.log"
tail -n 40 "$LOG_DIR/acceptance.log"
```

## Acceptance Criteria — Default Flow and Validation Gates

Think BIG. Do not keep micro-optimizations. The goal is meaningful algorithmic
wins, not shuffling code for a 0.5% blip.

All comparisons are against the **original baseline** established in setup, never against intermediate stacked states.

### Gate 1: Fast Signal

For repo-wide work, the weighted benchmark is the primary fast gate. For
intentionally level-specific work, targeted per-level benchmarks are the
primary fast gate, and the weighted benchmark is used to measure repo-wide
spillover.

Repo-wide changes are **promising enough to validate** only if ALL of these are
true:

- `cargo bench --bench weighted` completes successfully
- `cargo test --test acceptance -- --nocapture` passes (or skips only because `zstd` is unavailable)
- **At least one** composite score improves by **>= 3%**
- **No** composite score regresses by **> 1%**

For intentionally level-specific work, use this gate as the default fast signal,
not as an absolute veto. Judge the change first against targeted per-level
benchmarks and `docs/level-subsystem-baselines.md`, then use the weighted
benchmark to understand repo-wide spillover.

If improvement is < 3% on every metric, **discard immediately** for repo-wide
changes — do not run Silesia. Exception: if improvement is 1-3%, enter the
**stacking phase** (see below). For intentionally level-specific changes, a
flat weighted result may still be worth validating if the targeted per-level
results clearly match the subsystem policy.

If a change is ambiguous or borderline, run the full weighted sweep for a clearer signal:

```bash
ZSTD_RS_FULL_BENCHES=1 cargo bench --bench weighted > "$LOG_DIR/bench-full.log" 2>&1
```

### Gate 2: Silesia benchmark (reference comparison by level)

Run this after a change is promising under Gate 1 or under targeted per-level
validation for the affected subsystem:

```bash
cargo run --release --example silesia_bench -- \
  --download \
  --implementation both \
  --levels <affected-levels> \
  > "$LOG_DIR/silesia.log" 2>&1
```

Keep the change only if:

- The Silesia benchmark completes successfully
- The targeted levels move in the intended direction relative to reference
  `zstd`, according to `docs/level-subsystem-baselines.md`
- Any regressions outside the intended tradeoff budget are understood and
  acceptable

If Silesia does not confirm the intended subsystem movement relative to
reference, **discard**.

Typical commands:

```bash
# Repo-wide checkpoint
cargo run --release --example silesia_bench -- \
  --download \
  --implementation both \
  --levels 1,3,9,19 \
  > "$LOG_DIR/silesia.log" 2>&1

# Level-specific checkpoint
cargo run --release --example silesia_bench -- \
  --download \
  --implementation both \
  --levels 6,8,12 \
  > "$LOG_DIR/silesia-lazy.log" 2>&1
```

### Generic heuristics

- A ratio win on `repetitive` or `binary_structured` data is valuable only if throughput does not regress badly on the fast cases.
- A throughput win is valuable only if compressed size does not materially worsen on compressible inputs.
- Regressing `random/1` badly is usually a red flag, because it exercises the near-incompressible path.
- Simpler code wins ties.

These are secondary heuristics. Subsystem-specific policy in
`docs/level-subsystem-baselines.md` takes precedence.

### Level-specific validation

Keep the weighted benchmark and acceptance tests as the global gate. For work
aimed at a specific subsystem, also run targeted per-level checks so the result
can be attributed to the intended levels:

- `Fast`: benchmark at least levels `1` and `2`
- `DFast`: benchmark at least levels `3` and `4`
- `Greedy`: benchmark level `5`
- `Lazy` / `Lazy2`: benchmark at least levels `6`, `8`, and `12`
- `BtLazy2`: benchmark at least levels `13` and `15`
- `Optimal BT`: benchmark at least levels `16`, `18`, and `19`

Use these targeted runs to decide whether a local change matches the expected
tradeoff for that subsystem. The actual subsystem expectations live in
`docs/level-subsystem-baselines.md`. For intentionally level-specific work, do
not rely on the weighted benchmark alone, and do not treat flat weighted
results as a contradiction if the per-level movement is correct and the
spillover is acceptable.

## Profiling Loop

Use profiling when the fast benchmark shows a regression, a suspicious plateau, or a likely hotspot. Profile first, then change code. Do not cargo-cult micro-optimizations.

### Benchmark profiling

```bash
ZSTD_RS_PROFILE_BENCHES=1 cargo bench --profile profiling --features profiling --bench compression
```

This writes per-benchmark flamegraph artifacts under Criterion output directories.

### CLI profiling

Use the CLI path for a focused compress or decompress hotspot on a real file:

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

## Experiment Loop

Loop autonomously once setup is complete:

1. Inspect the current commit and the last accepted result.
2. Choose one concrete idea — think big, aim for algorithmic wins. Check `ideas.md` for promising directions.
3. Edit only the code needed for that idea.
4. Run acceptance tests and the default fast benchmark; save the logs.
5. If the change is intentionally level-specific, also run targeted per-level
   benchmarks for the affected subsystem.
6. **Gate 1 check**:
   - For repo-wide work: improvement >= 3% on at least one metric, no regression > 1% -> proceed to Gate 2.
   - For level-specific work: targeted per-level results match subsystem policy -> proceed to Gate 2, even if weighted results are flat.
   - Improvement 1-3% -> enter the **stacking phase** when the direction is still promising.
   - Discard immediately when neither the weighted results nor the targeted per-level results justify the change.
7. **Gate 2**: run the Silesia benchmark with `--implementation both` and the affected levels. If the targeted levels do not move in the intended direction relative to reference, **discard**.
8. If needed, run `cargo test` or the full benchmark sweep for extra confidence.
9. Log the outcome in `results.tsv`, including percentage changes and which levels were checked.
10. Keep the commit only if it cleared the appropriate gates for the kind of change you made; otherwise revert to the original baseline.
11. Update `ideas.md` if you learned something useful for future agents (new directions, dead ends, refinements). Keep entries concise and actionable; do not duplicate `results.tsv` data.

### Stacking phase

Small wins (1-3%) may combine into a significant improvement:

1. Keep the code changes in your working tree (do not discard yet).
2. Stack a complementary idea on top (e.g., if you improved match finding, now try reducing copy overhead in the same hot path).
3. After each stacking attempt, re-run the weighted benchmark against the **original baseline**.
4. If combined changes clear >= 3% on at least one metric -> proceed to Gate 2.
5. If after **3 stacking attempts** the total still does not clear 3%, **discard the entire stack** and revert to the original baseline. Log as `discard (stacked, X% total, below gate)`.

Stacking rules:
- Each stacked idea should be complementary, not a retry of the same approach.
- Do not stack more than 3 ideas before either clearing the gate or discarding.

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

## Logging and committing

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

**Always commit your work.** At the end of each research run — or periodically
during long runs — commit `results.tsv` and `ideas.md` even if every experiment
was discarded. The log of what was tried and what was learned is valuable to
future agents. Use a commit message like
`Update autoresearch results and ideas (N experiments, M kept)`.

## Safety

- Redirect long command output to log files; do not flood context with raw Criterion output.
- Write temp benchmark/test logs under `/tmp` or another untracked directory, not in the repo root.
- Do not commit `bench.log`, `bench-full.log`, `acceptance.log`, `silesia.log`, or profile artifacts.
- If acceptance tests fail because `zstd` is missing, say so explicitly; that weakens confidence in any keep/discard decision.
- If a change touches frame encoding, literals, sequences, or checksums, run broader tests before keeping it.

## Stop Condition

The loop is autonomous. Do not stop to ask whether to continue once the user has started a research run. Keep iterating until the user interrupts or a hard blocker makes further progress non-credible.

**Discards are normal, not a reason to stop.** The two-gate system is designed
to reject most ideas — that is working as intended. After a discard, go back to
step 1 of the experiment loop and try a different idea. Use profiling, re-read
hot paths, or try a completely different approach. Consecutive discards mean you
need better ideas, not that you should give up.

A "hard blocker" means something like: the build is broken and you cannot fix
it, or the test suite has an unrelated failure you cannot work around. It does
**not** mean "I tried a few things and they didn't clear the bar."

Aim for at least 8-10 experiment iterations per research run.
