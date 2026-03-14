//! Huffman coding for zstd literal compression.
//!
//! zstd uses a weight-based Huffman representation. Each symbol is assigned
//! a "weight" w where the code length is `max_bits - w + 1` (weight 0 means
//! the symbol is absent).
//!
//! # References
//! - <https://github.com/facebook/zstd/blob/dev/doc/zstd_compression_format.md#huffman-tree-description>

use crate::error::{Result, ZstdError};

/// Maximum number of symbols in a Huffman tree (all byte values + 1).
pub const MAX_SYMBOLS: usize = 256;
/// Maximum Huffman code length allowed by zstd.
pub const MAX_CODE_LEN: u8 = 11;

/// A Huffman tree, represented as a table of (code_length, code) pairs.
#[derive(Debug, Clone)]
pub struct HuffmanTable {
    /// Code length for each symbol (0 = absent).
    pub lengths: [u8; MAX_SYMBOLS],
    /// Canonical code for each symbol.
    pub codes: [u16; MAX_SYMBOLS],
    /// Maximum code length used.
    pub max_bits: u8,
}

impl HuffmanTable {
    /// Build a Huffman table from symbol weights.
    ///
    /// `weights[i]` is the weight assigned to symbol `i`.  A weight of 0
    /// means the symbol is unused.  The last symbol's weight is implied if it
    /// is omitted from the stream.
    pub fn from_weights(weights: &[u8]) -> Result<Self> {
        if weights.len() > MAX_SYMBOLS {
            return Err(ZstdError::HuffmanError("too many symbols"));
        }

        // Compute max_bits from the sum of weights.
        // For a complete Huffman tree: sum_weight = sum_i(2^(w_i - 1)) = 2^max_bits.
        // This is correct when ALL symbol weights are stored (including the last symbol
        // whose weight is normally implicit in the zstd stream format).
        let sum_weight: u32 = weights
            .iter()
            .map(|&w| if w > 0 { 1u32 << (w as u32 - 1) } else { 0 })
            .sum();
        if sum_weight == 0 {
            return Err(ZstdError::HuffmanError("all weights are zero"));
        }
        // Round up to the next power of two in case of minor rounding from build_lengths.
        let max_bits = sum_weight.next_power_of_two().ilog2() as u8;
        if max_bits == 0 || max_bits > MAX_CODE_LEN {
            return Err(ZstdError::HuffmanError("max_bits out of range"));
        }

        // Compute lengths from weights: length = max_bits - weight + 1 (weight 0 → absent).
        let mut lengths = [0u8; MAX_SYMBOLS];
        for (i, &w) in weights.iter().enumerate() {
            if w == 0 {
                lengths[i] = 0;
            } else if w > max_bits {
                return Err(ZstdError::HuffmanError("weight exceeds max_bits"));
            } else {
                let len = max_bits - w + 1;
                if len > MAX_CODE_LEN {
                    return Err(ZstdError::HuffmanError("code length exceeds max"));
                }
                lengths[i] = len;
            }
        }

        // Assign canonical codes.
        let codes = canonical_codes(&lengths, max_bits)?;

        Ok(Self {
            lengths,
            codes,
            max_bits,
        })
    }

    /// Build a Huffman table from symbol frequencies using a greedy algorithm.
    pub fn from_frequencies(freqs: &[u32]) -> Result<Self> {
        let n = freqs.len().min(MAX_SYMBOLS);
        if freqs.iter().take(n).all(|&f| f == 0) {
            return Err(ZstdError::HuffmanError("all frequencies are zero"));
        }

        // Use a simple package-merge / length-limited Huffman approach.
        // For simplicity, we use a basic heap-based Huffman capped at MAX_CODE_LEN.
        let lengths = build_lengths(freqs, MAX_CODE_LEN)?;
        let max_bits = lengths.iter().copied().max().unwrap_or(1).max(1);
        let codes = canonical_codes(&lengths, max_bits)?;

        Ok(Self {
            lengths,
            codes,
            max_bits,
        })
    }

    /// Encode `data` using this Huffman table, writing bits LSB-first.
    /// Returns the encoded bytes.
    pub fn encode(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Write bits MSB-first (zstd Huffman is MSB-first within each byte).
        let mut out: Vec<u8> = Vec::new();
        let mut pending: u64 = 0u64;
        let mut pending_bits: u32 = 0;

        for &sym in data {
            let len = self.lengths[sym as usize];
            if len == 0 {
                return Err(ZstdError::HuffmanError("symbol not in table"));
            }
            let code = self.codes[sym as usize] as u64;
            // Pack MSB-first: shift existing bits left, OR new code in the upper positions.
            pending = (pending << len) | code;
            pending_bits += len as u32;
            while pending_bits >= 8 {
                pending_bits -= 8;
                out.push((pending >> pending_bits) as u8);
                pending &= (1u64 << pending_bits) - 1;
            }
        }

        // Flush remaining bits (pad with zeros at LSB).
        if pending_bits > 0 {
            out.push((pending << (8 - pending_bits)) as u8);
        }

        Ok(out)
    }

    /// Decode `bit_count` bits from `data` into at most `max_symbols` symbols.
    pub fn decode(&self, data: &[u8], bit_count: usize, max_symbols: usize) -> Result<Vec<u8>> {
        // Build a fast lookup table: for each possible `max_bits`-bit prefix, store (symbol, length).
        let table_size = 1usize << self.max_bits;
        let mut decode_table: Vec<(u8, u8)> = vec![(0, 0); table_size];
        for sym in 0..MAX_SYMBOLS {
            let len = self.lengths[sym];
            if len == 0 {
                continue;
            }
            let code = self.codes[sym] as usize;
            // Fill all entries with this prefix
            let fill_count = 1 << (self.max_bits - len);
            for i in 0..fill_count {
                let idx = (code << (self.max_bits - len)) | i;
                decode_table[idx] = (sym as u8, len);
            }
        }

        let mut out = Vec::with_capacity(max_symbols);
        let mut buf: u64 = 0;
        let mut buf_bits: u32 = 0;
        let mut byte_iter = data.iter();
        let mut total_bits_consumed = 0usize;

        while total_bits_consumed < bit_count && out.len() < max_symbols {
            // Refill buffer (MSB-first)
            while buf_bits < self.max_bits as u32 {
                if let Some(&b) = byte_iter.next() {
                    buf = (buf << 8) | b as u64;
                    buf_bits += 8;
                } else {
                    break;
                }
            }

            if buf_bits < self.max_bits as u32 {
                // Pad with zeros for final partial byte
                buf <<= self.max_bits as u32 - buf_bits;
                buf_bits = self.max_bits as u32;
            }

            // Peek at top max_bits
            let idx = (buf >> (buf_bits - self.max_bits as u32)) as usize & (table_size - 1);
            let (sym, len) = decode_table[idx];
            if len == 0 {
                return Err(ZstdError::HuffmanError("invalid code during decode"));
            }
            buf_bits -= len as u32;
            buf &= (1u64 << buf_bits) - 1;
            total_bits_consumed += len as usize;
            out.push(sym);
        }

        Ok(out)
    }

    /// Compute symbol weights from this table's lengths (inverse of `from_weights`).
    pub fn to_weights(&self) -> Vec<u8> {
        let mut weights = vec![0u8; MAX_SYMBOLS];
        for sym in 0..MAX_SYMBOLS {
            let len = self.lengths[sym];
            if len > 0 {
                weights[sym] = self.max_bits - len + 1;
            }
        }
        // Trim trailing zeros
        while weights.last() == Some(&0) {
            weights.pop();
        }
        weights
    }
}

/// Assign canonical Huffman codes given code lengths.
fn canonical_codes(lengths: &[u8; MAX_SYMBOLS], max_bits: u8) -> Result<[u16; MAX_SYMBOLS]> {
    // Count symbols per length
    let mut bl_count = vec![0u32; max_bits as usize + 1];
    for &l in lengths {
        if l > 0 {
            if l > max_bits {
                return Err(ZstdError::HuffmanError("length exceeds max_bits"));
            }
            bl_count[l as usize] += 1;
        }
    }

    // Assign canonical starting codes per length.
    // next_code[1] = 0; for L >= 2: next_code[L] = (next_code[L-1] + bl_count[L-1]) << 1.
    // bl_count[0] (absent symbols) must NOT be included — start from length 1.
    let mut next_code = vec![0u16; max_bits as usize + 2];
    let mut code = 0u16;
    // next_code[1] = 0 (hardcoded; absent symbols at length 0 don't affect length-1 codes)
    for bits in 2..=max_bits as usize {
        code = (code + bl_count[bits - 1] as u16) << 1;
        next_code[bits] = code;
    }

    // Assign codes
    let mut codes = [0u16; MAX_SYMBOLS];
    for sym in 0..MAX_SYMBOLS {
        let l = lengths[sym] as usize;
        if l > 0 {
            codes[sym] = next_code[l];
            next_code[l] += 1;
        }
    }

    Ok(codes)
}

/// Build code lengths using a simple Huffman algorithm, capped at `max_len`.
fn build_lengths(freqs: &[u32], max_len: u8) -> Result<[u8; MAX_SYMBOLS]> {
    let n = freqs.len().min(MAX_SYMBOLS);
    let mut lengths = [0u8; MAX_SYMBOLS];

    // Build Huffman tree via a min-heap.
    // Node: (frequency, symbol_or_internal, left_child, right_child)
    // We represent the tree as indices into a flat array.
    let nodes: Vec<(u64, usize)> = freqs[..n]
        .iter()
        .enumerate()
        .filter(|&(_, &f)| f > 0)
        .map(|(i, &f)| (f as u64, i))
        .collect();

    if nodes.is_empty() {
        return Err(ZstdError::HuffmanError("no symbols with nonzero frequency"));
    }

    if nodes.len() == 1 {
        lengths[nodes[0].1] = 1;
        return Ok(lengths);
    }

    // We build depths via a simple O(n log n) heap approach.
    // depth_of[symbol_index] will hold the depth.
    let num_sym = nodes.len();
    let mut depth = vec![0u8; 2 * num_sym + 1];
    let mut tree: Vec<(u64, i32, i32)> = nodes
        .iter()
        .enumerate()
        .map(|(i, &(f, _))| (f, -(i as i32 + 1), -(i as i32 + 1)))
        .collect(); // negative = leaf (symbol index)

    // priority queue: (neg_freq, node_idx)
    use std::collections::BinaryHeap;
    use std::cmp::Reverse;
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = nodes
        .iter()
        .enumerate()
        .map(|(i, &(f, _))| Reverse((f, i)))
        .collect();

    let mut next_node = tree.len();
    // Extend tree to hold internal nodes
    tree.resize(2 * num_sym, (0, 0, 0));
    depth.resize(2 * num_sym, 0);

    let sym_indices: Vec<usize> = nodes.iter().map(|&(_, s)| s).collect();

    while heap.len() > 1 {
        let Reverse((f1, i1)) = heap.pop().unwrap();
        let Reverse((f2, i2)) = heap.pop().unwrap();
        let combined = f1 + f2;
        let parent = next_node;
        tree[parent] = (combined, i1 as i32, i2 as i32);
        depth[parent] = 0;
        next_node += 1;
        heap.push(Reverse((combined, parent)));
    }

    // Compute depths by DFS from root
    let root = next_node - 1;
    let mut stack = vec![(root, 0u8)];
    while let Some((node, d)) = stack.pop() {
        if node < num_sym {
            // leaf
            let sym = sym_indices[node];
            lengths[sym] = d.min(max_len);
        } else {
            let (_, left, right) = tree[node];
            stack.push((left as usize, d + 1));
            stack.push((right as usize, d + 1));
        }
    }

    // Enforce max_len by redistributing if needed (simple approach: truncate then fix)
    // Count bits used: sum(2^-len) should equal 1 for complete tree.
    // If any lengths exceed max_len, clamp and adjust longer codes to make room.
    let mut overflow: u32 = 0;
    for sym in 0..MAX_SYMBOLS {
        if lengths[sym] > max_len {
            overflow += (1u32 << (lengths[sym] - max_len)) - 1;
            lengths[sym] = max_len;
        }
    }
    // Give back overflow by extending shorter codes
    if overflow > 0 {
        for sym in (0..MAX_SYMBOLS).rev() {
            if lengths[sym] == 0 || lengths[sym] >= max_len {
                continue;
            }
            let steps = overflow.min(1 << (max_len - lengths[sym] - 1));
            lengths[sym] += 1;
            overflow -= steps;
            if overflow == 0 {
                break;
            }
        }
    }

    Ok(lengths)
}

/// Read Huffman header from a zstd literal block.
///
/// Returns `(table, bytes_consumed)`.
pub fn read_huffman_header(data: &[u8]) -> Result<(HuffmanTable, usize)> {
    if data.is_empty() {
        return Err(ZstdError::UnexpectedEof);
    }
    let header_byte = data[0];

    if header_byte < 128 {
        // FSE-compressed weights
        // header_byte is the number of bytes used for the FSE table
        let fse_size = header_byte as usize;
        if data.len() < 1 + fse_size {
            return Err(ZstdError::UnexpectedEof);
        }
        let fse_data = &data[1..1 + fse_size];
        let weights = decode_fse_weights(fse_data)?;
        let table = HuffmanTable::from_weights(&weights)?;
        Ok((table, 1 + fse_size))
    } else {
        // Direct representation: header_byte = number_of_symbols
        // Weights are packed 4 bits each
        let num_symbols = header_byte as usize - 127;
        let weight_bytes = (num_symbols + 1) / 2;
        if data.len() < 1 + weight_bytes {
            return Err(ZstdError::UnexpectedEof);
        }
        let mut weights = vec![0u8; num_symbols];
        for i in 0..num_symbols {
            let byte = data[1 + i / 2];
            weights[i] = if i % 2 == 0 { byte & 0x0F } else { byte >> 4 };
        }
        let table = HuffmanTable::from_weights(&weights)?;
        Ok((table, 1 + weight_bytes))
    }
}

/// Serialize a Huffman table header in direct (4-bit weight) format.
pub fn write_huffman_header(table: &HuffmanTable) -> Vec<u8> {
    let weights = table.to_weights();
    let num_symbols = weights.len();
    let weight_bytes = (num_symbols + 1) / 2;
    let mut out = Vec::with_capacity(1 + weight_bytes);
    out.push((num_symbols + 127) as u8);
    for i in (0..num_symbols).step_by(2) {
        let lo = weights[i];
        let hi = if i + 1 < num_symbols { weights[i + 1] } else { 0 };
        out.push(lo | (hi << 4));
    }
    out
}

/// Decode FSE-encoded Huffman weights.
fn decode_fse_weights(data: &[u8]) -> Result<Vec<u8>> {
    use crate::fse::{build_decode_table, read_distribution_table, BitReader};

    let (norm, accuracy_log, consumed) = read_distribution_table(data)?;
    let decode_table = build_decode_table(&norm, accuracy_log)?;

    let bitstream = &data[consumed..];
    if bitstream.is_empty() {
        return Ok(vec![]);
    }

    let mut reader = BitReader::new(bitstream);
    let table_size = 1usize << accuracy_log;

    // Initialize FSE state
    let state_bits = accuracy_log as u32;
    let mut state = reader.read_bits(state_bits) as usize;

    let mut weights = Vec::new();
    while !reader.is_empty() {
        let entry = &decode_table.table[state];
        weights.push(entry.symbol);
        let nb = entry.num_bits as u32;
        let extra = reader.read_bits(nb);
        state = entry.base_line as usize + extra as usize;
        if state >= table_size {
            break;
        }
    }
    // Decode final symbol
    let entry = &decode_table.table[state];
    weights.push(entry.symbol);

    Ok(weights)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_huffman_roundtrip() {
        let data: Vec<u8> = b"hello world, this is a test of huffman coding!".to_vec();
        let mut freqs = [0u32; MAX_SYMBOLS];
        for &b in &data {
            freqs[b as usize] += 1;
        }
        let table = HuffmanTable::from_frequencies(&freqs).unwrap();
        let encoded = table.encode(&data).unwrap();
        let total_bits: usize = data.iter().map(|&b| table.lengths[b as usize] as usize).sum();
        let decoded = table.decode(&encoded, total_bits, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_huffman_single_symbol() {
        let _data = vec![b'a'; 100];
        let mut freqs = [0u32; MAX_SYMBOLS];
        freqs[b'a' as usize] = 100;
        let table = HuffmanTable::from_frequencies(&freqs).unwrap();
        assert_eq!(table.lengths[b'a' as usize], 1);
    }

    #[test]
    fn test_weights_roundtrip() {
        let weights = vec![4u8, 3, 3, 2, 2, 2, 2, 0, 0];
        let table = HuffmanTable::from_weights(&weights).unwrap();
        let recovered = table.to_weights();
        // Trim to same length
        let min_len = weights.len().min(recovered.len());
        assert_eq!(&weights[..min_len], &recovered[..min_len]);
    }

    #[test]
    fn test_canonical_codes_ordered() {
        let mut freqs = [0u32; MAX_SYMBOLS];
        for (i, f) in [10u32, 8, 5, 3, 1].iter().enumerate() {
            freqs[i] = *f;
        }
        let table = HuffmanTable::from_frequencies(&freqs).unwrap();
        // Shorter codes should be assigned to more frequent symbols
        assert!(table.lengths[0] <= table.lengths[4]);
    }
}
