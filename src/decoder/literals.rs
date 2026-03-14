//! Decoding of the literals section within a compressed block.

use crate::error::{Result, ZstdError};
use crate::huffman::{read_huffman_header, HuffmanTable};

/// The result of decoding a literals section.
pub struct LiteralsSection {
    pub literals: Vec<u8>,
    /// Bytes consumed from the input.
    pub bytes_used: usize,
}

/// Decode the literals section of a compressed block.
///
/// Returns the decoded literal bytes and the number of input bytes consumed.
pub fn decode_literals(data: &[u8]) -> Result<LiteralsSection> {
    if data.is_empty() {
        return Err(ZstdError::UnexpectedEof);
    }

    let first_byte = data[0];
    let literals_type = first_byte & 0x3;
    let size_format = (first_byte >> 2) & 0x3;

    match literals_type {
        // Raw literals
        0 => {
            let (regen_size, header_size) = decode_size_raw(data, size_format)?;
            let end = header_size + regen_size;
            if data.len() < end {
                return Err(ZstdError::UnexpectedEof);
            }
            Ok(LiteralsSection {
                literals: data[header_size..end].to_vec(),
                bytes_used: end,
            })
        }
        // RLE literals
        1 => {
            let (regen_size, header_size) = decode_size_raw(data, size_format)?;
            if data.len() < header_size + 1 {
                return Err(ZstdError::UnexpectedEof);
            }
            let byte = data[header_size];
            Ok(LiteralsSection {
                literals: vec![byte; regen_size],
                bytes_used: header_size + 1,
            })
        }
        // Huffman-compressed literals (2 = single stream, 3 = 4 streams)
        2 | 3 => {
            let four_streams = literals_type == 3;
            let (regen_size, comp_size, header_size) =
                decode_size_compressed(data, size_format)?;

            let payload = &data[header_size..];
            if payload.len() < comp_size {
                return Err(ZstdError::UnexpectedEof);
            }
            let compressed = &payload[..comp_size];

            // Read huffman table
            let (huff_table, huff_header_size) = read_huffman_header(compressed)?;
            let bitstream_data = &compressed[huff_header_size..];

            let literals = if four_streams {
                decode_four_streams(bitstream_data, &huff_table, regen_size)?
            } else {
                // Single stream
                let total_bits = bitstream_data.len() * 8;
                huff_table.decode(bitstream_data, total_bits, regen_size)?
            };

            Ok(LiteralsSection {
                literals,
                bytes_used: header_size + comp_size,
            })
        }
        _ => Err(ZstdError::CorruptData("unknown literals type")),
    }
}

/// Decode sizes for raw/RLE literals. Returns `(regen_size, header_size)`.
fn decode_size_raw(data: &[u8], size_format: u8) -> Result<(usize, usize)> {
    match size_format {
        0 | 2 => {
            // size_format bit 0 used as size MSB when == 0
            let size = (data[0] >> 3) as usize;
            Ok((size, 1))
        }
        1 => {
            // 12-bit size: bits[7:4] of byte0, then byte1
            if data.len() < 2 {
                return Err(ZstdError::UnexpectedEof);
            }
            let size = ((data[0] as usize >> 4) | ((data[1] as usize) << 4)) & 0xFFF;
            Ok((size, 2))
        }
        3 => {
            // 20-bit size: bits[7:4] of byte0, then byte1, then byte2
            if data.len() < 3 {
                return Err(ZstdError::UnexpectedEof);
            }
            let size = ((data[0] as usize >> 4)
                | ((data[1] as usize) << 4)
                | ((data[2] as usize) << 12))
                & 0xFFFFF;
            Ok((size, 3))
        }
        _ => unreachable!(),
    }
}

/// Decode sizes for compressed literals. Returns `(regen_size, comp_size, header_size)`.
fn decode_size_compressed(data: &[u8], size_format: u8) -> Result<(usize, usize, usize)> {
    match size_format {
        0 | 1 => {
            if data.len() < 3 {
                return Err(ZstdError::UnexpectedEof);
            }
            let b0 = data[0] as usize;
            let b1 = data[1] as usize;
            let b2 = data[2] as usize;
            let regen = (b0 >> 4) | ((b1 & 0x3F) << 4);
            let comp = (b1 >> 6) | (b2 << 2);
            Ok((regen, comp, 3))
        }
        2 => {
            if data.len() < 4 {
                return Err(ZstdError::UnexpectedEof);
            }
            let b0 = data[0] as usize;
            let b1 = data[1] as usize;
            let b2 = data[2] as usize;
            let b3 = data[3] as usize;
            let regen = (b0 >> 4) | (b1 << 4) | ((b2 & 0x3) << 12);
            let comp = (b2 >> 2) | (b3 << 6);
            Ok((regen, comp, 4))
        }
        3 => {
            if data.len() < 5 {
                return Err(ZstdError::UnexpectedEof);
            }
            let b0 = data[0] as usize;
            let b1 = data[1] as usize;
            let b2 = data[2] as usize;
            let b3 = data[3] as usize;
            let b4 = data[4] as usize;
            let regen = (b0 >> 4) | (b1 << 4) | ((b2 & 0x3F) << 12);
            let comp = (b2 >> 6) | (b3 << 2) | (b4 << 10);
            Ok((regen, comp, 5))
        }
        _ => unreachable!(),
    }
}

/// Decode 4 interleaved Huffman streams.
fn decode_four_streams(
    data: &[u8],
    table: &HuffmanTable,
    total_regen: usize,
) -> Result<Vec<u8>> {
    if data.len() < 6 {
        return Err(ZstdError::UnexpectedEof);
    }
    // First 6 bytes = 3 x 2-byte stream sizes for streams 1-3.
    let s1_size = u16::from_le_bytes([data[0], data[1]]) as usize;
    let s2_size = u16::from_le_bytes([data[2], data[3]]) as usize;
    let s3_size = u16::from_le_bytes([data[4], data[5]]) as usize;
    let s4_size = data.len() - 6 - s1_size - s2_size - s3_size;

    let s1 = &data[6..6 + s1_size];
    let s2 = &data[6 + s1_size..6 + s1_size + s2_size];
    let s3 = &data[6 + s1_size + s2_size..6 + s1_size + s2_size + s3_size];
    let s4 = &data[6 + s1_size + s2_size + s3_size..];

    if s4.len() != s4_size {
        return Err(ZstdError::CorruptData("stream sizes inconsistent"));
    }

    // Each stream decodes total_regen/4 symbols (last stream may get extras).
    let per_stream = total_regen / 4;
    let mut out = Vec::with_capacity(total_regen);

    for (stream, count) in [
        (s1, per_stream),
        (s2, per_stream),
        (s3, per_stream),
        (s4, total_regen - 3 * per_stream),
    ] {
        let bits = stream.len() * 8;
        let mut decoded = table.decode(stream, bits, count)?;
        out.append(&mut decoded);
    }

    Ok(out)
}
