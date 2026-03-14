//! Command-line interface for zstd_rs.
//!
//! Usage:
//!   zstd_rs compress [LEVEL] <input> <output>
//!   zstd_rs decompress <input> <output>

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "Usage:\n  {} compress [level] <input> <output>\n  {} decompress <input> <output>",
            args[0], args[0]
        );
        process::exit(1);
    }

    match args[1].as_str() {
        "compress" | "c" => {
            let (level, in_path, out_path) = if args.len() == 5 {
                let l: i32 = args[2].parse().unwrap_or_else(|_| {
                    eprintln!("Invalid level: {}", args[2]);
                    process::exit(1);
                });
                (l, &args[3], &args[4])
            } else {
                (3, &args[2], &args[3])
            };

            let input = fs::read(in_path).unwrap_or_else(|e| {
                eprintln!("Cannot read {in_path}: {e}");
                process::exit(1);
            });

            let compressed = zstd_rs::compress(&input, level).unwrap_or_else(|e| {
                eprintln!("Compression error: {e}");
                process::exit(1);
            });

            fs::write(out_path, &compressed).unwrap_or_else(|e| {
                eprintln!("Cannot write {out_path}: {e}");
                process::exit(1);
            });

            println!(
                "Compressed {} → {} bytes ({:.1}%)",
                input.len(),
                compressed.len(),
                100.0 * compressed.len() as f64 / input.len().max(1) as f64
            );
        }

        "decompress" | "d" => {
            let in_path = &args[2];
            let out_path = &args[3];

            let input = fs::read(in_path).unwrap_or_else(|e| {
                eprintln!("Cannot read {in_path}: {e}");
                process::exit(1);
            });

            let decompressed = zstd_rs::decompress(&input).unwrap_or_else(|e| {
                eprintln!("Decompression error: {e}");
                process::exit(1);
            });

            fs::write(out_path, &decompressed).unwrap_or_else(|e| {
                eprintln!("Cannot write {out_path}: {e}");
                process::exit(1);
            });

            println!("Decompressed {} → {} bytes", input.len(), decompressed.len());
        }

        cmd => {
            eprintln!("Unknown command: {cmd}. Use 'compress' or 'decompress'.");
            process::exit(1);
        }
    }
}
