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
