//! Command-line interface for zstd_rs.
//!
//! Usage:
//!   zstd_rs [--profile-cpu <output.svg>] [--profile-repeat <count>] compress [LEVEL] <input> <output>
//!   zstd_rs [--profile-cpu <output.svg>] [--profile-repeat <count>] decompress <input> <output>

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    let program = args.first().map(String::as_str).unwrap_or("zstd_rs");
    let parsed = parse_args(&args).unwrap_or_else(|message| {
        eprintln!("{message}");
        print_usage_and_exit(program);
    });

    let _profile = parsed
        .profile_output
        .map(zstd_rs::profiling::ProfileSession::from_output_path)
        .transpose()
        .unwrap_or_else(|message| {
            eprintln!("{message}");
            process::exit(1);
        });

    match parsed.command.as_str() {
        "compress" | "c" => {
            let (level, in_path, out_path) = if parsed.positionals.len() == 3 {
                let l: i32 = parsed.positionals[0].parse().unwrap_or_else(|_| {
                    eprintln!("Invalid level: {}", parsed.positionals[0]);
                    process::exit(1);
                });
                (l, &parsed.positionals[1], &parsed.positionals[2])
            } else if parsed.positionals.len() == 2 {
                (3, &parsed.positionals[0], &parsed.positionals[1])
            } else {
                print_usage_and_exit(program);
            };

            let input = fs::read(in_path).unwrap_or_else(|e| {
                eprintln!("Cannot read {in_path}: {e}");
                process::exit(1);
            });

            let compressed =
                repeat_command(parsed.profile_repeat, || zstd_rs::compress(&input, level))
                    .unwrap_or_else(|e| {
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
            if parsed.positionals.len() != 2 {
                print_usage_and_exit(program);
            }

            let in_path = &parsed.positionals[0];
            let out_path = &parsed.positionals[1];

            let input = fs::read(in_path).unwrap_or_else(|e| {
                eprintln!("Cannot read {in_path}: {e}");
                process::exit(1);
            });

            let decompressed =
                repeat_command(parsed.profile_repeat, || zstd_rs::decompress(&input))
                    .unwrap_or_else(|e| {
                        eprintln!("Decompression error: {e}");
                        process::exit(1);
                    });

            fs::write(out_path, &decompressed).unwrap_or_else(|e| {
                eprintln!("Cannot write {out_path}: {e}");
                process::exit(1);
            });

            println!(
                "Decompressed {} → {} bytes",
                input.len(),
                decompressed.len()
            );
        }

        cmd => {
            eprintln!("Unknown command: {cmd}. Use 'compress' or 'decompress'.");
            process::exit(1);
        }
    }
}

struct ParsedArgs {
    profile_output: Option<String>,
    profile_repeat: usize,
    command: String,
    positionals: Vec<String>,
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut index = 1;
    let mut profile_output = None;
    let mut profile_repeat = 1usize;

    while index < args.len() {
        match args[index].as_str() {
            "--profile-cpu" => {
                let Some(path) = args.get(index + 1) else {
                    return Err("--profile-cpu requires an output path".into());
                };
                profile_output = Some(path.clone());
                index += 2;
            }
            "--profile-repeat" => {
                let Some(count) = args.get(index + 1) else {
                    return Err("--profile-repeat requires an iteration count".into());
                };
                profile_repeat = count
                    .parse()
                    .ok()
                    .filter(|count| *count > 0)
                    .ok_or_else(|| "--profile-repeat must be a positive integer".to_string())?;
                index += 2;
            }
            "--help" | "-h" => {
                print_usage_and_exit(args.first().map(String::as_str).unwrap_or("zstd_rs"));
            }
            flag if flag.starts_with('-') => {
                return Err(format!("Unknown flag: {flag}"));
            }
            _ => break,
        }
    }

    let Some(command) = args.get(index) else {
        return Err("missing command".into());
    };

    Ok(ParsedArgs {
        profile_output,
        profile_repeat,
        command: command.clone(),
        positionals: args[index + 1..].to_vec(),
    })
}

fn repeat_command<T, E, F>(repeat: usize, mut run: F) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
{
    let mut result = run()?;
    for _ in 1..repeat {
        result = run()?;
    }
    Ok(result)
}

fn print_usage_and_exit(program: &str) -> ! {
    eprintln!(
        "Usage:\n  {program} [--profile-cpu <output.svg>] [--profile-repeat <count>] compress [level] <input> <output>\n  {program} [--profile-cpu <output.svg>] [--profile-repeat <count>] decompress <input> <output>"
    );
    process::exit(1);
}
