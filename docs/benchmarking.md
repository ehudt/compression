# Benchmarking guide

---

## Quick-start commands

```bash
cargo bench                            # Fast signal benchmark (speed + ratio)
cargo bench --bench weighted           # Weighted composite benchmark (one score per metric)
cargo bench --bench squash             # Squash-style multi-corpus benchmark
ZSTD_RS_FULL_BENCHES=1 cargo bench    # Exhaustive all-level sweep (levels 1-22)
```

**Never run more than one benchmark at a time.** Concurrent benchmarks cause
CPU contention and produce unreliable numbers. Wait for one to finish before
starting the next.

---

## Signal benchmark details

`cargo bench` runs four 64 KiB cases: `all_zeros` level 3, `repetitive`
level 3, `binary_structured` level 3, `random` level 1, plus two round-trip
checks.

Set `ZSTD_RS_FULL_BENCHES=1` for the exhaustive `1..=22` sweep.

---

## Squash benchmark

`cargo bench --bench squash` uses eight synthetic corpora (text, xml,
source_code, executable, database, medical_image, json, random) at levels
1, 3, 9, 19.  Full mode uses 256 KiB inputs and all 22 levels plus
size-scaling tests.

---

## Silesia benchmark

The standalone Silesia benchmark (`examples/silesia_bench.rs`) compares
`zstd_rs` against the system `zstd` and generates markdown, JSON, and SVG
outputs under `docs/benchmarks/`.

**Required tools:** Rust toolchain, `zstd` CLI in `PATH`, `curl` and
`python3` in `PATH` when using `--download`.

```bash
# Both implementations
cargo run --release --example silesia_bench -- --download --implementation both

# Official zstd only
cargo run --release --example silesia_bench -- --download --implementation official

# zstd_rs only
cargo run --release --example silesia_bench -- --download --implementation ours
```

All flags:

```bash
cargo run --release --example silesia_bench -- \
  --corpus-dir ~/silesia \
  --output-dir docs/benchmarks \
  --implementation both \
  --levels 1,3,9,19 \
  --min-bench-ms 1000
```

---

## Profiling workflow

Profiling is opt-in; it must not change behavior or add overhead when disabled.

- Build with `--features profiling`.
- Run: `cargo run --profile profiling --features profiling -- --profile-cpu out.svg --profile-repeat 200 --profile-min-ms 500 compress 3 input out.zst`
  (same flags work with `decompress`).
- CLI profiling does one unprofiled warmup iteration, then samples the
  steady-state in-memory loop until repeat count and duration target are met.
- Integration tests emit one SVG per test when
  `ZSTD_RS_PROFILE_TESTS=/path/to/dir` is set.
- Criterion benchmarks enable `pprof` when `ZSTD_RS_PROFILE_BENCHES=1` is set.
- Every profile capture also writes `*.folded` (folded stacks) and
  `*.summary.txt` (agent-friendly summary).
