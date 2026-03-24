# Compression Level Subsystem Baselines

This document records the current high-level tradeoff expectations for each
compression subsystem, based on the repo's implementation strategy and the
current Silesia comparison against the reference `zstd`.

Use this as context for deciding what kinds of regressions are acceptable when
working on a specific level family. This is not a replacement for benchmark
data; it is the interpretation layer for that data.

## Subsystems

The practical optimization subsystems are:

- `Fast`: levels `1-2`
- `DFast`: levels `3-4`
- `Greedy`: level `5`
- `Lazy` / `Lazy2`: levels `6-12`
- `BtLazy2`: levels `13-15`
- `Optimal BT`: levels `16-19`
- Levels `20-22` currently reuse level `19` parameters and should be treated
  like `BtUltra2` until they diverge.

## Baseline source

Current baseline data comes from:

```bash
cargo run --release --example silesia_bench -- \
  --download \
  --implementation both \
  --levels 1,2,3,4,5,6,8,12,13,15,16,18,19
```

The generated comparison table lives in
`docs/benchmarks/silesia-comparison.md`.

## Baseline snapshot

Representative points from the current Silesia comparison:

| Level | Ours ratio | Ref ratio | Ours comp | Ref comp | Ours decomp | Ref decomp |
| ----- | ----------:| ---------:| ---------:| --------:| -----------:| ----------:|
| `1`   | `1.216` | `2.886` | `743.1 MB/s` | `334.6 MB/s` | `1378.0 MB/s` | `964.2 MB/s` |
| `2`   | `1.360` | `3.048` | `496.2 MB/s` | `297.6 MB/s` | `814.8 MB/s` | `883.1 MB/s` |
| `3`   | `1.872` | `3.185` | `250.1 MB/s` | `228.7 MB/s` | `414.5 MB/s` | `898.2 MB/s` |
| `4`   | `1.944` | `3.242` | `219.0 MB/s` | `190.9 MB/s` | `403.3 MB/s` | `866.0 MB/s` |
| `5`   | `2.588` | `3.359` | `63.8 MB/s` | `116.6 MB/s` | `294.2 MB/s` | `861.8 MB/s` |
| `6`   | `2.588` | `3.440` | `64.3 MB/s` | `89.4 MB/s` | `294.7 MB/s` | `947.7 MB/s` |
| `8`   | `2.793` | `3.528` | `24.8 MB/s` | `62.6 MB/s` | `310.3 MB/s` | `933.6 MB/s` |
| `12`  | `2.957` | `3.637` | `5.9 MB/s` | `26.9 MB/s` | `324.4 MB/s` | `888.7 MB/s` |
| `13`  | `2.875` | `3.652` | `16.7 MB/s` | `11.6 MB/s` | `340.7 MB/s` | `902.3 MB/s` |
| `15`  | `2.885` | `3.702` | `15.4 MB/s` | `7.0 MB/s` | `330.1 MB/s` | `859.4 MB/s` |
| `16`  | `3.101` | `3.831` | `5.1 MB/s` | `5.3 MB/s` | `358.8 MB/s` | `909.6 MB/s` |
| `18`  | `3.161` | `3.965` | `3.9 MB/s` | `3.3 MB/s` | `351.5 MB/s` | `768.4 MB/s` |
| `19`  | `3.167` | `3.997` | `3.1 MB/s` | `2.7 MB/s` | `352.5 MB/s` | `779.4 MB/s` |

## Current interpretation by subsystem

### Fast: levels 1-2

Current state:

- Compression speed is far ahead of reference.
- Ratio is dramatically below reference.
- Decompression is better than reference at level `1`, but not at level `2`.

Implication:

- This subsystem remains speed-first.
- It can afford to spend some compression speed to recover ratio.
- Do not accept speed wins that mostly come from collapsing toward raw output.

### DFast: levels 3-4

Current state:

- Compression speed is only modestly ahead of reference.
- Ratio is still far behind reference.
- Decompression is much slower than reference.

Implication:

- This subsystem should aim for balanced improvement.
- Spending some compression speed for ratio is acceptable.
- Avoid changes that make `DFast` look like `Fast` in ratio behavior.

### Greedy: level 5

Current state:

- We are behind reference on both ratio and compression speed.
- Decompression is also far slower.

Implication:

- Treat this level as an underperforming anchor, not as a healthy baseline.
- This subsystem should pursue balanced improvement on both ratio and
  compression speed.
- Avoid changes that improve only one axis while worsening the other without a
  very strong reason.

### Lazy / Lazy2: levels 6-12

Current state:

- Across sampled levels `6`, `8`, and `12`, we are behind reference on both
  ratio and compression speed.
- Decompression is also much slower than reference.

Implication:

- This family is not currently in a position to spend speed freely for ratio.
- Prefer changes that recover ratio and compression speed together, or at least
  deliver ratio gains without major additional slowdown.
- Reject search-cost increases that do not clearly improve match quality.

### BtLazy2: levels 13-15

Current state:

- We are faster than reference on compression at sampled levels `13` and `15`.
- Ratio remains materially behind reference.
- Decompression remains much slower than reference.

Implication:

- This subsystem can afford to spend compression speed for ratio.
- Large slowdowns are still not automatically acceptable, but the current
  compression-speed lead gives meaningful room to trade.
- Focus on better search quality and parse decisions before micro-optimizing for
  speed.

### Optimal BT: levels 16-19

Current state:

- Compression speed is roughly competitive with reference, and slightly better
  at sampled levels `18-19`.
- Ratio remains materially behind reference.
- Decompression is still much slower than reference.

Implication:

- This subsystem is primarily ratio-first.
- Spending some compression speed for ratio is acceptable.
- Pathological slowdowns are still failures, especially if they come from
  repeated work or inefficient DP/BT traversal.

## Cross-cutting observation

Except for level `1`, decompression throughput is substantially below reference
at every sampled level. Compression-level work should not ignore decompression
regressions, but the main subsystem-specific policy above is driven by
compression ratio and compression throughput because that is where the
compression-level strategies differ most directly.

## How to use this document

- When making a level-specific change, benchmark the affected subsystem levels.
- Use the subsystem interpretation above to decide which regressions are
  acceptable.
- Revisit this document when the Silesia baseline changes materially.
