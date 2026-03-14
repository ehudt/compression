//! Block-level and frame-level decoder.

pub mod literals;
pub mod sequences;

use self::literals::decode_literals;
use self::sequences::{decode_sequences, execute_sequences};
use crate::error::{Result, ZstdError};

/// Decode a single zstd block.
///
/// `data`    — raw block bytes (after the 3-byte block header).
/// `block_type` — 0=raw, 1=RLE, 2=compressed.
/// `block_size` — declared regenerated size for RLE blocks.
/// `history`  — decompressed bytes from previous blocks (sliding window).
/// `output`   — destination buffer.
pub fn decode_block(
    data: &[u8],
    block_type: u8,
    block_size: usize,
    history: &[u8],
    output: &mut Vec<u8>,
) -> Result<()> {
    match block_type {
        0 => {
            // Raw block: data is already uncompressed
            output.extend_from_slice(data);
            Ok(())
        }
        1 => {
            // RLE block: one byte repeated `block_size` times
            if data.is_empty() {
                return Err(ZstdError::UnexpectedEof);
            }
            output.extend(std::iter::repeat(data[0]).take(block_size));
            Ok(())
        }
        2 => {
            // Compressed block
            decode_compressed_block(data, history, output)
        }
        3 => Err(ZstdError::InvalidBlockType(3)),
        _ => Err(ZstdError::InvalidBlockType(block_type)),
    }
}

fn decode_compressed_block(data: &[u8], history: &[u8], output: &mut Vec<u8>) -> Result<()> {
    // Decode literals section
    let lit_section = decode_literals(data)?;
    let literals = lit_section.literals;
    let seq_start = lit_section.bytes_used;

    // Decode sequences section
    let seq_data = &data[seq_start..];
    let (sequences, _) = decode_sequences(seq_data)?;

    // Execute sequences
    execute_sequences(&sequences, &literals, history, output)
}
