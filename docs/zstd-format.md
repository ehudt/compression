# Zstd frame format details

Reference for the frame/block/literals encoding used in this codebase.

---

## Frame structure

A zstd-compressed file is one or more **frames**:

```
[Magic 4B][Frame Header][Block...][Optional checksum 4B]
```

**Frame Header** (written by `frame.rs:compress_with_config`):
- FHD byte: `FCS_flag=2` (4-byte content size), no dict, checksum bit
- Window descriptor byte: 56 (= 128 KiB window)
- 4-byte content size (u32 LE)

**Block header** (3 bytes):
- bit[0]: last-block flag
- bits[2:1]: block type -- 0=raw, 1=RLE, 2=compressed
- bits[23:3]: block size in bytes

Block payload for type 2 (compressed):
```
[Literals section][Sequences section]
```

---

## Literals section encoding

The size field in the first byte(s) uses **different packing depending on
`size_format` (bits [3:2] of byte 0)**:

| `size_format` | Header bytes | Size bits in byte 0 | Shift to decode |
|---|---|---|---|
| 0 | 1 | bits [7:3] (5 bits) | `byte0 >> 3` |
| 1 | 2 | bits [7:4] (4 bits) | `byte0 >> 4` |
| 2 | 1 | same as 0 | `byte0 >> 3` |
| 3 | 3 | bits [7:4] (4 bits) | `byte0 >> 4` |

**Encoder constants** (`src/encoder/block.rs`):
- raw sf=0: `byte0 = (n << 3) as u8`
- raw sf=1: `byte0 = 0x04 | ((n & 0xF) << 4) as u8`
- raw sf=3: `byte0 = 0x0C | ((n & 0xF) << 4) as u8`
- RLE sf=0: `byte0 = 0x01 | ((n << 3) as u8)`
- RLE sf=1: `byte0 = 0x05 | ((n & 0xF) << 4) as u8`
- RLE sf=3: `byte0 = 0x0D | ((n & 0xF) << 4) as u8`
- Compressed sf=2 (4-byte header): `byte0 = 0x0A | ((regen & 0xF) << 4) as u8`

If you add a new sf value, update **both** `encoder/block.rs` and
`decoder/literals.rs:decode_size_raw` / `decode_size_compressed`.

---

## Huffman coding invariants

`HuffmanTable::from_weights` (`src/huffman.rs`):

- `max_bits` is **NOT** `max_weight`.  It is computed from the weight sum:
  `max_bits = next_power_of_two(sum_weight).ilog2()`
  where `sum_weight = sum of 2^(w_i - 1)` for active symbols.
  This must equal `2^max_bits` for a complete tree.
- Canonical codes are generated starting from `next_code[1] = 0`.
  `bl_count[0]` (absent symbols) **must not** influence code generation.
  The loop starts at `bits = 2`, not `bits = 1`.
- When building the decode lookup table (`HuffmanTable::decode`), each symbol
  fills `1 << (max_bits - len)` consecutive entries.  If any `code >= (1 << len)`
  the table overflows -- this indicates invalid lengths from `build_lengths`.
