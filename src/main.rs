//! Command-line interface for zstd_rs.
//!
//! Usage:
//!   zstd_rs [--profile-cpu <output.svg>] [--profile-repeat <count>] [--profile-min-ms <ms>] [--profile-hz <hz>] compress [LEVEL] <input> <output>
//!   zstd_rs [--profile-cpu <output.svg>] [--profile-repeat <count>] [--profile-min-ms <ms>] [--profile-hz <hz>] decompress <input> <output>

use std::env;
use std::fs;
use std::process;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = env::args().collect();
    let program = args.first().map(String::as_str).unwrap_or("zstd_rs");
    let parsed = parse_args(&args).unwrap_or_else(|message| {
        eprintln!("{message}");
        print_usage_and_exit(program);
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

            let compressed = run_command(&parsed, || {
                zstd_rs::compress(&input, level).map_err(|e| e.to_string())
            })
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

            let decompressed = run_command(&parsed, || {
                zstd_rs::decompress(&input).map_err(|e| e.to_string())
            })
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

#[derive(Debug)]
struct ParsedArgs {
    profile_output: Option<String>,
    profile_repeat: usize,
    profile_min_duration_ms: Option<u64>,
    profile_frequency_hz: i32,
    command: String,
    positionals: Vec<String>,
}

struct ProfileConfig {
    output_path: String,
    repeat: usize,
    min_duration: Duration,
    frequency_hz: i32,
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut index = 1;
    let mut profile_output = None;
    let mut profile_repeat = 1usize;
    let mut profile_min_duration_ms = None;
    let mut profile_frequency_hz = zstd_rs::profiling::DEFAULT_PROFILE_FREQUENCY_HZ;
    let mut profile_frequency_explicit = false;

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
            "--profile-min-ms" => {
                let Some(ms) = args.get(index + 1) else {
                    return Err("--profile-min-ms requires a duration in milliseconds".into());
                };
                profile_min_duration_ms =
                    Some(ms.parse().ok().filter(|ms| *ms > 0).ok_or_else(|| {
                        "--profile-min-ms must be a positive integer".to_string()
                    })?);
                index += 2;
            }
            "--profile-hz" => {
                let Some(hz) = args.get(index + 1) else {
                    return Err("--profile-hz requires a sampling frequency".into());
                };
                profile_frequency_explicit = true;
                profile_frequency_hz = hz
                    .parse()
                    .ok()
                    .filter(|hz| *hz > 0)
                    .ok_or_else(|| "--profile-hz must be a positive integer".to_string())?;
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

    if profile_min_duration_ms.is_some() && profile_output.is_none() {
        return Err("--profile-min-ms requires --profile-cpu".into());
    }

    if profile_frequency_explicit && profile_output.is_none() {
        return Err("--profile-hz requires --profile-cpu".into());
    }

    Ok(ParsedArgs {
        profile_output,
        profile_repeat,
        profile_min_duration_ms,
        profile_frequency_hz,
        command: command.clone(),
        positionals: args[index + 1..].to_vec(),
    })
}

fn run_command<T, F>(parsed: &ParsedArgs, mut run: F) -> Result<T, String>
where
    F: FnMut() -> Result<T, String>,
{
    if let Some(profile) = parsed.profile_config() {
        return run_profiled_command(&profile, &mut run);
    }

    repeat_command(parsed.profile_repeat, run)
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

fn run_profiled_command<T, F>(profile: &ProfileConfig, run: &mut F) -> Result<T, String>
where
    F: FnMut() -> Result<T, String>,
{
    let _warmup = run()?;
    let _session = zstd_rs::profiling::ProfileSession::from_output_path(
        &profile.output_path,
        profile.frequency_hz,
    )?;

    let started = Instant::now();
    let mut measured_iterations = 0usize;
    let mut result = run()?;
    measured_iterations += 1;

    while should_continue_profiled_runs(
        measured_iterations,
        profile.repeat,
        profile.min_duration,
        started.elapsed(),
    ) {
        result = run()?;
        measured_iterations += 1;
    }

    eprintln!(
        "profiled {measured_iterations} steady-state iteration(s) after 1 warmup iteration over {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );

    Ok(result)
}

fn should_continue_profiled_runs(
    measured_iterations: usize,
    repeat: usize,
    min_duration: Duration,
    elapsed: Duration,
) -> bool {
    measured_iterations < repeat || elapsed < min_duration
}

impl ParsedArgs {
    fn profile_config(&self) -> Option<ProfileConfig> {
        self.profile_output
            .as_ref()
            .map(|output_path| ProfileConfig {
                output_path: output_path.clone(),
                repeat: self.profile_repeat,
                min_duration: self
                    .profile_min_duration_ms
                    .map(Duration::from_millis)
                    .unwrap_or(Duration::ZERO),
                frequency_hz: self.profile_frequency_hz,
            })
    }
}

fn print_usage_and_exit(program: &str) -> ! {
    eprintln!(
        "Usage:\n  {program} [--profile-cpu <output.svg>] [--profile-repeat <count>] [--profile-min-ms <ms>] [--profile-hz <hz>] compress [level] <input> <output>\n  {program} [--profile-cpu <output.svg>] [--profile-repeat <count>] [--profile-min-ms <ms>] [--profile-hz <hz>] decompress <input> <output>"
    );
    process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::{parse_args, should_continue_profiled_runs};
    use std::time::Duration;

    #[test]
    fn parse_args_accepts_profile_min_ms() {
        let args = vec![
            "zstd_rs".to_string(),
            "--profile-cpu".to_string(),
            "out.svg".to_string(),
            "--profile-repeat".to_string(),
            "200".to_string(),
            "--profile-min-ms".to_string(),
            "250".to_string(),
            "--profile-hz".to_string(),
            "1500".to_string(),
            "compress".to_string(),
            "3".to_string(),
            "input".to_string(),
            "output".to_string(),
        ];

        let parsed = parse_args(&args).expect("args should parse");
        assert_eq!(parsed.profile_output.as_deref(), Some("out.svg"));
        assert_eq!(parsed.profile_repeat, 200);
        assert_eq!(parsed.profile_min_duration_ms, Some(250));
        assert_eq!(parsed.profile_frequency_hz, 1500);
    }

    #[test]
    fn parse_args_rejects_profile_min_ms_without_profile_cpu() {
        let args = vec![
            "zstd_rs".to_string(),
            "--profile-min-ms".to_string(),
            "250".to_string(),
            "compress".to_string(),
            "input".to_string(),
            "output".to_string(),
        ];

        let error = parse_args(&args).expect_err("args should fail");
        assert_eq!(error, "--profile-min-ms requires --profile-cpu");
    }

    #[test]
    fn profile_loop_respects_repeat_and_duration_targets() {
        assert!(should_continue_profiled_runs(
            1,
            3,
            Duration::from_millis(0),
            Duration::from_millis(100)
        ));
        assert!(should_continue_profiled_runs(
            3,
            3,
            Duration::from_millis(50),
            Duration::from_millis(25)
        ));
        assert!(!should_continue_profiled_runs(
            3,
            3,
            Duration::from_millis(50),
            Duration::from_millis(50)
        ));
    }
}
