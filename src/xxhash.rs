//! XXHash-32 checksum implementation, used by zstd frame checksums.
//!
//! This is a minimal implementation of the xxHash-32 algorithm as specified
//! at <https://github.com/Cyan4973/xxHash/blob/dev/doc/xxhash_spec.md>.

const PRIME1: u32 = 0x9E3779B1;
const PRIME2: u32 = 0x85EBCA77;
const PRIME3: u32 = 0xC2B2AE3D;
const PRIME4: u32 = 0x27D4EB2F;
const PRIME5: u32 = 0x165667B1;

/// Compute the XXHash-32 of `data` with the given `seed`.
pub fn xxhash32(data: &[u8], seed: u32) -> u32 {
    let len = data.len();
    let mut input = data;
    let mut h32: u32;

    if len >= 16 {
        let mut v1 = seed.wrapping_add(PRIME1).wrapping_add(PRIME2);
        let mut v2 = seed.wrapping_add(PRIME2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(PRIME1);

        while input.len() >= 16 {
            v1 = round(v1, read_u32_le(input, 0));
            v2 = round(v2, read_u32_le(input, 4));
            v3 = round(v3, read_u32_le(input, 8));
            v4 = round(v4, read_u32_le(input, 12));
            input = &input[16..];
        }

        h32 = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
    } else {
        h32 = seed.wrapping_add(PRIME5);
    }

    h32 = h32.wrapping_add(len as u32);

    // Consume remaining 4-byte chunks
    while input.len() >= 4 {
        h32 = h32
            .wrapping_add(read_u32_le(input, 0).wrapping_mul(PRIME3))
            .rotate_left(17)
            .wrapping_mul(PRIME4);
        input = &input[4..];
    }

    // Consume remaining bytes
    for &byte in input {
        h32 = h32
            .wrapping_add((byte as u32).wrapping_mul(PRIME5))
            .rotate_left(11)
            .wrapping_mul(PRIME1);
    }

    // Avalanche
    h32 ^= h32 >> 15;
    h32 = h32.wrapping_mul(PRIME2);
    h32 ^= h32 >> 13;
    h32 = h32.wrapping_mul(PRIME3);
    h32 ^= h32 >> 16;

    h32
}

#[inline(always)]
fn round(acc: u32, input: u32) -> u32 {
    acc.wrapping_add(input.wrapping_mul(PRIME2))
        .rotate_left(13)
        .wrapping_mul(PRIME1)
}

#[inline(always)]
fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test vectors from the xxHash specification.
    #[test]
    fn test_empty_seed0() {
        assert_eq!(xxhash32(b"", 0), 0x02CC5D05);
    }

    #[test]
    fn test_empty_seed1() {
        assert_eq!(xxhash32(b"", 1), 0x0B2CB792);
    }

    #[test]
    fn test_self_consistency() {
        // The same input always produces the same hash.
        let data = b"hello world, this is a consistency check";
        assert_eq!(xxhash32(data, 0), xxhash32(data, 0));
        // Different seeds produce different hashes.
        assert_ne!(xxhash32(data, 0), xxhash32(data, 1));
    }

    #[test]
    fn test_different_inputs() {
        // Different inputs should (almost certainly) produce different hashes.
        assert_ne!(xxhash32(b"abc", 0), xxhash32(b"abd", 0));
    }

    #[test]
    fn test_long_input() {
        // Exercises the 16-byte-at-a-time main loop.
        let data: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
        let h = xxhash32(&data, 0);
        assert_eq!(h, xxhash32(&data, 0)); // deterministic
    }
}
