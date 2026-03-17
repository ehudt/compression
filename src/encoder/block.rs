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

use super::lz77::{Event, parse};
use crate::decoder::sequences::{decode_sequences, execute_sequences};
use crate::error::Result;
use crate::fse::{BitWriter, FseDecodeTable, build_decode_table};
use crate::huffman::{HuffmanTable, MAX_SYMBOLS, write_huffman_header};
use crate::tables::sequences::{
    LITERALS_LENGTH_DEFAULT_ACCURACY, LITERALS_LENGTH_DEFAULT_NORM, LITERALS_LENGTH_EXTRA,
    MATCH_LENGTH_DEFAULT_ACCURACY, MATCH_LENGTH_DEFAULT_NORM, MATCH_LENGTH_EXTRA,
    OFFSET_DEFAULT_ACCURACY, OFFSET_DEFAULT_NORM,
};

#[derive(Debug, Clone, Copy)]
struct EncodedSequence {
    ll_code: usize,
    ll_extra: u32,
    ml_code: usize,
    ml_extra: u32,
    of_code: usize,
    of_extra: u32,
}

/// Encode a block of data into compressed form.
///
/// Returns the compressed bytes (without block header).
pub fn encode_block(data: &[u8], cfg: &super::MatchConfig) -> Result<Vec<u8>> {
    if data.is_empty() {
        // One-byte "0 raw literals" + one-byte "0 sequences"
        return Ok(vec![0x00, 0x00]);
    }

    let (mut literals, mut sequences) = collect_sequences(data, cfg);
    let mut seq_section = encode_sequences(&sequences);
    if should_validate_sequences()
        && !sequences.is_empty()
        && !validate_sequences(data, &literals, &seq_section)
    {
        literals = data.to_vec();
        sequences.clear();
        seq_section = encode_sequences(&sequences);
    }

    let mut out = encode_literals(&literals)?;
    out.extend_from_slice(&seq_section);
    Ok(out)
}

#[inline]
fn should_validate_sequences() -> bool {
    cfg!(debug_assertions)
}

fn validate_sequences(original: &[u8], literals: &[u8], seq_section: &[u8]) -> bool {
    std::panic::catch_unwind(|| {
        let Ok((decoded, _)) = decode_sequences(seq_section) else {
            return false;
        };
        let mut reconstructed = Vec::new();
        if execute_sequences(&decoded, literals, &[], &mut reconstructed).is_err() {
            return false;
        }
        reconstructed == original
    })
    .unwrap_or(false)
}

fn collect_sequences(data: &[u8], cfg: &super::MatchConfig) -> (Vec<u8>, Vec<EncodedSequence>) {
    let events = parse(data, cfg);
    let mut literals = Vec::new();
    let mut sequences = Vec::new();
    let mut pending_lit_len = 0usize;

    for event in events {
        match event {
            Event::Literals(start, end) => {
                literals.extend_from_slice(&data[start..end]);
                pending_lit_len += end - start;
            }
            Event::Match {
                pos: _,
                offset,
                length,
            } => {
                let lit_len = pending_lit_len;
                let Some((ll_code, ll_extra)) = encode_length_code(lit_len, &LITERALS_LENGTH_EXTRA)
                else {
                    return (data.to_vec(), Vec::new());
                };
                let Some((ml_code, ml_extra)) = encode_length_code(length, &MATCH_LENGTH_EXTRA)
                else {
                    return (data.to_vec(), Vec::new());
                };
                // Encode all offsets using the non-repeat path: raw_offset = offset + 3.
                let raw_offset = offset + 3;
                let of_code = usize::BITS as usize - 1 - raw_offset.leading_zeros() as usize;
                if of_code >= OFFSET_DEFAULT_NORM.len() {
                    return (data.to_vec(), Vec::new());
                }
                let of_extra = raw_offset - (1usize << of_code);

                sequences.push(EncodedSequence {
                    ll_code,
                    ll_extra,
                    ml_code,
                    ml_extra,
                    of_code,
                    of_extra: of_extra as u32,
                });
                pending_lit_len = 0;
            }
        }
    }

    (literals, sequences)
}

fn encode_length_code(value: usize, table: &[(u32, u8)]) -> Option<(usize, u32)> {
    for (code, &(base, extra_bits)) in table.iter().enumerate().rev() {
        let span = if extra_bits == 0 {
            1u32
        } else {
            1u32 << extra_bits
        };
        if value as u32 >= base && (value as u32) < base + span {
            return Some((code, value as u32 - base));
        }
    }
    None
}

fn encode_sequences(sequences: &[EncodedSequence]) -> Vec<u8> {
    if sequences.is_empty() {
        return vec![0x00];
    }

    let ll_table = build_decode_table(
        &LITERALS_LENGTH_DEFAULT_NORM,
        LITERALS_LENGTH_DEFAULT_ACCURACY,
    )
    .expect("valid predefined LL table");
    let of_table = build_decode_table(&OFFSET_DEFAULT_NORM, OFFSET_DEFAULT_ACCURACY)
        .expect("valid predefined OF table");
    let ml_table = build_decode_table(&MATCH_LENGTH_DEFAULT_NORM, MATCH_LENGTH_DEFAULT_ACCURACY)
        .expect("valid predefined ML table");

    let ll_symbols: Vec<usize> = sequences.iter().map(|s| s.ll_code).collect();
    let of_symbols: Vec<usize> = sequences.iter().map(|s| s.of_code).collect();
    let ml_symbols: Vec<usize> = sequences.iter().map(|s| s.ml_code).collect();

    let (ll_states, ll_trans) = build_state_path(&ll_table, &ll_symbols);
    let (of_states, of_trans) = build_state_path(&of_table, &of_symbols);
    let (ml_states, ml_trans) = build_state_path(&ml_table, &ml_symbols);

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
            let (of_bits, of_nb) = of_trans[i];
            bits.write_bits(of_bits as u64, of_nb as u32);
            let (ml_bits, ml_nb) = ml_trans[i];
            bits.write_bits(ml_bits as u64, ml_nb as u32);
            let (ll_bits, ll_nb) = ll_trans[i];
            bits.write_bits(ll_bits as u64, ll_nb as u32);
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
    bits.write_bits(ml_states[0] as u64, ml_table.accuracy_log as u32);
    bits.write_bits(of_states[0] as u64, of_table.accuracy_log as u32);
    bits.write_bits(ll_states[0] as u64, ll_table.accuracy_log as u32);

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

fn build_state_path(table: &FseDecodeTable, symbols: &[usize]) -> (Vec<usize>, Vec<(u16, u8)>) {
    let n = symbols.len();
    let mut states = vec![0usize; n];
    let mut transitions = vec![(0u16, 0u8); n.saturating_sub(1)];

    let last_sym = symbols[n - 1] as u8;
    states[n - 1] = table
        .table
        .iter()
        .position(|e| e.symbol == last_sym)
        .expect("symbol must exist in predefined table");

    for i in (0..n - 1).rev() {
        let target = states[i + 1] as u16;
        let sym = symbols[i] as u8;
        let (prev_state, bits, nb) =
            inverse_transition(table, sym, target).expect("no inverse transition found");
        states[i] = prev_state as usize;
        transitions[i] = (bits, nb);
    }

    (states, transitions)
}

fn inverse_transition(
    table: &FseDecodeTable,
    symbol: u8,
    next_state: u16,
) -> Option<(u16, u16, u8)> {
    for (idx, entry) in table.table.iter().enumerate() {
        if entry.symbol != symbol {
            continue;
        }
        let span = 1u16 << entry.num_bits;
        if next_state >= entry.base_line && next_state < entry.base_line + span {
            return Some((idx as u16, next_state - entry.base_line, entry.num_bits));
        }
    }
    None
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

    // Direct-mode Huffman header supports at most 128 stored weights
    // (header_byte 128..=255). If the table has more active symbols,
    // fall back to raw to avoid header overflow.
    let weights = table.to_weights();
    if weights.len() > 128 {
        return encode_raw(data);
    }

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
