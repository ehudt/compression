# Ideas

## FSE-compressed Huffman weight encoding

**Status:** Not yet implemented
**Context:** The zstd spec supports two ways to encode Huffman table weights in the literals section header:

1. **Direct mode** (header_byte 128–255): Stores up to 128 weights packed as 4-bit pairs. Simple but limited to ~128 active symbols.
2. **FSE-compressed mode** (header_byte 1–127): The weight array itself is entropy-coded using a small FSE table. Supports any number of symbols up to 256.

Currently the encoder only implements direct mode. When data uses more than 128 distinct byte values (e.g., medical/image data, executables, pseudo-random content), the encoder falls back to raw (uncompressed) literals because the Huffman table can't be serialized in direct mode.

**Opportunity:** Implementing FSE-compressed weight encoding would allow Huffman-coded literals for high-entropy data with large alphabets. This matters most for data that has all 256 byte values present but with skewed enough frequencies that Huffman coding still provides a size win (e.g., executables, structured binary formats).

**Implementation notes:**
- The decoder already has `decode_fse_weights()` in `src/huffman.rs` — the encoder counterpart is needed.
- The weight distribution typically has few distinct values (weights 1–11), so the FSE table is small.
- The encoder should also adopt the spec convention of omitting the last weight (implicit from the power-of-two tree completeness constraint), which effectively gives +1 symbol capacity in both modes.
- Compare compressed-weight header size vs direct-mode header size and pick the smaller one.

## lzbench-style benchmark against reference zstd

**Status:** Not yet implemented
**Reference:** https://github.com/facebook/zstd/blob/dev/README.md
**Context:** The official zstd repository benchmarks against other compressors using [lzbench](https://github.com/inikep/lzbench), an open-source in-memory benchmark by @inikep, on the [Silesia compression corpus](https://sun.aei.polsl.pl//~sdeor/index.php?page=silesia). Their reference results (Core i7-9700K @ 4.9GHz, Ubuntu 24.04, gcc 14.2.0):

| Compressor name         | Ratio | Compression| Decompress.|
| ---------------         | ------| -----------| ---------- |
| **zstd 1.5.7 -1**       | 2.896 |   510 MB/s |  1550 MB/s |
| brotli 1.1.0 -1         | 2.883 |   290 MB/s |   425 MB/s |
| zlib 1.3.1 -1            | 2.743 |   105 MB/s |   390 MB/s |
| **zstd 1.5.7 --fast=1** | 2.439 |   545 MB/s |  1850 MB/s |
| quicklz 1.5.0 -1        | 2.238 |   520 MB/s |   750 MB/s |
| **zstd 1.5.7 --fast=4** | 2.146 |   665 MB/s |  2050 MB/s |
| lzo1x 2.10 -1           | 2.106 |   650 MB/s |   780 MB/s |
| lz4 1.10.0               | 2.101 |   675 MB/s |  3850 MB/s |
| snappy 1.2.1             | 2.089 |   520 MB/s |  1500 MB/s |

**Opportunity:** Implement a benchmark that runs our encoder/decoder on the Silesia corpus and reports the same three metrics (ratio, compression MB/s, decompression MB/s) in the same tabular format, making it easy to compare our implementation against the reference C zstd and other compressors.

**Implementation notes:**
- Download the Silesia corpus (211 MB, 12 files covering text, executables, databases, images, etc.) and cache it locally (e.g., `benches/data/silesia/`; gitignore the directory).
- Measure in-memory throughput: time the compress/decompress functions directly, not including I/O, to match lzbench methodology.
- Report per-file and aggregate results in the same `| Compressor | Ratio | Compress | Decompress |` table format.
- Run across all levels (or a representative subset like 1, 3, 9, 19) to show the speed-vs-ratio tradeoff.
- Optionally shell out to the system `zstd` binary on the same corpus to provide a direct side-by-side comparison in the same run.
- Could be a new Criterion bench (`benches/silesia.rs`) or a standalone binary (`examples/silesia_bench.rs`).
