//! Basic usage example for zstd_rs.

fn main() {
    // Compress some data
    let original = b"Hello, world! This is a demonstration of zstd_rs compression. \
                     The quick brown fox jumps over the lazy dog. \
                     The quick brown fox jumps over the lazy dog again.";

    println!("Original:   {} bytes", original.len());

    for level in [1, 3, 6, 9] {
        let compressed = zstd_rs::compress(original, level)
            .expect("compression failed");
        let decompressed = zstd_rs::decompress(&compressed)
            .expect("decompression failed");

        assert_eq!(&decompressed, original, "round-trip mismatch!");
        println!(
            "Level {:2}: {:4} bytes  ({:.1}%)",
            level,
            compressed.len(),
            100.0 * compressed.len() as f64 / original.len() as f64
        );
    }

    // Highly compressible data
    let repetitive = b"aaaa".repeat(10_000);
    let compressed = zstd_rs::compress(&repetitive, 3).unwrap();
    println!(
        "\n40 000 bytes of 'a': {} bytes compressed ({:.2}% of original)",
        compressed.len(),
        100.0 * compressed.len() as f64 / repetitive.len() as f64
    );
}
