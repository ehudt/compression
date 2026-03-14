//! Compressed block encoder.
//!
//! Encodes a block of input data into the zstd compressed block format:
//! 1. Literals section (Huffman-coded literal bytes)
//! 2. Sequences section (0 sequences — the entire block is encoded as literals)
//!
//! # Note on design
//!
//! Encoding all content as literals (with 0 sequences) is valid per the zstd
//! spec and still achieves good compression ratios via Huffman coding of the
//! literal bytes.  LZ77 back-reference sequences require a correctly-implemented
//! FSE state machine (a tANS coder), which is left as a future enhancement.
//! The decoder already supports the full FSE sequence format for decompression.

use crate::error::Result;
use crate::huffman::{write_huffman_header, HuffmanTable, MAX_SYMBOLS};

/// Encode a block of data into compressed form (literals-only, 0 sequences).
///
/// Returns the compressed bytes (without block header).
pub fn encode_block(data: &[u8], _cfg: &super::MatchConfig) -> Result<Vec<u8>> {
    if data.is_empty() {
        // One-byte "0 raw literals" + one-byte "0 sequences"
        return Ok(vec![0x00, 0x00]);
    }

    let mut out = encode_literals(data)?;
    out.push(0x00); // 0 sequences
    Ok(out)
}

/// Encode the literals section.
fn encode_literals(data: &[u8]) -> Result<Vec<u8>> {
    // Count symbol frequencies
    let mut freqs = [0u32; MAX_SYMBOLS];
    for &b in data {
        freqs[b as usize] += 1;
    }

    // Single-symbol input → use RLE (type=1)
    let nonzero_count = freqs.iter().filter(|&&f| f > 0).count();
    if nonzero_count == 1 {
        let sym = freqs.iter().position(|&f| f > 0).unwrap() as u8;
        return encode_rle(sym, data.len());
    }

    // Build Huffman table; fall back to raw on failure
    let table = match HuffmanTable::from_frequencies(&freqs) {
        Ok(t) => t,
        Err(_) => return encode_raw(data),
    };
    let encoded = match table.encode(data) {
        Ok(e) => e,
        Err(_) => return encode_raw(data),
    };

    let huff_header = write_huffman_header(&table);
    let comp_size = huff_header.len() + encoded.len();

    // Fall back to raw if no size gain
    if comp_size >= data.len() {
        return encode_raw(data);
    }

    encode_compressed(data.len(), &huff_header, &encoded)
}

// ── Literals type=0 (raw) ─────────────────────────────────────────────────────
// Byte 0: bits[1:0]=0 (type), bits[3:2]=sf, bits[7:3]=size_low

fn encode_raw(data: &[u8]) -> Result<Vec<u8>> {
    let n = data.len();
    let mut out = Vec::with_capacity(3 + n);
    if n < 32 {
        // sf=0: 5-bit size in bits [7:3] of byte 0
        out.push((n << 3) as u8); // type=0, sf=0
    } else if n < 4096 {
        // sf=1: 12-bit size: bits[7:4] of byte0 + byte1
        out.push(0x04 | (((n & 0xF) << 4) as u8)); // type=0, sf=1
        out.push((n >> 4) as u8);
    } else {
        // sf=3: 20-bit size: bits[7:4] of byte0 + byte1 + byte2
        out.push(0x0C | (((n & 0xF) << 4) as u8)); // type=0, sf=3
        out.push(((n >> 4) & 0xFF) as u8);
        out.push(((n >> 12) & 0xFF) as u8);
    }
    out.extend_from_slice(data);
    Ok(out)
}

// ── Literals type=1 (RLE) ─────────────────────────────────────────────────────

fn encode_rle(sym: u8, count: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(4);
    if count < 32 {
        // sf=0: 5-bit size in bits [7:3]
        out.push(0x01 | ((count << 3) as u8)); // type=1, sf=0
    } else if count < 4096 {
        // sf=1: 12-bit size: bits[7:4] of byte0 + byte1
        out.push(0x05 | (((count & 0xF) << 4) as u8)); // type=1, sf=1
        out.push((count >> 4) as u8);
    } else {
        // sf=3: 20-bit size: bits[7:4] of byte0 + byte1 + byte2
        out.push(0x0D | (((count & 0xF) << 4) as u8)); // type=1, sf=3
        out.push(((count >> 4) & 0xFF) as u8);
        out.push(((count >> 12) & 0xFF) as u8);
    }
    out.push(sym);
    Ok(out)
}

// ── Literals type=2 (Huffman, single stream) ──────────────────────────────────
// Use size_format=2 (4-byte header): 14-bit regen_size, 14-bit comp_size
// Byte 0: bits[1:0]=2, bits[3:2]=2 (sf), bits[7:4]=regen[3:0]
// Byte 1: regen[11:4]
// Byte 2: regen[13:12] | comp[5:0]<<2
// Byte 3: comp[13:6]

fn encode_compressed(regen: usize, huff_header: &[u8], encoded: &[u8]) -> Result<Vec<u8>> {
    let comp = huff_header.len() + encoded.len();
    debug_assert!(regen < (1 << 14), "regen_size too large for 4-byte header");
    debug_assert!(comp < (1 << 14), "comp_size too large for 4-byte header");

    let mut out = Vec::with_capacity(4 + comp);
    // sf=2: bits [3:2] = 0b10, type=2: bits[1:0]=0b10 → byte0 base = 0x0A
    out.push(0x0A | (((regen & 0xF) << 4) as u8));
    out.push(((regen >> 4) & 0xFF) as u8);
    out.push((((regen >> 12) & 0x3) | ((comp & 0x3F) << 2)) as u8);
    out.push(((comp >> 6) & 0xFF) as u8);
    out.extend_from_slice(huff_header);
    out.extend_from_slice(encoded);
    Ok(out)
}
