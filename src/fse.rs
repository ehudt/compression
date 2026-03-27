//! Finite State Entropy (FSE) encoding and decoding.
//!
//! FSE is an asymmetric numeral systems (ANS)-based entropy coder used
//! throughout zstd for encoding literal lengths, match lengths, and offsets.
//!
//! # References
//! - <https://github.com/facebook/zstd/blob/dev/doc/zstd_compression_format.md#fse-table-description>
//! - Jarek Duda's ANS paper: <https://arxiv.org/abs/1311.2540>

use crate::error::{Result, ZstdError};

/// Maximum FSE table log (accuracy_log).
pub const FSE_MAX_TABLELOG: u8 = 12;
/// Minimum FSE table log.
pub const FSE_MIN_TABLELOG: u8 = 5;

/// An FSE symbol with its normalized probability count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FseSymbol {
    /// Normalized probability (number of cells in the table). -1 means "less-than-1" probability.
    pub norm: i16,
}

/// A decoded FSE state used during decompression.
#[derive(Debug, Clone, Copy)]
pub struct FseDecodeEntry {
    /// The decoded symbol.
    pub symbol: u8,
    /// Number of bits to read to advance the state.
    pub num_bits: u8,
    /// Base value to add after reading bits.
    pub base_line: u16,
    /// Next state after advancing.
    pub next_state: u16,
}

/// An FSE decode table.
#[derive(Debug, Clone)]
pub struct FseDecodeTable {
    pub accuracy_log: u8,
    pub table: Vec<FseDecodeEntry>,
}

/// An FSE encode table entry.
#[derive(Debug, Clone, Copy)]
pub struct FseEncodeEntry {
    /// Delta to apply to the state.
    pub delta_find_state: i32,
    /// Number of bits to output.
    pub num_bits: u8,
    /// Minimum state value requiring this many bits.
    pub find_state_min: u16,
}

/// An FSE encode table.
#[derive(Debug, Clone)]
pub struct FseEncodeTable {
    pub accuracy_log: u8,
    pub table: Vec<FseEncodeEntry>,
}

/// Normalize symbol frequencies into FSE probabilities summing to `1 << accuracy_log`.
///
/// Returns a vector of normalized counts indexed by symbol value.
pub fn normalize_counts(counts: &[u32], total: u32, accuracy_log: u8) -> Result<Vec<i16>> {
    if accuracy_log < FSE_MIN_TABLELOG || accuracy_log > FSE_MAX_TABLELOG {
        return Err(ZstdError::FseError("accuracy_log out of range"));
    }
    let table_size = 1u32 << accuracy_log;
    let mut norm = vec![0i16; counts.len()];
    let mut sum: i32 = 0;
    let mut largest_idx = 0usize;
    let mut largest_count = 0u32;

    for (i, &c) in counts.iter().enumerate() {
        if c == 0 {
            norm[i] = 0;
            continue;
        }
        if c == total {
            // Only one symbol — assign entire table to it
            norm[i] = table_size as i16;
            return Ok(norm);
        }
        // Scale proportionally, guarantee at least 1
        let scaled = ((c as u64 * table_size as u64) / total as u64) as i16;
        norm[i] = scaled.max(1);
        sum += norm[i] as i32;
        if c > largest_count {
            largest_count = c;
            largest_idx = i;
        }
    }

    // Adjust for rounding error: assign remainder/excess to the most-frequent symbol
    let remainder = table_size as i32 - sum;
    norm[largest_idx] = (norm[largest_idx] as i32 + remainder) as i16;

    Ok(norm)
}

/// Build an FSE decode table from normalized counts.
pub fn build_decode_table(norm: &[i16], accuracy_log: u8) -> Result<FseDecodeTable> {
    if accuracy_log > FSE_MAX_TABLELOG {
        return Err(ZstdError::FseError("accuracy_log too large"));
    }
    let table_size = 1usize << accuracy_log;
    let high_threshold = table_size - 1;

    // Phase 1: spread symbols across the table using the zstd "spread" formula.
    // step = (table_size >> 1) + (table_size >> 3) + 3
    let step = (table_size >> 1) + (table_size >> 3) + 3;
    let mask = table_size - 1;

    let mut symbol_next = vec![0u16; norm.len()];
    let mut table_symbol = vec![0u8; table_size];

    // First, place "less-than-1-probability" symbols at the end (high threshold positions)
    let mut pos = high_threshold;
    for (sym, &n) in norm.iter().enumerate() {
        if n == -1 {
            table_symbol[pos] = sym as u8;
            if pos == 0 {
                break;
            }
            pos -= 1;
            symbol_next[sym] = 1;
        }
    }

    // Spread remaining symbols
    let mut spread_pos = 0usize;
    for (sym, &n) in norm.iter().enumerate() {
        if n <= 0 {
            continue;
        }
        symbol_next[sym] = n as u16;
        for _ in 0..n {
            table_symbol[spread_pos] = sym as u8;
            spread_pos = (spread_pos + step) & mask;
            // Skip positions reserved for low-prob symbols
            while spread_pos > pos {
                spread_pos = (spread_pos + step) & mask;
            }
        }
    }

    // Phase 2: Build decode entries
    let mut table = vec![
        FseDecodeEntry {
            symbol: 0,
            num_bits: 0,
            base_line: 0,
            next_state: 0,
        };
        table_size
    ];

    for (i, &sym) in table_symbol.iter().enumerate() {
        let x = symbol_next[sym as usize];
        symbol_next[sym as usize] += 1;
        let num_bits = (accuracy_log as u16 - x.ilog2() as u16) as u8;
        let base_line = ((x as u32) << num_bits) as u16 - table_size as u16;
        table[i] = FseDecodeEntry {
            symbol: sym,
            num_bits,
            base_line,
            next_state: x,
        };
    }

    Ok(FseDecodeTable {
        accuracy_log,
        table,
    })
}

/// Build an FSE encode table from normalized counts.
pub fn build_encode_table(norm: &[i16], accuracy_log: u8) -> Result<FseEncodeTable> {
    let table_size = 1i32 << accuracy_log;
    let mut entries = vec![
        FseEncodeEntry {
            delta_find_state: 0,
            num_bits: 0,
            find_state_min: 0,
        };
        norm.len()
    ];

    let mut cumul = 0i32;
    for (sym, &n) in norm.iter().enumerate() {
        if n == 0 {
            continue;
        }
        // -1 means "less-than-1" probability: gets exactly 1 cell in the table.
        let count = if n == -1 { 1i32 } else { n as i32 };
        entries[sym].delta_find_state = cumul - count;
        cumul += count;
        if n == -1 {
            // Low-probability symbol: emit the full state as bits.
            entries[sym].num_bits = accuracy_log;
            entries[sym].find_state_min = 0;
        } else {
            let nb = accuracy_log - (n as u16).ilog2() as u8;
            entries[sym].num_bits = nb;
            entries[sym].find_state_min = ((n as u16) << nb) - (1 << accuracy_log);
        }
    }
    debug_assert_eq!(cumul, table_size, "norm sum mismatch");

    Ok(FseEncodeTable {
        accuracy_log,
        table: entries,
    })
}

/// Read an FSE distribution table from a bitstream (zstd format).
///
/// Returns `(norm_counts, accuracy_log, bytes_consumed)`.
pub fn read_distribution_table(data: &[u8]) -> Result<(Vec<i16>, u8, usize)> {
    if data.is_empty() {
        return Err(ZstdError::UnexpectedEof);
    }

    let accuracy_log = (data[0] & 0x0F) + 5;
    if accuracy_log > FSE_MAX_TABLELOG {
        return Err(ZstdError::FseError("accuracy_log too large in header"));
    }

    let table_size = 1i32 << accuracy_log;
    let mut remaining = table_size + 1;
    let mut threshold = table_size;
    let mut norm: Vec<i16> = Vec::new();

    let mut bits_read = 4u32; // consumed the low 4 bits of first byte
    let mut bit_buf = (data[0] >> 4) as u64;
    let mut byte_idx = 1usize;

    let nb_bits_needed = |thresh: i32| -> u32 { (thresh + 1).ilog2() + 1 };

    let fill_buf = |buf: &mut u64, bits: &mut u32, idx: &mut usize| -> Result<()> {
        while *bits < 16 {
            if *idx >= data.len() {
                return Err(ZstdError::UnexpectedEof);
            }
            *buf |= (data[*idx] as u64) << *bits;
            *bits += 8;
            *idx += 1;
        }
        Ok(())
    };

    while remaining > 1 {
        fill_buf(&mut bit_buf, &mut bits_read, &mut byte_idx)?;

        let nb = nb_bits_needed(threshold);
        let raw = bit_buf & ((1 << nb) - 1);
        bit_buf >>= nb;
        bits_read -= nb;

        let count = raw as i16 - 1;
        norm.push(count);
        remaining -= if count == -1 { 1 } else { count as i32 };

        if count == 0 {
            // Check for repeat-zero run-length encoding (pairs of 2-bit flags)
            loop {
                fill_buf(&mut bit_buf, &mut bits_read, &mut byte_idx)?;
                let repeat = bit_buf & 0x3;
                bit_buf >>= 2;
                bits_read -= 2;
                for _ in 0..repeat {
                    norm.push(0);
                }
                if repeat != 3 {
                    break;
                }
            }
        }

        threshold = remaining - 1;
        if threshold < 1 {
            break;
        }
    }

    // Last symbol gets all remaining probability
    if remaining > 1 {
        norm.push((remaining - 1) as i16);
    }

    // Align to byte boundary
    bits_read = bits_read % 8;
    if bits_read > 0 {
        // partial byte was consumed
        // byte_idx points one past the last fully consumed byte; the partial
        // bits are already inside bit_buf, which we discard.
    }
    // byte_idx is already past the partial byte because we filled by whole bytes.

    Ok((norm, accuracy_log, byte_idx))
}

/// A bit-stream reader used during FSE decoding.
///
/// Implements the zstd backward bitstream format: bytes are loaded as a
/// little-endian integer (byte[0] at bit 0, byte[last] at high bits).
/// The sentinel bit (highest set bit in the stream) is stripped on init.
/// Bits are then read from the HIGH end downward (MSB-first).
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Index of the start of the currently loaded 8-byte window.
    ptr: usize,
    /// 8 bytes loaded as LE u64 from data[ptr..ptr+8].
    bit_container: u64,
    /// Number of bits consumed from the MSB of bit_container.
    bits_consumed: u32,
}

impl<'a> BitReader<'a> {
    /// Create a new bit-reader.  Loads the last ≤8 bytes as a LE u64,
    /// finds the sentinel (highest set bit), and positions the reader
    /// just past it.
    pub fn new(data: &'a [u8]) -> Self {
        let n = data.len();
        if n == 0 {
            return Self {
                data,
                ptr: 0,
                bit_container: 0,
                bits_consumed: 64,
            };
        }

        let (ptr, bit_container) = if n >= 8 {
            let ptr = n - 8;
            (ptr, load_bitreader_window(data, ptr))
        } else {
            (0usize, load_bitreader_window(data, 0))
        };

        // Sentinel = highest set bit.  Consume it and all zero bits above it.
        let bits_consumed = 1 + bit_container.leading_zeros();

        Self {
            data,
            ptr,
            bit_container,
            bits_consumed,
        }
    }

    /// Reload the window: move ptr backward so that sub-byte alignment is
    /// maintained, then re-load 8 bytes as a fresh LE u64.
    fn reload(&mut self) {
        let full_bytes = (self.bits_consumed / 8) as usize;
        if full_bytes == 0 || self.ptr == 0 {
            return;
        }
        let go_back = full_bytes.min(self.ptr);
        self.ptr -= go_back;
        self.bits_consumed -= (go_back * 8) as u32;

        self.bit_container = load_bitreader_window(self.data, self.ptr);
    }

    /// Read `n` bits from the HIGH end (MSB-first).
    pub fn read_bits(&mut self, n: u32) -> u64 {
        if n == 0 {
            return 0;
        }
        let val = (self.bit_container << self.bits_consumed) >> (64 - n);
        self.bits_consumed += n;
        self.reload();
        val
    }

    /// Number of bits still available.
    pub fn bits_left(&self) -> u32 {
        64u32.saturating_sub(self.bits_consumed) + (self.ptr as u32) * 8
    }

    pub fn is_empty(&self) -> bool {
        self.bits_consumed >= 64 && self.ptr == 0
    }
}

#[inline]
fn load_bitreader_window(data: &[u8], ptr: usize) -> u64 {
    let avail = data.len() - ptr;
    if avail >= 8 {
        unsafe { u64::from_le(data.as_ptr().add(ptr).cast::<u64>().read_unaligned()) }
    } else {
        let mut bytes = [0u8; 8];
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr().add(ptr), bytes.as_mut_ptr(), avail);
        }
        u64::from_le_bytes(bytes)
    }
}

/// A bit-stream writer (LSB first, reversed) used during FSE encoding.
pub struct BitWriter {
    buf: Vec<u8>,
    pending: u64,
    pending_bits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            pending: 0,
            pending_bits: 0,
        }
    }

    /// Append `n` bits from `val` (LSB first).
    pub fn write_bits(&mut self, val: u64, n: u32) {
        debug_assert!(n <= 56);
        self.pending |= val << self.pending_bits;
        self.pending_bits += n;
        while self.pending_bits >= 8 {
            self.buf.push(self.pending as u8);
            self.pending >>= 8;
            self.pending_bits -= 8;
        }
    }

    /// Flush remaining bits with a sentinel '1' bit and pad to byte boundary.
    ///
    /// The zstd backward bitstream has the sentinel in the LAST (highest-address)
    /// byte.  We write bits LSB-first into a growing buffer, so the sentinel
    /// naturally ends up in the last flushed byte — no reversal needed.
    pub fn finish(mut self) -> Vec<u8> {
        // Write sentinel bit
        self.pending |= 1u64 << self.pending_bits;
        self.pending_bits += 1;
        while self.pending_bits > 0 {
            self.buf.push(self.pending as u8);
            self.pending >>= 8;
            self.pending_bits = self.pending_bits.saturating_sub(8);
        }
        self.buf
    }
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bit_writer_reader_roundtrip() {
        // BitWriter accumulates bits LSB-first; BitReader reads MSB-first.
        // So the decoder reads in the reverse order of writes.
        let mut w = BitWriter::new();
        w.write_bits(0b11, 2); // written first → lowest bits → read last
        w.write_bits(0b1101, 4); // written second → middle bits
        w.write_bits(0b101, 3); // written last → highest bits → read first
        let data = w.finish();

        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(3), 0b101); // reads highest bits first
        assert_eq!(r.read_bits(4), 0b1101);
        assert_eq!(r.read_bits(2), 0b11); // reads lowest bits last
    }

    #[test]
    fn test_normalize_single_symbol() {
        let counts = vec![100u32];
        let norm = normalize_counts(&counts, 100, 6).unwrap();
        assert_eq!(norm[0], 64);
    }

    #[test]
    fn test_normalize_two_symbols_equal() {
        let counts = vec![50u32, 50];
        let norm = normalize_counts(&counts, 100, 6).unwrap();
        assert_eq!(norm[0] + norm[1], 64);
        assert!(norm[0] > 0 && norm[1] > 0);
    }

    #[test]
    fn test_build_decode_table_basic() {
        let norm = vec![32i16, 32];
        let table = build_decode_table(&norm, 6).unwrap();
        assert_eq!(table.table.len(), 64);
        // Every entry should be symbol 0 or 1
        for entry in &table.table {
            assert!(entry.symbol <= 1);
        }
    }
}
