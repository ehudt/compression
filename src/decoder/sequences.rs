//! Decoding of the sequences section within a compressed block.
//!
//! Sequences describe copy commands: (literal_length, match_length, offset).
//! Applying them reconstructs the original data.

use crate::error::{Result, ZstdError};
use crate::fse::{BitReader, FseDecodeTable, build_decode_table, read_distribution_table};
use crate::tables::sequences::{
    LITERALS_LENGTH_DEFAULT_ACCURACY, LITERALS_LENGTH_DEFAULT_NORM, LITERALS_LENGTH_EXTRA,
    MATCH_LENGTH_DEFAULT_ACCURACY, MATCH_LENGTH_DEFAULT_NORM, MATCH_LENGTH_EXTRA,
    OFFSET_DEFAULT_ACCURACY, OFFSET_DEFAULT_NORM,
};

/// A single decoded sequence command.
#[derive(Debug, Clone, Copy)]
pub struct Sequence {
    /// Number of literal bytes to copy before the match.
    pub literal_length: usize,
    /// Number of bytes to copy from the match.
    pub match_length: usize,
    /// Offset (back-reference distance from current output position).
    pub offset: usize,
}

/// Decode the sequences section of a compressed block.
///
/// `data` is the raw sequences section bytes.
/// Returns the list of sequences and the number of bytes consumed.
pub fn decode_sequences(data: &[u8]) -> Result<(Vec<Sequence>, usize)> {
    let mut repeat_offsets = [1usize, 4, 8];
    decode_sequences_with_offsets(data, &mut repeat_offsets)
}

/// Decode sequences, threading repeat-offset state across calls (for multi-block frames).
pub fn decode_sequences_with_offsets(
    data: &[u8],
    repeat_offsets: &mut [usize; 3],
) -> Result<(Vec<Sequence>, usize)> {
    if data.is_empty() {
        return Ok((vec![], 0));
    }

    // Byte 0: number of sequences (special encoding)
    let (num_sequences, mut offset) = read_sequence_count(data)?;
    if num_sequences == 0 {
        return Ok((vec![], offset));
    }

    // Byte at `offset`: symbol compression modes
    if data.len() <= offset {
        return Err(ZstdError::UnexpectedEof);
    }
    let mode_byte = data[offset];
    offset += 1;

    let ll_mode = (mode_byte >> 6) & 0x3;
    let of_mode = (mode_byte >> 4) & 0x3;
    let ml_mode = (mode_byte >> 2) & 0x3;

    // Build decode tables for each symbol type
    let (ll_table, of_table, ml_table, tables_size) =
        read_sequence_tables(&data[offset..], ll_mode, of_mode, ml_mode)?;
    offset += tables_size;

    // Remaining bytes form the FSE bitstream (read backwards)
    let bitstream = &data[offset..];
    let sequences = decode_sequence_bitstream(
        bitstream,
        num_sequences,
        &ll_table,
        &of_table,
        &ml_table,
        repeat_offsets,
    )?;

    Ok((sequences, data.len()))
}

fn read_sequence_count(data: &[u8]) -> Result<(usize, usize)> {
    let b0 = data[0] as usize;
    if b0 == 0 {
        Ok((0, 1))
    } else if b0 < 128 {
        Ok((b0, 1))
    } else if b0 < 255 {
        if data.len() < 2 {
            return Err(ZstdError::UnexpectedEof);
        }
        let count = ((b0 - 128) << 8) + data[1] as usize;
        Ok((count, 2))
    } else {
        if data.len() < 3 {
            return Err(ZstdError::UnexpectedEof);
        }
        let count = data[1] as usize + (data[2] as usize * 256) + 0x7F00;
        Ok((count, 3))
    }
}

fn read_sequence_tables(
    data: &[u8],
    ll_mode: u8,
    of_mode: u8,
    ml_mode: u8,
) -> Result<(FseDecodeTable, FseDecodeTable, FseDecodeTable, usize)> {
    let mut offset = 0;

    let (ll_table, consumed) = build_mode_table(data, ll_mode, TableType::LiteralLength)?;
    offset += consumed;
    let (of_table, consumed) = build_mode_table(&data[offset..], of_mode, TableType::Offset)?;
    offset += consumed;
    let (ml_table, consumed) = build_mode_table(&data[offset..], ml_mode, TableType::MatchLength)?;
    offset += consumed;

    Ok((ll_table, of_table, ml_table, offset))
}

#[derive(Clone, Copy)]
enum TableType {
    LiteralLength,
    Offset,
    MatchLength,
}

fn build_mode_table(data: &[u8], mode: u8, ty: TableType) -> Result<(FseDecodeTable, usize)> {
    match mode {
        0 => {
            // Predefined table
            let (norm, accuracy) = match ty {
                TableType::LiteralLength => (
                    LITERALS_LENGTH_DEFAULT_NORM.to_vec(),
                    LITERALS_LENGTH_DEFAULT_ACCURACY,
                ),
                TableType::Offset => (OFFSET_DEFAULT_NORM.to_vec(), OFFSET_DEFAULT_ACCURACY),
                TableType::MatchLength => (
                    MATCH_LENGTH_DEFAULT_NORM.to_vec(),
                    MATCH_LENGTH_DEFAULT_ACCURACY,
                ),
            };
            Ok((build_decode_table(&norm, accuracy)?, 0))
        }
        1 => {
            // RLE: single symbol
            if data.is_empty() {
                return Err(ZstdError::UnexpectedEof);
            }
            let sym = data[0];
            // Build a trivial table: accuracy_log=0 means table_size=1
            let table = build_rle_table(sym);
            Ok((table, 1))
        }
        2 => {
            // FSE-compressed table
            let (norm, accuracy, consumed) = read_distribution_table(data)?;
            Ok((build_decode_table(&norm, accuracy)?, consumed))
        }
        3 => {
            // Repeat: caller must handle; return a sentinel
            Err(ZstdError::SequenceError(
                "repeat mode not handled at this level",
            ))
        }
        _ => Err(ZstdError::SequenceError("unknown mode")),
    }
}

fn build_rle_table(sym: u8) -> FseDecodeTable {
    use crate::fse::FseDecodeEntry;
    FseDecodeTable {
        accuracy_log: 0,
        table: vec![FseDecodeEntry {
            symbol: sym,
            num_bits: 0,
            base_line: 0,
            next_state: 0,
        }],
    }
}

fn decode_sequence_bitstream(
    data: &[u8],
    num_sequences: usize,
    ll_table: &FseDecodeTable,
    of_table: &FseDecodeTable,
    ml_table: &FseDecodeTable,
    repeat_offsets: &mut [usize; 3],
) -> Result<Vec<Sequence>> {
    let mut reader = BitReader::new(data);
    let ll_log = ll_table.accuracy_log as u32;
    let of_log = of_table.accuracy_log as u32;
    let ml_log = ml_table.accuracy_log as u32;

    // Initialize states
    let mut ll_state = reader.read_bits(ll_log) as usize;
    let mut of_state = reader.read_bits(of_log) as usize;
    let mut ml_state = reader.read_bits(ml_log) as usize;

    let mut sequences = Vec::with_capacity(num_sequences);

    for seq_idx in 0..num_sequences {
        // Read offset code
        let of_entry = &of_table.table[of_state];
        let of_code = of_entry.symbol as usize;
        // Read match length code
        let ml_entry = &ml_table.table[ml_state];
        let ml_code = ml_entry.symbol as usize;
        // Read literal length code
        let ll_entry = &ll_table.table[ll_state];
        let ll_code = ll_entry.symbol as usize;

        // zstd bitstream order (per spec §3.1.1.3.2, confirmed by reference decoder):
        // For each sequence: extra bits first (OF, ML, LL), then state transitions (LL, ML, OF).
        // State transitions are skipped for the last sequence.

        // Decode offset extra bits first
        let raw_offset = if of_code <= 31 {
            let extra_bits = of_code as u32;
            let extra = reader.read_bits(extra_bits) as usize;
            (1usize << of_code) + extra
        } else {
            return Err(ZstdError::SequenceError("offset code too large"));
        };

        // Handle repeat offsets
        let offset = if raw_offset <= 3 {
            let rep_idx = raw_offset - 1;
            if ll_code == 0 {
                // Special: if ll_code == 0, repeat offsets are shifted
                if rep_idx == 2 {
                    let o = repeat_offsets[0] - 1;
                    repeat_offsets[2] = repeat_offsets[1];
                    repeat_offsets[1] = repeat_offsets[0];
                    repeat_offsets[0] = o;
                    o
                } else {
                    let o = repeat_offsets[rep_idx + 1];
                    if rep_idx == 1 {
                        repeat_offsets[2] = repeat_offsets[1];
                    }
                    repeat_offsets[1] = repeat_offsets[0];
                    repeat_offsets[0] = o;
                    o
                }
            } else {
                let o = repeat_offsets[rep_idx];
                // Rotate
                for i in (1..=rep_idx).rev() {
                    repeat_offsets[i] = repeat_offsets[i - 1];
                }
                repeat_offsets[0] = o;
                o
            }
        } else {
            let o = raw_offset - 3;
            repeat_offsets[2] = repeat_offsets[1];
            repeat_offsets[1] = repeat_offsets[0];
            repeat_offsets[0] = o;
            o
        };

        // Decode match length extra bits (second, after OF extra)
        if ml_code >= MATCH_LENGTH_EXTRA.len() {
            return Err(ZstdError::SequenceError("ml_code out of range"));
        }
        let (ml_base, ml_extra_bits) = MATCH_LENGTH_EXTRA[ml_code];
        let ml_extra = reader.read_bits(ml_extra_bits as u32) as u32;
        let match_length = (ml_base + ml_extra) as usize;

        // Decode literal length extra bits (third, after ML extra)
        if ll_code >= LITERALS_LENGTH_EXTRA.len() {
            return Err(ZstdError::SequenceError("ll_code out of range"));
        }
        let (ll_base, ll_extra_bits) = LITERALS_LENGTH_EXTRA[ll_code];
        let ll_extra = reader.read_bits(ll_extra_bits as u32) as u32;
        let literal_length = (ll_base + ll_extra) as usize;

        // Advance FSE states AFTER reading extra bits (for non-last sequences).
        // Read order: LL transition, ML transition, OF transition.
        if seq_idx + 1 < num_sequences {
            let ll_nb = ll_entry.num_bits as u32;
            let ll_extra_state = reader.read_bits(ll_nb) as usize;
            ll_state = ll_entry.base_line as usize + ll_extra_state;

            let ml_nb = ml_entry.num_bits as u32;
            let ml_extra_state = reader.read_bits(ml_nb) as usize;
            ml_state = ml_entry.base_line as usize + ml_extra_state;

            let of_nb = of_entry.num_bits as u32;
            let of_extra_state = reader.read_bits(of_nb) as usize;
            of_state = of_entry.base_line as usize + of_extra_state;
        }

        sequences.push(Sequence {
            literal_length,
            match_length,
            offset,
        });
    }

    Ok(sequences)
}

/// Apply decoded sequences to reconstruct the original data.
///
/// `literals` is the decoded literals buffer.
/// `history` is the previous decompressed data (window).
pub fn execute_sequences(
    sequences: &[Sequence],
    literals: &[u8],
    history: &[u8],
    output: &mut Vec<u8>,
) -> Result<()> {
    let mut lit_pos = 0usize;

    for seq in sequences {
        // Copy literal bytes
        let lit_end = lit_pos + seq.literal_length;
        if lit_end > literals.len() {
            return Err(ZstdError::CorruptData("literal position out of bounds"));
        }
        output.extend_from_slice(&literals[lit_pos..lit_end]);
        lit_pos = lit_end;

        // Copy from match
        if seq.offset == 0 {
            return Err(ZstdError::CorruptData("zero offset"));
        }

        let current_len = output.len();
        let total_available = current_len + history.len();
        if seq.offset > total_available {
            return Err(ZstdError::CorruptData("offset exceeds available history"));
        }

        // The back-reference may overlap with what we're currently writing (self-referential)
        for i in 0..seq.match_length {
            let back_pos = current_len + i;
            let src_pos = back_pos as isize - seq.offset as isize;
            let byte = if src_pos < 0 {
                let hist_pos = (history.len() as isize + src_pos) as usize;
                if hist_pos >= history.len() {
                    return Err(ZstdError::CorruptData("history reference out of bounds"));
                }
                history[hist_pos]
            } else {
                output[src_pos as usize]
            };
            output.push(byte);
        }
    }

    // Copy any trailing literals
    if lit_pos < literals.len() {
        output.extend_from_slice(&literals[lit_pos..]);
    }

    Ok(())
}

#[cfg(test)]
mod debug_tests {
    use super::*;
    use crate::fse::BitReader;
    use crate::tables::sequences::*;

    #[test]
    fn inspect_predefined_tables() {
        let ll_table = build_decode_table(&LITERALS_LENGTH_DEFAULT_NORM.to_vec(), LITERALS_LENGTH_DEFAULT_ACCURACY).unwrap();
        let of_table = build_decode_table(&OFFSET_DEFAULT_NORM.to_vec(), OFFSET_DEFAULT_ACCURACY).unwrap();
        let ml_table = build_decode_table(&MATCH_LENGTH_DEFAULT_NORM.to_vec(), MATCH_LENGTH_DEFAULT_ACCURACY).unwrap();

        // States from the level9 bitstream (ll=14, of=10, ml=44)
        eprintln!("LL[14] = sym={} nb={} base={}", ll_table.table[14].symbol, ll_table.table[14].num_bits, ll_table.table[14].base_line);
        eprintln!("OF[10] = sym={} nb={} base={}", of_table.table[10].symbol, of_table.table[10].num_bits, of_table.table[10].base_line);
        eprintln!("ML[44] = sym={} nb={} base={}", ml_table.table[44].symbol, ml_table.table[44].num_bits, ml_table.table[44].base_line);

        // Print all table entries
        eprintln!("=== LL table ===");
        for (i, e) in ll_table.table.iter().enumerate() {
            eprintln!("  LL[{:2}] sym={:2} nb={} base={:2}", i, e.symbol, e.num_bits, e.base_line);
        }
        eprintln!("=== OF table ===");
        for (i, e) in of_table.table.iter().enumerate() {
            eprintln!("  OF[{:2}] sym={:2} nb={} base={:2}", i, e.symbol, e.num_bits, e.base_line);
        }
        eprintln!("=== ML table ===");
        for (i, e) in ml_table.table.iter().enumerate() {
            eprintln!("  ML[{:2}] sym={:2} nb={} base={:2}", i, e.symbol, e.num_bits, e.base_line);
        }
    }

    #[test]
    fn decode_level9_bitstream() {
        // The actual level9 bitstream for repetitive_text(32768)
        // 2 sequences, predefined tables
        let seq_section = &[0x02u8, 0x00, 0xd0, 0x3f, 0x54, 0x8b, 0x16, 0xac, 0x72, 0x02];
        match decode_sequences(seq_section) {
            Ok((seqs, _)) => {
                for (i, seq) in seqs.iter().enumerate() {
                    eprintln!("Seq {}: ll={}, ml={}, offset={}", i, seq.literal_length, seq.match_length, seq.offset);
                }
            }
            Err(e) => eprintln!("ERROR: {}", e),
        }
    }

    #[test]
    fn debug_reference_bitstream() {
        // Reference bitstream for "hello world " * 100 compressed at level 1
        // Expected: ll=12, ml=1188, offset=12
        let seq_section = &[0x01u8, 0x00, 0xa1, 0xfc, 0x2f, 0x49];
        
        let (seqs, _) = decode_sequences(seq_section).expect("decode failed");
        
        for (i, seq) in seqs.iter().enumerate() {
            eprintln!("Seq {}: ll={}, ml={}, offset={}", i, seq.literal_length, seq.match_length, seq.offset);
        }
        
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0].literal_length, 12, "literal_length mismatch");
        assert_eq!(seqs[0].match_length, 1188, "match_length mismatch");
        assert_eq!(seqs[0].offset, 12, "offset mismatch");
    }
}
