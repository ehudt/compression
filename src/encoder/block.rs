//! Compressed block encoder.
//!
//! Encodes a block of input data into the zstd compressed block format:
//! 1. Literals section (Huffman-coded literal bytes)
//! 2. Sequences section (FSE-coded sequence commands)
//!
//! # Note on design
//!
//! The block encoder parses LZ77 events and emits sequence commands using the
//! predefined FSE tables for literal-length, match-length, and offset codes.

use super::lz77::{MatchFinder, ParseSink, parse_ranges};
use crate::decoder::sequences::{decode_sequences, execute_sequences};
use crate::error::Result;
use crate::fse::{BitWriter, FseDecodeTable, build_decode_table};
use crate::huffman::{HuffmanTable, MAX_SYMBOLS, write_huffman_header};
use crate::tables::sequences::{
    LITERALS_LENGTH_DEFAULT_ACCURACY, LITERALS_LENGTH_DEFAULT_NORM, LITERALS_LENGTH_EXTRA,
    MATCH_LENGTH_DEFAULT_ACCURACY, MATCH_LENGTH_DEFAULT_NORM, MATCH_LENGTH_EXTRA,
    OFFSET_DEFAULT_ACCURACY, OFFSET_DEFAULT_NORM,
};
use std::sync::OnceLock;

#[derive(Debug)]
struct PredefinedFseEncoder {
    accuracy_log: u8,
    initial_state_by_symbol: Vec<u16>,
    transitions: Vec<Option<(u16, u16, u8)>>,
    table_size: usize,
}

#[derive(Debug, Clone, Copy)]
struct EncodedSequence {
    ll_code: usize,
    ll_extra: u32,
    ml_code: usize,
    ml_extra: u32,
    of_code: usize,
    of_extra: u32,
}

struct SequenceCollector<'a> {
    data: &'a [u8],
    literals: Vec<u8>,
    sequences: Vec<EncodedSequence>,
    pending_lit_len: usize,
    invalid: bool,
}

impl ParseSink for SequenceCollector<'_> {
    fn literals(&mut self, start: usize, end: usize) {
        if self.invalid {
            return;
        }
        self.literals.extend_from_slice(&self.data[start..end]);
        self.pending_lit_len += end - start;
    }

    fn matched(&mut self, _pos: usize, offset: usize, length: usize) {
        if self.invalid {
            return;
        }
        let lit_len = self.pending_lit_len;
        let Some((ll_code, ll_extra)) = literal_length_code(lit_len) else {
            self.invalid = true;
            return;
        };
        let Some((ml_code, ml_extra)) = match_length_code(length) else {
            self.invalid = true;
            return;
        };
        // Encode all offsets using the non-repeat path: raw_offset = offset + 3.
        let Some((of_code, of_extra)) = offset_code(offset) else {
            self.invalid = true;
            return;
        };

        self.sequences.push(EncodedSequence {
            ll_code,
            ll_extra,
            ml_code,
            ml_extra,
            of_code,
            of_extra,
        });
        self.pending_lit_len = 0;
    }
}

/// Encode a block of data into compressed form.
///
/// `full_data` is the entire frame input; `start..end` is the current block range.
/// `finder` is the frame-scoped match finder (carries cross-block history).
///
/// Returns the compressed bytes (without block header).
pub fn encode_block(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
) -> Result<Vec<u8>> {
    if start == end {
        // One-byte "0 raw literals" + one-byte "0 sequences"
        return Ok(vec![0x00, 0x00]);
    }

    let block_data = &full_data[start..end];
    let (mut literals, mut sequences) = collect_sequences(full_data, start, end, finder);
    let mut seq_section = encode_sequences(&sequences);
    if should_validate_sequences() && !sequences.is_empty() {
        // Prior history: up to one window worth of bytes preceding this block.
        let prior_start = start.saturating_sub(finder.window_size());
        let prior = &full_data[prior_start..start];
        if !validate_sequences(block_data, prior, &literals, &seq_section) {
            literals = block_data.to_vec();
            sequences.clear();
            seq_section = encode_sequences(&sequences);
        }
    }

    let mut out = encode_literals(&literals)?;
    out.extend_from_slice(&seq_section);
    Ok(out)
}

#[inline]
fn should_validate_sequences() -> bool {
    cfg!(debug_assertions)
}

/// Validate that `seq_section` + `literals` reconstructs `original` when applied
/// against `prior` history (the preceding window bytes from earlier blocks).
fn validate_sequences(
    original: &[u8],
    prior: &[u8],
    literals: &[u8],
    seq_section: &[u8],
) -> bool {
    std::panic::catch_unwind(|| {
        let Ok((decoded, _)) = decode_sequences(seq_section) else {
            return false;
        };
        // Seed the output buffer with prior history so cross-block offsets resolve.
        let mut reconstructed = prior.to_vec();
        let prior_len = reconstructed.len();
        if execute_sequences(&decoded, literals, &mut reconstructed).is_err() {
            return false;
        }
        reconstructed[prior_len..] == *original
    })
    .unwrap_or(false)
}

fn collect_sequences(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
) -> (Vec<u8>, Vec<EncodedSequence>) {
    let block_len = end - start;
    let mut collector = SequenceCollector {
        data: full_data,
        literals: Vec::with_capacity(block_len / 4),
        sequences: Vec::with_capacity(block_len / 32),
        pending_lit_len: 0,
        invalid: false,
    };
    parse_ranges(full_data, start, end, finder, &mut collector);

    if collector.invalid {
        (full_data[start..end].to_vec(), Vec::new())
    } else {
        (collector.literals, collector.sequences)
    }
}

fn literal_length_code(value: usize) -> Option<(usize, u32)> {
    static TABLE: OnceLock<Vec<(u8, u32)>> = OnceLock::new();
    lookup_length_code(
        value,
        TABLE.get_or_init(|| build_length_code_table(&LITERALS_LENGTH_EXTRA)),
    )
}

fn match_length_code(value: usize) -> Option<(usize, u32)> {
    static TABLE: OnceLock<Vec<(u8, u32)>> = OnceLock::new();
    lookup_length_code(
        value,
        TABLE.get_or_init(|| build_length_code_table(&MATCH_LENGTH_EXTRA)),
    )
}

fn offset_code(offset: usize) -> Option<(usize, u32)> {
    if offset == 0 {
        return None;
    }
    // zstd non-repeat offset: raw_offset = offset + 3
    // of_code = floor(log2(raw_offset)), of_extra = raw_offset - (1 << of_code)
    let raw = offset + 3;
    let code = usize::BITS as usize - 1 - raw.leading_zeros() as usize;
    if code >= OFFSET_DEFAULT_NORM.len() {
        return None; // offset too large for predefined table
    }
    Some((code, (raw - (1 << code)) as u32))
}

fn lookup_length_code(value: usize, table: &[(u8, u32)]) -> Option<(usize, u32)> {
    table
        .get(value)
        .copied()
        .map(|(code, extra)| (code as usize, extra))
}

fn build_length_code_table(table: &[(u32, u8)]) -> Vec<(u8, u32)> {
    let max_value = table
        .iter()
        .map(|&(base, extra_bits)| base as usize + ((1usize << extra_bits) - 1))
        .max()
        .unwrap_or(0);
    let mut lookup = vec![(0u8, 0u32); max_value + 1];
    for (code, &(base, extra_bits)) in table.iter().enumerate() {
        let span = 1usize << extra_bits;
        for extra in 0..span {
            lookup[base as usize + extra] = (code as u8, extra as u32);
        }
    }
    lookup
}


fn encode_sequences(sequences: &[EncodedSequence]) -> Vec<u8> {
    if sequences.is_empty() {
        return vec![0x00];
    }

    let ll_table = literal_length_encoder();
    let of_table = offset_encoder();
    let ml_table = match_length_encoder();
    let last = sequences[sequences.len() - 1];
    let mut ll_state = ll_table.initial_state(last.ll_code);
    let mut of_state = of_table.initial_state(last.of_code);
    let mut ml_state = ml_table.initial_state(last.ml_code);

    let mut out = Vec::new();
    write_sequence_count(&mut out, sequences.len());
    out.push(0x00); // predefined tables for LL/OF/ML

    // Encode sequences in reverse order (last sequence → lowest bits, first → highest).
    // Within each sequence block, extra bits go first (lower) and state transitions
    // go after (higher), so the decoder reading MSB-first sees transitions before extras.
    // Initial states are written last (highest bits); decoder reads them first.
    let mut bits = BitWriter::new();

    for i in (0..sequences.len()).rev() {
        let seq = sequences[i];
        // State transitions come before extras in the low bits, so the decoder
        // (reading MSB-first) sees extras before transitions for each sequence.
        // Transition write order (lowest→highest): OF, ML, LL → decoder reads LL, ML, OF.
        if i + 1 < sequences.len() {
            let (next_of_state, of_bits, of_nb) = of_table
                .transition(seq.of_code, of_state)
                .expect("no inverse transition found");
            bits.write_bits(of_bits as u64, of_nb as u32);
            let (next_ml_state, ml_bits, ml_nb) = ml_table
                .transition(seq.ml_code, ml_state)
                .expect("no inverse transition found");
            bits.write_bits(ml_bits as u64, ml_nb as u32);
            let (next_ll_state, ll_bits, ll_nb) = ll_table
                .transition(seq.ll_code, ll_state)
                .expect("no inverse transition found");
            bits.write_bits(ll_bits as u64, ll_nb as u32);
            of_state = next_of_state;
            ml_state = next_ml_state;
            ll_state = next_ll_state;
        }
        // Extra bits come above transitions (highest within seq group).
        // Write order (lowest→highest): LL, ML, OF → decoder reads OF, ML, LL.
        bits.write_bits(
            seq.ll_extra as u64,
            LITERALS_LENGTH_EXTRA[seq.ll_code].1 as u32,
        );
        bits.write_bits(
            seq.ml_extra as u64,
            MATCH_LENGTH_EXTRA[seq.ml_code].1 as u32,
        );
        bits.write_bits(seq.of_extra as u64, seq.of_code as u32);
    }

    // Initial states: ML (lowest), OF, LL (highest) so decoder reads LL first.
    bits.write_bits(ml_state as u64, ml_table.accuracy_log as u32);
    bits.write_bits(of_state as u64, of_table.accuracy_log as u32);
    bits.write_bits(ll_state as u64, ll_table.accuracy_log as u32);

    out.extend_from_slice(&bits.finish());
    out
}

fn write_sequence_count(out: &mut Vec<u8>, count: usize) {
    if count < 128 {
        out.push(count as u8);
    } else if count < 0x7F00 {
        out.push((128 + (count >> 8)) as u8);
        out.push((count & 0xFF) as u8);
    } else {
        let adjusted = count - 0x7F00;
        out.push(255);
        out.push((adjusted & 0xFF) as u8);
        out.push(((adjusted >> 8) & 0xFF) as u8);
    }
}

impl PredefinedFseEncoder {
    fn from_decode_table(table: &FseDecodeTable, symbol_count: usize) -> Self {
        let table_size = table.table.len();
        let mut initial_state_by_symbol = vec![u16::MAX; symbol_count];
        let mut transitions = vec![None; symbol_count * table_size];

        for (idx, entry) in table.table.iter().enumerate() {
            let symbol = entry.symbol as usize;
            if initial_state_by_symbol[symbol] == u16::MAX {
                initial_state_by_symbol[symbol] = idx as u16;
            }

            let base = entry.base_line as usize;
            let span = 1usize << entry.num_bits;
            for bits in 0..span {
                let next_state = base + bits;
                transitions[symbol * table_size + next_state] =
                    Some((idx as u16, bits as u16, entry.num_bits));
            }
        }

        Self {
            accuracy_log: table.accuracy_log,
            initial_state_by_symbol,
            transitions,
            table_size,
        }
    }

    #[inline]
    fn initial_state(&self, symbol: usize) -> u16 {
        self.initial_state_by_symbol[symbol]
    }

    #[inline]
    fn transition(&self, symbol: usize, next_state: u16) -> Option<(u16, u16, u8)> {
        self.transitions[symbol * self.table_size + next_state as usize]
    }
}

fn literal_length_encoder() -> &'static PredefinedFseEncoder {
    static TABLE: OnceLock<PredefinedFseEncoder> = OnceLock::new();
    TABLE.get_or_init(|| {
        let decode = build_decode_table(
            &LITERALS_LENGTH_DEFAULT_NORM,
            LITERALS_LENGTH_DEFAULT_ACCURACY,
        )
        .expect("valid predefined LL table");
        PredefinedFseEncoder::from_decode_table(&decode, LITERALS_LENGTH_EXTRA.len())
    })
}

fn offset_encoder() -> &'static PredefinedFseEncoder {
    static TABLE: OnceLock<PredefinedFseEncoder> = OnceLock::new();
    TABLE.get_or_init(|| {
        let decode = build_decode_table(&OFFSET_DEFAULT_NORM, OFFSET_DEFAULT_ACCURACY)
            .expect("valid predefined OF table");
        PredefinedFseEncoder::from_decode_table(&decode, OFFSET_DEFAULT_NORM.len())
    })
}

fn match_length_encoder() -> &'static PredefinedFseEncoder {
    static TABLE: OnceLock<PredefinedFseEncoder> = OnceLock::new();
    TABLE.get_or_init(|| {
        let decode = build_decode_table(&MATCH_LENGTH_DEFAULT_NORM, MATCH_LENGTH_DEFAULT_ACCURACY)
            .expect("valid predefined ML table");
        PredefinedFseEncoder::from_decode_table(&decode, MATCH_LENGTH_EXTRA.len())
    })
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

    // Build a Huffman table, then validate that the serialized weight header
    // can reconstruct the same coding model. If not, emit raw literals rather
    // than an invalid compressed-literals section.
    let table = match HuffmanTable::from_frequencies(&freqs) {
        Ok(t) => t,
        Err(_) => return encode_raw(data),
    };

    // Direct-mode Huffman header supports at most 128 stored weights
    // (header_byte 128..=255). If the table has more active symbols,
    // fall back to raw to avoid header overflow.
    let weights = table.to_weights();
    if weights.len() > 128 {
        return encode_raw(data);
    }

    let table = match HuffmanTable::from_weights(&weights) {
        Ok(table) => table,
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
    let mut out;

    if regen < (1 << 14) && comp < (1 << 14) {
        out = Vec::with_capacity(4 + comp);
        // sf=2: bits [3:2] = 0b10, type=2: bits[1:0]=0b10 → byte0 base = 0x0A
        out.push(0x0A | (((regen & 0xF) << 4) as u8));
        out.push(((regen >> 4) & 0xFF) as u8);
        out.push((((regen >> 12) & 0x3) | ((comp & 0x3F) << 2)) as u8);
        out.push(((comp >> 6) & 0xFF) as u8);
    } else {
        debug_assert!(regen < (1 << 18), "regen_size too large for 5-byte header");
        debug_assert!(comp < (1 << 18), "comp_size too large for 5-byte header");

        out = Vec::with_capacity(5 + comp);
        // sf=3: bits [3:2] = 0b11, type=2: bits[1:0]=0b10 → byte0 base = 0x0E
        out.push(0x0E | (((regen & 0xF) << 4) as u8));
        out.push(((regen >> 4) & 0xFF) as u8);
        out.push((((regen >> 12) & 0x3F) | ((comp & 0x3) << 6)) as u8);
        out.push(((comp >> 2) & 0xFF) as u8);
        out.push(((comp >> 10) & 0xFF) as u8);
    }

    out.extend_from_slice(huff_header);
    out.extend_from_slice(encoded);
    Ok(out)
}
