---
name: compression-autoresearch
description: Autonomous research loop to improve this repo's compression engine toward reference zstd baselines. Picks a target (subsystem or cross-cutting), benchmarks on Silesia, and keeps only verified improvements.
---

# Compression Autoresearch

Run an autonomous experiment loop for this repository's Rust zstd implementation.
The goal is to close the gap with reference `zstd` — better compression ratio,
better throughput, or both — at each compression level.

Never accept an unbudgeted tradeoff. Allowed regressions depend on the target
subsystem and must match `docs/level-subsystem-baselines.md`. Never keep a
change that compromises frame correctness or interoperability.

## Required reading

Read these files before changing code:

- `docs/level-subsystem-baselines.md` — current per-level tradeoff expectations and Silesia comparison snapshot
- `docs/benchmarking.md` — benchmark modes, Silesia usage, profiling workflow
- `ideas.md` — promising directions and dead ends from previous sessions
- `tests/acceptance.rs` — interop contract with system `zstd`
- `tests/integration.rs` — round-trip and sanity expectations

Likely hot paths:

- `src/encoder/lz77.rs` — match finding
- `src/encoder/block.rs` — block construction and sequence emission
- `src/huffman.rs` and `src/fse.rs` — entropy coding
- `src/frame.rs` — block/frame-level decisions
- `src/decoder/*` — only when a format or round-trip change requires it

Do not edit the benchmark harness or tests unless the task is explicitly about
measurement or missing coverage.

## Setup

1. **Stay on the current branch.** Do not create, switch to, or push any
   branch. All commits go directly onto whatever branch is checked out when the
   run starts. If run 10 times in a row, the result should be 10 commits
   stacked on the starting HEAD.
2. Confirm the worktree is clean enough to experiment safely.
3. Use the tracked `results.tsv` and `campaigns.tsv` files as the experiment
   log. Before a run, ensure they are in the current schema:
   ```bash
   python3 scripts/autoresearch_log.py ensure-files --results results.tsv --campaigns campaigns.tsv
   ```
4. Create a unique temp log directory outside the repo:
   ```bash
   LOG_DIR=$(mktemp -d /tmp/compression-autoresearch-XXXXXX)
   ```
   Write all benchmark/test logs there.
5. Download the Silesia corpus (needed for integration tests to compile):
   ```bash
   cargo run --release --example silesia_bench -- --download --implementation ours --levels 1 2>/dev/null
   ```
   This populates `~/silesia/` which `tests/integration.rs` depends on at
   compile time via `include_bytes!`.
6. Read `docs/level-subsystem-baselines.md` and `ideas.md`.
7. Establish the baseline for the current research campaign before making any
   code changes (see Baseline).

## Choosing a target

Each experiment targets either a **subsystem** or a **cross-cutting** change.
Pick the highest-leverage target by reading the baselines doc (where are the
biggest gaps?) and `ideas.md` (what has been tried, what looks promising?).

### Level-specific subsystem work

Pick a subsystem and improve it toward reference `zstd`:

| Subsystem | Levels | Benchmark levels |
|-----------|--------|------------------|
| Fast | 1-2 | `1,2` |
| DFast | 3-4 | `3,4` |
| Greedy | 5 | `5` |
| Lazy / Lazy2 | 6-12 | `6,8,12` |
| BtLazy2 | 13-15 | `13,15` |
| Optimal BT | 16-19 | `16,18,19` |

The baselines doc tells you the tradeoff policy for each subsystem: which axis
to prioritize, which regressions are acceptable, and how much headroom exists.

### Cross-cutting work

Some improvements affect all levels:

- **Structural**: entropy coding completeness (e.g., FSE-compressed Huffman
  weights), frame-level decisions, block type selection.
- **Pipeline**: removing passes, reducing allocations, eliminating copies.

For cross-cutting work, benchmark a representative spread: `--levels 1,3,9,19`.
Judge the change by whether the *intended axis* improves across the spread
without collapsing the other axes.

### How to decide

1. Read `docs/level-subsystem-baselines.md` — find the largest gaps.
2. Read `ideas.md` — find promising untried directions or refinements of
   near-misses.
3. Consider the type of gap: if a subsystem is far behind on ratio, a
   level-specific algorithm change is likely best. If throughput is behind
   everywhere, a cross-cutting pipeline change may have more leverage.
4. Pick one concrete idea. Think big — aim for algorithmic wins, not
   micro-optimizations.

## Campaigns and baselines

A research **campaign** is a run against one chosen target area on one branch
head. A campaign can contain multiple experiments.

Reuse the campaign baseline while all of these stay true:

- the checked-out branch head still descends from the original baseline commit
- the target subsystem or cross-cutting focus has not changed
- the benchmark levels under evaluation are the same

Start a new campaign baseline when the branch moves in a way that invalidates
comparison, or when you pivot to a meaningfully different target.

## Baseline

Establish the baseline once per campaign, before any code changes for that
campaign.

### Correctness baseline

```bash
cargo test --test acceptance -- --nocapture > "$LOG_DIR/acceptance.log" 2>&1
```

### Silesia baseline

Run Silesia for the levels you plan to target:

```bash
# Level-specific example (Lazy subsystem)
cargo run --release --example silesia_bench -- \
  --download \
  --implementation both \
  --levels 6,8,12 \
  > "$LOG_DIR/silesia-baseline.log" 2>&1

# Cross-cutting example
cargo run --release --example silesia_bench -- \
  --download \
  --implementation both \
  --levels 1,3,9,19 \
  > "$LOG_DIR/silesia-baseline.log" 2>&1
```

Record the baseline numbers in `results.tsv` and add or update the matching row
in `campaigns.tsv`. All later comparisons in the same campaign are against this
baseline.

### Weighted sanity baseline

The weighted benchmark is a fast synthetic sanity check. It is **not** a gate,
but a divergence detector.

```bash
cargo bench --bench weighted > "$LOG_DIR/weighted-baseline.log" 2>&1
grep -A5 "\\[weighted-benchmark\\]" "$LOG_DIR/weighted-baseline.log"
```

Record the weighted scores for later comparison.

## Experiment loop

Loop autonomously once setup is complete:

1. **Pick a campaign target.** Review baselines and `ideas.md`. Choose one
   concrete target (subsystem or cross-cutting) and stay with it for multiple
   experiments until you either find a keep, hit a clear dead end, or profiling
   says the target was wrong.

2. **Edit code.** Only the code needed for that idea.

3. **Correctness check.** Run acceptance tests:
   ```bash
   cargo test --test acceptance -- --nocapture > "$LOG_DIR/acceptance.log" 2>&1
   ```
   If acceptance fails, fix or discard immediately.

4. **Silesia benchmark.** Run Silesia for the affected levels:
   ```bash
   cargo run --release --example silesia_bench -- \
     --download \
     --implementation both \
     --levels <affected-levels> \
     > "$LOG_DIR/silesia.log" 2>&1
   ```

5. **Evaluate against baseline.** Compare Silesia results to the baseline
   established in setup. Apply the keep/discard criteria below.

6. **Weighted sanity check.** If the change passes the Silesia evaluation, run
   the weighted benchmark as a divergence check:
   ```bash
   cargo bench --bench weighted > "$LOG_DIR/weighted.log" 2>&1
   ```
   If weighted regresses > 5% on any composite score while Silesia improved,
   investigate before keeping. This likely indicates overfitting or a
   measurement issue.

7. **Log the outcome** in `results.tsv`. Fill in the campaign metadata columns:
   `campaign_id`, `target`, `axis`, `levels`, `rerun_status`,
   `evidence_status`, and `per_file_notes`.

8. **Keep or discard.** If the change clears the criteria, keep the commit.
   Otherwise, revert to the baseline.

9. **Update `ideas.md`** if you learned something useful for future agents.

10. **Repeat.** Stay in the current campaign while the target still looks
    promising. Pivot only when repeated evidence says the current lane is
    saturated.

## Keep/discard criteria

Think BIG. Do not keep micro-optimizations. The goal is meaningful algorithmic
wins, not shuffling code for a 0.1% blip.

All comparisons are against the **original baseline** from setup.

### What "improvement" means

An improvement is real when it is **visible, consistent, and confirmed**:

- **Visible**: the change meaningfully narrows the gap to reference `zstd` on
  the affected levels. For throughput, at least 2% on the affected levels
  (below that is within Silesia run-to-run variance). For ratio, the
  improvement should be apparent in per-file Silesia results — not just a
  rounding-error shift in the aggregate.
- **Consistent**: the improvement shows across multiple Silesia files, not just
  one outlier.
- **Confirmed**: if the improvement is borderline (2-5% throughput, or ratio
  gains visible on only 2-3 files), rerun the benchmark once to confirm it is
  not noise.

### Subsystem policy

Use `docs/level-subsystem-baselines.md` to judge the tradeoff:

- Does the change move the intended axis (ratio or throughput) in the right
  direction?
- Are regressions on the other axis within the subsystem's budget?
- Does the change close the gap with reference `zstd` on the affected levels?

### Discard when

- Improvement is not visible or not consistent across Silesia files.
- The change regresses an axis that the subsystem cannot afford to regress
  (per baselines doc).
- Acceptance tests fail.
- The improvement cannot be confirmed on rerun.
- The code complexity is disproportionate to the gain.

### Weighted divergence flag

The weighted benchmark uses small synthetic data and has historically shown
false positives (ideas.md documents several). It is a **sanity check**, not a
gate:

- Weighted improving while Silesia is flat → do not trust the weighted signal.
- Weighted regressing > 5% while Silesia improves → investigate before keeping.
- Weighted flat while Silesia improves → normal, proceed.

### Pivot rules

Do not keep probing a stale local neighborhood forever.

- After 2-3 discards in the same narrow idea family, either profile again or
  pivot to a different lever.
- If `ideas.md` already classifies an idea family as below-bar on the current
  branch shape, treat that as a warning against another tiny variant.
- Prefer structural changes, pass removal, or policy changes once repeated
  micro-optimizations fail to clear the gate.

## Profiling

Use profiling when a benchmark shows a regression, a suspicious plateau, or to
find the hotspot before writing code. Profile first, then change code.

### Benchmark profiling

```bash
ZSTD_RS_PROFILE_BENCHES=1 cargo bench --profile profiling --features profiling --bench compression
```

### CLI profiling

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

## Good experiment ideas

- Improve LZ77 match search quality without exploding search cost
- Reduce wasted work in candidate scanning, chain traversal, or lazy matching
- Make block decisions smarter: raw vs RLE vs compressed
- Improve literal or sequence coding decisions when entropy work is not paying off
- Reduce allocator churn, copying, or temporary buffer traffic in hot paths
- Special-case obvious incompressible inputs so they exit quickly
- Implement missing format features (FSE-compressed Huffman weights) to unlock
  ratio on high-entropy data

## Bad experiment ideas

- Chasing tiny wins by adding large amounts of brittle code
- Breaking zstd interoperability to get prettier ratios
- Changing tests or benches to hide regressions
- Tuning constants to one corpus while hurting generality
- Keeping a change because one Silesia file improved while the rest got worse

## Logging and committing

TSV header:

```tsv
commit	status	ratio_notes	compress_notes	decompress_notes	roundtrip_notes	description
```

Use one row per experiment, including discards. Status values:

- `baseline` — initial measurement
- `keep` — change cleared all criteria
- `discard` — change did not clear criteria
- `crash` — build or test failure
- `blocked` — infrastructure or unrelated repo problem prevented evaluation
- `inconclusive` — result did not justify a keep/discard claim yet

Include percentage changes vs baseline and which levels were checked. In
the dedicated metadata columns, record:

- `campaign_id` — stable id for the campaign, for example `dfast-20260327-a`
- `target` — subsystem or cross-cutting area
- `axis` — `ratio`, `compress`, `decompress`, or `balanced`
- `levels` — exact benchmark levels, for example `3,4`
- `rerun_status` — `not_needed`, `needed`, `confirmed`, `failed`
- `evidence_status` — `aggregate_only`, `per_file_visible`, `per_file_confirmed`
- `per_file_notes` — short summary of the strongest per-file winners/losers

`campaigns.tsv` should contain one row per campaign with the baseline commit and
high-level target metadata.

**Always commit your work.** At the end of each research run — or periodically
during long runs — commit `results.tsv` and `ideas.md` even if every experiment
was discarded. Use a commit message like
`Update autoresearch results and ideas (N experiments, M kept)`.

## Safety

- Redirect long command output to log files; do not flood context.
- Write temp logs under `/tmp`, not in the repo root.
- Do not commit log files or profile artifacts.
- If acceptance tests fail because `zstd` is missing, say so explicitly; that
  weakens confidence in any keep/discard decision.
- If a change touches frame encoding, literals, sequences, or checksums, run
  `cargo test` (full suite) before keeping it.
- **Never run more than one benchmark at a time.** Concurrent benchmarks cause
  CPU contention and unreliable numbers.

## Stop condition

The loop is autonomous. Do not stop to ask whether to continue. Keep iterating
until the user interrupts or a hard blocker makes further progress impossible.

**Discards are normal, not a reason to stop.** After a discard, go back to
step 1 and try a different idea. Use profiling, re-read hot paths, or try a
completely different approach. Consecutive discards mean you need better ideas,
not that you should give up.

A "hard blocker" means the build is broken and you cannot fix it, or the test
suite has an unrelated failure you cannot work around. It does **not** mean
"I tried a few things and they didn't clear the bar."

Aim for at least 8-10 experiment iterations per research run, but do not pay
for that with repeated baseline rebuilds or stale micro-variants. A good run
contains multiple experiments per campaign baseline.
