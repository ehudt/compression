//! XXHash-64 checksum, used for zstd frame content checksums.
//!
//! zstd stores the lower 32 bits of XXHash-64(data, seed=0) as the
//! 4-byte content checksum.  Despite what the format spec text says
//! ("xxhash32"), the reference implementation uses XXH64.
//!
//! References:
//! - <https://github.com/Cyan4973/xxHash/blob/dev/doc/xxhash_spec.md>
//! - zstd source: `ZSTD_XXH64` in `lib/common/xxhash.h`

const PRIME64_1: u64 = 0x9E3779B185EBCA87;
const PRIME64_2: u64 = 0xC2B2AE3D27D4EB4F;
const PRIME64_3: u64 = 0x165667B19E3779F9;
const PRIME64_4: u64 = 0x85EBCA77C2B2AE63;
const PRIME64_5: u64 = 0x27D4EB2F165667C5;

/// Compute the lower 32 bits of XXHash-64 of `data` with the given `seed`.
///
/// This is the value zstd stores as its 4-byte content checksum.
pub fn xxhash32(data: &[u8], seed: u64) -> u32 {
    xxhash64(data, seed) as u32
}

fn xxhash64(data: &[u8], seed: u64) -> u64 {
    let len = data.len();
    let mut input = data;
    let mut h64: u64;

    if len >= 32 {
        let mut v1 = seed.wrapping_add(PRIME64_1).wrapping_add(PRIME64_2);
        let mut v2 = seed.wrapping_add(PRIME64_2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(PRIME64_1);

        while input.len() >= 32 {
            v1 = round64(v1, read_u64_le(input, 0));
            v2 = round64(v2, read_u64_le(input, 8));
            v3 = round64(v3, read_u64_le(input, 16));
            v4 = round64(v4, read_u64_le(input, 24));
            input = &input[32..];
        }

        h64 = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));

        h64 = merge_round(h64, v1);
        h64 = merge_round(h64, v2);
        h64 = merge_round(h64, v3);
        h64 = merge_round(h64, v4);
    } else {
        h64 = seed.wrapping_add(PRIME64_5);
    }

    h64 = h64.wrapping_add(len as u64);

    // Consume remaining 8-byte chunks
    while input.len() >= 8 {
        let k = round64(0, read_u64_le(input, 0));
        h64 ^= k;
        h64 = h64
            .rotate_left(27)
            .wrapping_mul(PRIME64_1)
            .wrapping_add(PRIME64_4);
        input = &input[8..];
    }

    // Consume remaining 4-byte chunk
    if input.len() >= 4 {
        let k = read_u32_le(input, 0) as u64;
        h64 ^= k.wrapping_mul(PRIME64_1);
        h64 = h64
            .rotate_left(23)
            .wrapping_mul(PRIME64_2)
            .wrapping_add(PRIME64_3);
        input = &input[4..];
    }

    // Consume remaining bytes
    for &byte in input {
        let k = (byte as u64).wrapping_mul(PRIME64_5);
        h64 ^= k;
        h64 = h64.rotate_left(11).wrapping_mul(PRIME64_1);
    }

    // Avalanche
    h64 ^= h64 >> 33;
    h64 = h64.wrapping_mul(PRIME64_2);
    h64 ^= h64 >> 29;
    h64 = h64.wrapping_mul(PRIME64_3);
    h64 ^= h64 >> 32;

    h64
}

#[inline(always)]
fn round64(acc: u64, input: u64) -> u64 {
    acc.wrapping_add(input.wrapping_mul(PRIME64_2))
        .rotate_left(31)
        .wrapping_mul(PRIME64_1)
}

#[inline(always)]
fn merge_round(acc: u64, val: u64) -> u64 {
    let val = round64(0, val);
    (acc ^ val).wrapping_mul(PRIME64_1).wrapping_add(PRIME64_4)
}

#[inline(always)]
fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

#[inline(always)]
fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known test vectors derived from zstd's own checksum output.
    #[test]
    fn test_empty_seed0() {
        // zstd --content-size on empty input produces this checksum
        assert_eq!(xxhash32(b"", 0), 0x51D8E999);
    }

    #[test]
    fn test_single_byte_z() {
        // Verified against: printf 'Z' | zstd -c | tail -c4 | od -An -tx4
        assert_eq!(xxhash32(b"Z", 0), 0x5F35570B);
    }

    #[test]
    fn test_self_consistency() {
        let data = b"hello world, this is a consistency check";
        assert_eq!(xxhash32(data, 0), xxhash32(data, 0));
        assert_ne!(xxhash32(data, 0), xxhash32(data, 1));
    }

    #[test]
    fn test_different_inputs() {
        assert_ne!(xxhash32(b"abc", 0), xxhash32(b"abd", 0));
    }

    #[test]
    fn test_long_input() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
        let h = xxhash32(&data, 0);
        assert_eq!(h, xxhash32(&data, 0));
    }
}
