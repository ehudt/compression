use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{Duration, Instant};

use zstd_rs::{compress, decompress};

const DEFAULT_CORPUS_DIR: &str = "~/silesia";
const DEFAULT_OUTPUT_DIR: &str = "docs/benchmarks";
const DEFAULT_LEVELS: &[i32] = &[1, 3, 9, 19];
const DEFAULT_MIN_BENCH_MS: u64 = 750;
const DEFAULT_WARMUP_ITERATIONS: usize = 1;
const DEFAULT_MEASURE_ITERATIONS: usize = 1;
const SILESIA_ARCHIVE_URL: &str = "https://sun.aei.polsl.pl/~sdeor/corpus/silesia.zip";

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let config = Config::parse(env::args().skip(1))?;

    if config.download_if_missing && corpus_cache_needs_refresh(&config.corpus_dir)? {
        download_silesia_corpus(&config.corpus_dir)?;
    }

    let corpus = load_corpus(&config.corpus_dir)?;
    if corpus.files.is_empty() {
        return Err(format!(
            "no corpus files found under {}",
            config.corpus_dir.display()
        ));
    }

    let mut results = Vec::new();
    let official_version = detect_zstd_version().unwrap_or_else(|_| "zstd".to_string());
    let ours_name = format!("zstd_rs {}", env!("CARGO_PKG_VERSION"));
    let implementations_per_level = usize::from(config.implementation.includes_ours())
        + usize::from(config.implementation.includes_official());
    let total_runs = config.levels.len() * implementations_per_level;
    let mut completed_runs = 0usize;

    for &level in &config.levels {
        if config.implementation.includes_ours() {
            print_progress(
                completed_runs,
                total_runs,
                &format!("running {ours_name} level {level}"),
            )?;
            let result = benchmark_ours(level, &ours_name, &corpus, &config);
            completed_runs += 1;
            print_result_summary(completed_runs, total_runs, &result);
            results.push(result);
        }
        if config.implementation.includes_official() {
            print_progress(
                completed_runs,
                total_runs,
                &format!("running {official_version} level {level}"),
            )?;
            let result = benchmark_official(
                level,
                &official_version,
                &corpus,
                &config,
            );
            completed_runs += 1;
            print_result_summary(completed_runs, total_runs, &result);
            results.push(result);
        }
    }

    results.sort_by(|left, right| {
        left.level
            .cmp(&right.level)
            .then(left.impl_kind.sort_key().cmp(&right.impl_kind.sort_key()))
    });

    fs::create_dir_all(&config.output_dir).map_err(|err| {
        format!(
            "failed to create output directory {}: {err}",
            config.output_dir.display()
        )
    })?;

    let markdown = render_markdown_table(&results, &corpus);
    let summary_path = config.output_dir.join("silesia-comparison.md");
    fs::write(&summary_path, markdown).map_err(|err| {
        format!(
            "failed to write markdown summary {}: {err}",
            summary_path.display()
        )
    })?;

    let json = render_json(&results, &corpus);
    let json_path = config.output_dir.join("silesia-comparison.json");
    fs::write(&json_path, json).map_err(|err| {
        format!(
            "failed to write JSON summary {}: {err}",
            json_path.display()
        )
    })?;

    let svg = render_svg(&results, &corpus);
    let svg_path = config.output_dir.join("silesia-comparison.svg");
    fs::write(&svg_path, svg)
        .map_err(|err| format!("failed to write SVG chart {}: {err}", svg_path.display()))?;

    println!(
        "Benchmarked {} files / {:.1} MiB from {}",
        corpus.files.len(),
        corpus.total_bytes as f64 / (1024.0 * 1024.0),
        config.corpus_dir.display()
    );
    println!();
    println!("{}", render_terminal_table(&results));
    println!();
    println!("Markdown: {}", summary_path.display());
    println!("JSON: {}", json_path.display());
    println!("SVG: {}", svg_path.display());

    Ok(())
}

#[derive(Debug)]
struct Config {
    corpus_dir: PathBuf,
    output_dir: PathBuf,
    levels: Vec<i32>,
    min_bench_time: Duration,
    warmup_iterations: usize,
    measure_iterations: usize,
    download_if_missing: bool,
    implementation: ImplementationSelection,
}

impl Config {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut corpus_dir = expand_tilde(DEFAULT_CORPUS_DIR);
        let mut output_dir = PathBuf::from(DEFAULT_OUTPUT_DIR);
        let mut levels = DEFAULT_LEVELS.to_vec();
        let mut min_bench_ms = DEFAULT_MIN_BENCH_MS;
        let mut warmup_iterations = DEFAULT_WARMUP_ITERATIONS;
        let mut measure_iterations = DEFAULT_MEASURE_ITERATIONS;
        let mut download_if_missing = false;
        let mut implementation = ImplementationSelection::Both;

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--corpus-dir" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "--corpus-dir requires a path".to_string())?;
                    corpus_dir = expand_tilde(&value);
                }
                "--output-dir" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "--output-dir requires a path".to_string())?;
                    output_dir = PathBuf::from(value);
                }
                "--levels" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "--levels requires a comma-separated list".to_string())?;
                    levels = parse_levels(&value)?;
                }
                "--min-bench-ms" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "--min-bench-ms requires a number".to_string())?;
                    min_bench_ms = value
                        .parse()
                        .map_err(|_| format!("invalid --min-bench-ms value: {value}"))?;
                }
                "--warmup-iterations" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "--warmup-iterations requires a number".to_string())?;
                    warmup_iterations = parse_positive_usize("--warmup-iterations", &value)?;
                }
                "--measure-iterations" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "--measure-iterations requires a number".to_string())?;
                    measure_iterations = parse_positive_usize("--measure-iterations", &value)?;
                }
                "--download" => {
                    download_if_missing = true;
                }
                "--implementation" => {
                    let value = args.next().ok_or_else(|| {
                        "--implementation requires one of: ours, official, both".to_string()
                    })?;
                    implementation = ImplementationSelection::parse(&value)?;
                }
                "--help" | "-h" => {
                    print_usage();
                    process::exit(0);
                }
                other => {
                    return Err(format!("unknown argument: {other}"));
                }
            }
        }

        if levels.is_empty() {
            return Err("at least one benchmark level is required".to_string());
        }

        Ok(Self {
            corpus_dir,
            output_dir,
            levels,
            min_bench_time: Duration::from_millis(min_bench_ms),
            warmup_iterations,
            measure_iterations,
            download_if_missing,
            implementation,
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum ImplementationSelection {
    Ours,
    Official,
    Both,
}

impl ImplementationSelection {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ours" => Ok(Self::Ours),
            "official" => Ok(Self::Official),
            "both" => Ok(Self::Both),
            _ => Err(format!(
                "invalid --implementation value: {value} (expected ours, official, or both)"
            )),
        }
    }

    fn includes_ours(self) -> bool {
        matches!(self, Self::Ours | Self::Both)
    }

    fn includes_official(self) -> bool {
        matches!(self, Self::Official | Self::Both)
    }
}

fn print_usage() {
    eprintln!(
        "Usage: cargo run --release --example silesia_bench -- [options]\n\
         \n\
         Options:\n\
           --corpus-dir <path>          Silesia corpus directory (default: {DEFAULT_CORPUS_DIR})\n\
           --output-dir <path>          Output directory for table/json/svg (default: {DEFAULT_OUTPUT_DIR})\n\
           --implementation <mode>      Which implementation(s) to benchmark: ours, official, both (default: both)\n\
           --levels <csv>               Compression levels to benchmark (default: 1,3,9,19)\n\
           --min-bench-ms <ms>          Minimum time per timed benchmark (default: {DEFAULT_MIN_BENCH_MS})\n\
           --warmup-iterations <n>      Warmup iterations before timing (default: {DEFAULT_WARMUP_ITERATIONS})\n\
           --measure-iterations <n>     Minimum timed iterations before stopping (default: {DEFAULT_MEASURE_ITERATIONS})\n\
           --download                   Download a cached Silesia corpus if missing\n"
    );
}

fn parse_positive_usize(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{flag} must be a positive integer"))
}

fn print_progress(completed_runs: usize, total_runs: usize, message: &str) -> Result<(), String> {
    println!("[{}/{}] {}", completed_runs + 1, total_runs, message);
    io::stdout()
        .flush()
        .map_err(|err| format!("failed to flush benchmark progress output: {err}"))
}

fn print_result_summary(completed_runs: usize, total_runs: usize, result: &ResultRow) {
    match &result.note {
        Some(note) => println!(
            "[{}/{}] {} failed: {}",
            completed_runs, total_runs, result.compressor_name, note
        ),
        None => println!(
            "[{}/{}] {} done: ratio {}, comp {}, decomp {}",
            completed_runs,
            total_runs,
            result.compressor_name,
            format_ratio(result.ratio),
            format_speed(result.compression_mbps),
            format_speed(result.decompression_mbps)
        ),
    }
}

fn parse_levels(value: &str) -> Result<Vec<i32>, String> {
    let mut levels = Vec::new();
    for part in value.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let level = trimmed
            .parse::<i32>()
            .map_err(|_| format!("invalid compression level: {trimmed}"))?;
        levels.push(level);
    }
    levels.sort_unstable();
    levels.dedup();
    Ok(levels)
}

#[derive(Clone)]
struct CorpusFile {
    name: String,
    bytes: Vec<u8>,
    path: PathBuf,
}

struct Corpus {
    files: Vec<CorpusFile>,
    total_bytes: usize,
}

fn load_corpus(dir: &Path) -> Result<Corpus, String> {
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(dir)
        .map_err(|err| format!("failed to read corpus directory {}: {err}", dir.display()))?;

    for entry in read_dir {
        let entry = entry.map_err(|err| format!("failed to read corpus entry: {err}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') {
            continue;
        }

        let bytes = fs::read(&path)
            .map_err(|err| format!("failed to read corpus file {}: {err}", path.display()))?;
        entries.push(CorpusFile {
            name: file_name,
            bytes,
            path,
        });
    }

    entries.sort_by(|left, right| left.name.cmp(&right.name));
    let total_bytes = entries.iter().map(|file| file.bytes.len()).sum();
    Ok(Corpus {
        files: entries,
        total_bytes,
    })
}

fn corpus_cache_needs_refresh(dir: &Path) -> Result<bool, String> {
    if !dir.exists() {
        return Ok(true);
    }

    let entries = fs::read_dir(dir).map_err(|err| {
        format!(
            "failed to inspect corpus directory {}: {err}",
            dir.display()
        )
    })?;

    let mut saw_file = false;
    for entry in entries {
        let entry = entry.map_err(|err| format!("failed to inspect corpus entry: {err}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        saw_file = true;
        if path.extension().and_then(|ext| ext.to_str()) == Some("zip") {
            return Ok(true);
        }
    }

    Ok(!saw_file)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn download_silesia_corpus(dest_dir: &Path) -> Result<(), String> {
    let parent = dest_dir.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|err| {
        format!(
            "failed to create parent directory {}: {err}",
            parent.display()
        )
    })?;

    let temp_root = env::temp_dir().join(format!("zstd-rs-silesia-{}", process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).map_err(|err| {
            format!(
                "failed to clean temporary download directory {}: {err}",
                temp_root.display()
            )
        })?;
    }
    fs::create_dir_all(&temp_root).map_err(|err| {
        format!(
            "failed to create temporary download directory {}: {err}",
            temp_root.display()
        )
    })?;

    let archive_path = temp_root.join("silesia.zip");
    run_command(
        Command::new("curl")
            .arg("-L")
            .arg("--fail")
            .arg("-o")
            .arg(&archive_path)
            .arg(SILESIA_ARCHIVE_URL),
        "download Silesia corpus",
    )?;

    let extracted_dir = temp_root.join("extracted");
    fs::create_dir_all(&extracted_dir).map_err(|err| {
        format!(
            "failed to create extraction directory {}: {err}",
            extracted_dir.display()
        )
    })?;

    run_command(
        Command::new("python3")
            .arg("-c")
            .arg(
                "import pathlib, sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])",
            )
            .arg(&archive_path)
            .arg(&extracted_dir),
        "extract Silesia corpus",
    )?;

    let data_root = find_single_child_dir(&extracted_dir)?.unwrap_or(extracted_dir.clone());

    if dest_dir.exists() {
        fs::remove_dir_all(dest_dir).map_err(|err| {
            format!(
                "failed to replace existing corpus cache {}: {err}",
                dest_dir.display()
            )
        })?;
    }
    fs::create_dir_all(dest_dir).map_err(|err| {
        format!(
            "failed to create corpus cache directory {}: {err}",
            dest_dir.display()
        )
    })?;

    for entry in fs::read_dir(&data_root).map_err(|err| {
        format!(
            "failed to read extracted corpus {}: {err}",
            data_root.display()
        )
    })? {
        let entry = entry.map_err(|err| format!("failed to inspect extracted file: {err}"))?;
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let target = dest_dir.join(name);
        fs::copy(&path, &target).map_err(|err| {
            format!(
                "failed to copy {} to {}: {err}",
                path.display(),
                target.display()
            )
        })?;
    }

    let _ = fs::remove_dir_all(&temp_root);
    Ok(())
}

fn find_single_child_dir(dir: &Path) -> Result<Option<PathBuf>, String> {
    let mut children = fs::read_dir(dir)
        .map_err(|err| {
            format!(
                "failed to read temporary directory {}: {err}",
                dir.display()
            )
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    children.sort();
    Ok(children
        .into_iter()
        .find(|path| path.file_name().and_then(|name| name.to_str()) != Some(".")))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImplementationKind {
    Ours,
    Official,
}

impl ImplementationKind {
    fn sort_key(self) -> u8 {
        match self {
            Self::Ours => 0,
            Self::Official => 1,
        }
    }

    fn color(self) -> &'static str {
        match self {
            Self::Ours => "#c75b12",
            Self::Official => "#146356",
        }
    }
}

#[derive(Clone)]
struct ResultRow {
    impl_kind: ImplementationKind,
    compressor_name: String,
    level: i32,
    ratio: Option<f64>,
    compression_mbps: Option<f64>,
    decompression_mbps: Option<f64>,
    total_input_bytes: usize,
    total_compressed_bytes: Option<usize>,
    note: Option<String>,
}

fn benchmark_ours(level: i32, name: &str, corpus: &Corpus, config: &Config) -> ResultRow {
    let mut compressed = Vec::with_capacity(corpus.files.len());
    let mut total_compressed_bytes = 0usize;

    for file in &corpus.files {
        let bytes = match compress(&file.bytes, level) {
            Ok(bytes) => bytes,
            Err(err) => {
                return failed_row(
                    ImplementationKind::Ours,
                    format!("{name} -{level}"),
                    level,
                    corpus.total_bytes,
                    format!("compression failed for {}: {err}", file.path.display()),
                );
            }
        };
        total_compressed_bytes += bytes.len();
        compressed.push(bytes);
    }

    for (file, encoded) in corpus.files.iter().zip(&compressed) {
        let decoded = match decompress(encoded) {
            Ok(decoded) => decoded,
            Err(err) => {
                return failed_row(
                    ImplementationKind::Ours,
                    format!("{name} -{level}"),
                    level,
                    corpus.total_bytes,
                    format!("round-trip failed for {}: {err}", file.path.display()),
                );
            }
        };
        if decoded != file.bytes {
            return failed_row(
                ImplementationKind::Ours,
                format!("{name} -{level}"),
                level,
                corpus.total_bytes,
                format!("round-trip mismatch for {}", file.path.display()),
            );
        }
    }

    if let Err(note) = warm_up(config.warmup_iterations, || {
        for file in &corpus.files {
            let _ = compress(&file.bytes, level).map_err(|err| {
                format!(
                    "zstd_rs compression failed for {}: {err}",
                    file.path.display()
                )
            })?;
        }
        Ok(())
    }) {
        return failed_row(
            ImplementationKind::Ours,
            format!("{name} -{level}"),
            level,
            corpus.total_bytes,
            note,
        );
    }

    let compression_mbps = match measure_throughput(
        corpus.total_bytes,
        config.min_bench_time,
        config.measure_iterations,
        || {
            for file in &corpus.files {
                let _ = compress(&file.bytes, level).map_err(|err| {
                    format!(
                        "zstd_rs compression failed for {}: {err}",
                        file.path.display()
                    )
                })?;
            }
            Ok(())
        },
    ) {
        Ok(value) => value,
        Err(note) => {
            return failed_row(
                ImplementationKind::Ours,
                format!("{name} -{level}"),
                level,
                corpus.total_bytes,
                note,
            );
        }
    };

    if let Err(note) = warm_up(config.warmup_iterations, || {
        for encoded in &compressed {
            let _ = decompress(encoded)
                .map_err(|err| format!("zstd_rs decompression failed: {err}"))?;
        }
        Ok(())
    }) {
        return failed_row(
            ImplementationKind::Ours,
            format!("{name} -{level}"),
            level,
            corpus.total_bytes,
            note,
        );
    }

    let decompression_mbps = match measure_throughput(
        corpus.total_bytes,
        config.min_bench_time,
        config.measure_iterations,
        || {
            for encoded in &compressed {
                let _ = decompress(encoded)
                    .map_err(|err| format!("zstd_rs decompression failed: {err}"))?;
            }
            Ok(())
        },
    ) {
        Ok(value) => value,
        Err(note) => {
            return failed_row(
                ImplementationKind::Ours,
                format!("{name} -{level}"),
                level,
                corpus.total_bytes,
                note,
            );
        }
    };

    ResultRow {
        impl_kind: ImplementationKind::Ours,
        compressor_name: format!("{name} -{level}"),
        level,
        ratio: Some(corpus.total_bytes as f64 / total_compressed_bytes as f64),
        compression_mbps: Some(compression_mbps),
        decompression_mbps: Some(decompression_mbps),
        total_input_bytes: corpus.total_bytes,
        total_compressed_bytes: Some(total_compressed_bytes),
        note: None,
    }
}

fn benchmark_official(level: i32, version: &str, corpus: &Corpus, config: &Config) -> ResultRow {
    let temp_root = env::temp_dir().join(format!(
        "zstd-rs-official-bench-{}-{}",
        process::id(),
        level
    ));
    if temp_root.exists() {
        if let Err(err) = fs::remove_dir_all(&temp_root) {
            return failed_row(
                ImplementationKind::Official,
                format!("{version} -{level}"),
                level,
                corpus.total_bytes,
                format!(
                    "failed to clean temporary benchmark directory {}: {err}",
                    temp_root.display()
                ),
            );
        }
    }
    if let Err(err) = fs::create_dir_all(&temp_root) {
        return failed_row(
            ImplementationKind::Official,
            format!("{version} -{level}"),
            level,
            corpus.total_bytes,
            format!(
                "failed to create temporary benchmark directory {}: {err}",
                temp_root.display()
            ),
        );
    }

    let mut compressed_paths = Vec::with_capacity(corpus.files.len());
    let mut total_compressed_bytes = 0usize;

    for file in &corpus.files {
        let output_path = temp_root.join(format!("{}.zst", file.name));
        let status = match Command::new("zstd")
            .arg("-q")
            .arg("-f")
            .arg(format!("-{level}"))
            .arg(&file.path)
            .arg("-o")
            .arg(&output_path)
            .status()
        {
            Ok(status) => status,
            Err(err) => {
                return failed_row(
                    ImplementationKind::Official,
                    format!("{version} -{level}"),
                    level,
                    corpus.total_bytes,
                    format!("failed to start zstd for {}: {err}", file.path.display()),
                );
            }
        };
        if !status.success() {
            return failed_row(
                ImplementationKind::Official,
                format!("{version} -{level}"),
                level,
                corpus.total_bytes,
                format!(
                    "zstd compression failed for {} with status {status}",
                    file.path.display()
                ),
            );
        }
        let compressed_len = match fs::metadata(&output_path) {
            Ok(meta) => meta.len() as usize,
            Err(err) => {
                return failed_row(
                    ImplementationKind::Official,
                    format!("{version} -{level}"),
                    level,
                    corpus.total_bytes,
                    format!("failed to stat {}: {err}", output_path.display()),
                );
            }
        };
        total_compressed_bytes += compressed_len;
        compressed_paths.push(output_path);
    }

    if let Err(note) = warm_up(config.warmup_iterations, || {
        for file in &corpus.files {
            run_command(
                Command::new("zstd")
                    .arg("-q")
                    .arg("-f")
                    .arg(format!("-{level}"))
                    .arg(&file.path)
                    .arg("-c")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null()),
                "run zstd compression warmup",
            )?;
        }
        Ok(())
    }) {
        return failed_row(
            ImplementationKind::Official,
            format!("{version} -{level}"),
            level,
            corpus.total_bytes,
            note,
        );
    }

    let compression_mbps = match measure_throughput(
        corpus.total_bytes,
        config.min_bench_time,
        config.measure_iterations,
        || {
            for file in &corpus.files {
                run_command(
                    Command::new("zstd")
                        .arg("-q")
                        .arg("-f")
                        .arg(format!("-{level}"))
                        .arg(&file.path)
                        .arg("-c")
                        .stdout(Stdio::null())
                        .stderr(Stdio::null()),
                    "run zstd compression benchmark",
                )?;
            }
            Ok(())
        },
    ) {
        Ok(value) => value,
        Err(note) => {
            return failed_row(
                ImplementationKind::Official,
                format!("{version} -{level}"),
                level,
                corpus.total_bytes,
                note,
            );
        }
    };

    if let Err(note) = warm_up(config.warmup_iterations, || {
        for path in &compressed_paths {
            run_command(
                Command::new("zstd")
                    .arg("-q")
                    .arg("-f")
                    .arg("-d")
                    .arg(path)
                    .arg("-c")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null()),
                "run zstd decompression warmup",
            )?;
        }
        Ok(())
    }) {
        return failed_row(
            ImplementationKind::Official,
            format!("{version} -{level}"),
            level,
            corpus.total_bytes,
            note,
        );
    }

    let decompression_mbps = match measure_throughput(
        corpus.total_bytes,
        config.min_bench_time,
        config.measure_iterations,
        || {
            for path in &compressed_paths {
                run_command(
                    Command::new("zstd")
                        .arg("-q")
                        .arg("-f")
                        .arg("-d")
                        .arg(path)
                        .arg("-c")
                        .stdout(Stdio::null())
                        .stderr(Stdio::null()),
                    "run zstd decompression benchmark",
                )?;
            }
            Ok(())
        },
    ) {
        Ok(value) => value,
        Err(note) => {
            return failed_row(
                ImplementationKind::Official,
                format!("{version} -{level}"),
                level,
                corpus.total_bytes,
                note,
            );
        }
    };

    let _ = fs::remove_dir_all(&temp_root);

    ResultRow {
        impl_kind: ImplementationKind::Official,
        compressor_name: format!("{version} -{level}"),
        level,
        ratio: Some(corpus.total_bytes as f64 / total_compressed_bytes as f64),
        compression_mbps: Some(compression_mbps),
        decompression_mbps: Some(decompression_mbps),
        total_input_bytes: corpus.total_bytes,
        total_compressed_bytes: Some(total_compressed_bytes),
        note: None,
    }
}

fn failed_row(
    impl_kind: ImplementationKind,
    compressor_name: String,
    level: i32,
    total_input_bytes: usize,
    note: String,
) -> ResultRow {
    ResultRow {
        impl_kind,
        compressor_name,
        level,
        ratio: None,
        compression_mbps: None,
        decompression_mbps: None,
        total_input_bytes,
        total_compressed_bytes: None,
        note: Some(note),
    }
}

fn warm_up(
    mut iterations: usize,
    mut run_once: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    while iterations > 0 {
        run_once()?;
        iterations -= 1;
    }
    Ok(())
}

fn measure_throughput(
    total_bytes: usize,
    min_duration: Duration,
    min_iterations: usize,
    mut run_once: impl FnMut() -> Result<(), String>,
) -> Result<f64, String> {
    let started = Instant::now();
    let mut iterations = 0usize;
    while iterations < min_iterations || started.elapsed() < min_duration {
        run_once()?;
        iterations += 1;
    }
    let elapsed = started.elapsed();
    let total_megabytes = (total_bytes as f64 * iterations as f64) / 1_000_000.0;
    Ok(total_megabytes / elapsed.as_secs_f64())
}

fn detect_zstd_version() -> Result<String, String> {
    let output = Command::new("zstd")
        .arg("--version")
        .output()
        .map_err(|err| format!("failed to run zstd --version: {err}"))?;
    if !output.status.success() {
        return Err(format!("zstd --version exited with {}", output.status));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(shorten_zstd_version(
        stdout.lines().next().unwrap_or("zstd").trim(),
    ))
}

fn shorten_zstd_version(version_line: &str) -> String {
    let trimmed = version_line.trim();
    if trimmed.is_empty() {
        return "zstd".to_string();
    }

    if let Some(version) = trimmed
        .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '*')
        .find(|token| {
            token.starts_with('v')
                && token
                    .chars()
                    .nth(1)
                    .is_some_and(|ch| ch.is_ascii_digit())
        })
    {
        return format!("zstd {}", &version[1..]);
    }

    if let Some((name, version)) = trimmed.split_once(' ') {
        if !name.is_empty() && !version.is_empty() {
            return format!("{name} {version}");
        }
    }

    trimmed.to_string()
}

fn run_command(command: &mut Command, action: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|err| format!("failed to {action}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{action} exited with status {status}"))
    }
}

fn render_terminal_table(results: &[ResultRow]) -> String {
    let name_header = "Compressor name";
    let ratio_header = "Ratio";
    let compression_header = "Compression";
    let decompression_header = "Decompression";

    let rows = results
        .iter()
        .map(|row| {
            (
                row.compressor_name.as_str(),
                format_ratio(row.ratio),
                format_speed(row.compression_mbps),
                format_speed(row.decompression_mbps),
            )
        })
        .collect::<Vec<_>>();

    let name_width = rows
        .iter()
        .map(|(name, _, _, _)| name.len())
        .max()
        .unwrap_or(0)
        .max(name_header.len());
    let ratio_width = rows
        .iter()
        .map(|(_, ratio, _, _)| ratio.len())
        .max()
        .unwrap_or(0)
        .max(ratio_header.len());
    let compression_width = rows
        .iter()
        .map(|(_, _, compression, _)| compression.len())
        .max()
        .unwrap_or(0)
        .max(compression_header.len());
    let decompression_width = rows
        .iter()
        .map(|(_, _, _, decompression)| decompression.len())
        .max()
        .unwrap_or(0)
        .max(decompression_header.len());

    let mut output = String::new();
    output.push_str(&format!(
        "| {name_header:<name_width$} | {ratio_header:>ratio_width$} | {compression_header:>compression_width$} | {decompression_header:>decompression_width$} |\n"
    ));
    output.push_str(&format!(
        "| {:-<name_width$} | {:-<ratio_width$} | {:-<compression_width$} | {:-<decompression_width$} |\n",
        "",
        "",
        "",
        "",
    ));
    for (name, ratio, compression, decompression) in rows {
        output.push_str(&format!(
            "| {name:<name_width$} | {ratio:>ratio_width$} | {compression:>compression_width$} | {decompression:>decompression_width$} |\n"
        ));
    }
    append_notes(&mut output, results);
    output
}

fn render_markdown_table(results: &[ResultRow], corpus: &Corpus) -> String {
    let mut output = String::new();
    output.push_str("# Silesia Comparison\n\n");
    output.push_str(&format!(
        "Corpus: {} files, {:.1} MiB total.\n\n",
        corpus.files.len(),
        corpus.total_bytes as f64 / (1024.0 * 1024.0)
    ));
    output.push_str("| Compressor name | Ratio | Compression | Decompress. |\n");
    output.push_str("| --------------- | -----:| -----------:| -----------:|\n");
    for row in results {
        output.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            row.compressor_name,
            format_ratio(row.ratio),
            format_speed(row.compression_mbps),
            format_speed(row.decompression_mbps)
        ));
    }
    append_notes(&mut output, results);
    output
}

fn render_json(results: &[ResultRow], corpus: &Corpus) -> String {
    let mut rows = String::new();
    for (index, row) in results.iter().enumerate() {
        if index > 0 {
            rows.push_str(",\n");
        }
        rows.push_str(&format!(
            "    {{\"implementation\":\"{}\",\"name\":{},\"level\":{},\"ratio\":{},\"compression_mbps\":{},\"decompression_mbps\":{},\"input_bytes\":{},\"compressed_bytes\":{},\"note\":{}}}",
            match row.impl_kind {
                ImplementationKind::Ours => "ours",
                ImplementationKind::Official => "official",
            },
            json_string(&row.compressor_name),
            row.level,
            row.ratio
                .map(|value| format!("{value:.6}"))
                .unwrap_or_else(|| "null".to_string()),
            row.compression_mbps
                .map(|value| format!("{value:.3}"))
                .unwrap_or_else(|| "null".to_string()),
            row.decompression_mbps
                .map(|value| format!("{value:.3}"))
                .unwrap_or_else(|| "null".to_string()),
            row.total_input_bytes,
            row.total_compressed_bytes
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".to_string()),
            row.note
                .as_deref()
                .map(json_string)
                .unwrap_or_else(|| "null".to_string())
        ));
    }

    format!(
        "{{\n  \"corpus\": {{\"files\": {}, \"total_bytes\": {}}},\n  \"results\": [\n{}\n  ]\n}}\n",
        corpus.files.len(),
        corpus.total_bytes,
        rows
    )
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn render_svg(results: &[ResultRow], corpus: &Corpus) -> String {
    let successful = results
        .iter()
        .filter(|row| {
            row.ratio.is_some()
                && row.compression_mbps.is_some()
                && row.decompression_mbps.is_some()
        })
        .collect::<Vec<_>>();
    let width = 1200.0;
    let height = 560.0;
    let panel_width = 500.0;
    let panel_height = 360.0;
    let top = 120.0;
    let left_compress = 80.0;
    let left_decompress = 640.0;

    let min_ratio = successful
        .iter()
        .filter_map(|row| row.ratio)
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(1.0);
    let max_ratio = successful
        .iter()
        .filter_map(|row| row.ratio)
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .max(min_ratio + 0.5);
    let max_compress = successful
        .iter()
        .filter_map(|row| row.compression_mbps)
        .fold(0.0, f64::max)
        * 1.1;
    let max_decompress = successful
        .iter()
        .filter_map(|row| row.decompression_mbps)
        .fold(0.0, f64::max)
        * 1.1;

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">"
    ));
    svg.push_str("<defs><linearGradient id=\"bg\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0%\" stop-color=\"#f7f1e3\"/><stop offset=\"100%\" stop-color=\"#fffaf1\"/></linearGradient></defs>");
    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"url(#bg)\"/>");
    svg.push_str("<text x=\"60\" y=\"52\" font-family=\"Georgia, serif\" font-size=\"28\" font-weight=\"700\" fill=\"#231f20\">Silesia Benchmark Comparison</text>");
    svg.push_str(&format!(
        "<text x=\"60\" y=\"80\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"15\" fill=\"#4b4b4b\">{} files, {:.1} MiB total. Style modeled after the official zstd README benchmark figures.</text>",
        corpus.files.len(),
        corpus.total_bytes as f64 / (1024.0 * 1024.0)
    ));

    render_panel(
        &mut svg,
        "Compression Speed vs Ratio",
        left_compress,
        top,
        panel_width,
        panel_height,
        min_ratio,
        max_ratio,
        0.0,
        max_compress.max(1.0),
        "Compression speed (MB/s)",
        results,
        |row| row.compression_mbps,
    );

    render_panel(
        &mut svg,
        "Decompression Speed",
        left_decompress,
        top,
        panel_width,
        panel_height,
        min_ratio,
        max_ratio,
        0.0,
        max_decompress.max(1.0),
        "Decompression speed (MB/s)",
        results,
        |row| row.decompression_mbps,
    );

    svg.push_str("<g>");
    svg.push_str("<rect x=\"60\" y=\"500\" width=\"18\" height=\"18\" rx=\"4\" fill=\"#c75b12\"/>");
    svg.push_str("<text x=\"86\" y=\"514\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"14\" fill=\"#222\">zstd_rs</text>");
    svg.push_str(
        "<rect x=\"190\" y=\"500\" width=\"18\" height=\"18\" rx=\"4\" fill=\"#146356\"/>",
    );
    svg.push_str("<text x=\"216\" y=\"514\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"14\" fill=\"#222\">official zstd</text>");
    svg.push_str("</g>");
    let mut note_y = 472.0;
    for row in results.iter().filter(|row| row.note.is_some()) {
        svg.push_str(&format!(
            "<text x=\"360\" y=\"{note_y}\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"12\" fill=\"#7a2f2f\">{}</text>",
            escape_xml(&format!(
                "{}: {}",
                row.compressor_name,
                row.note.as_deref().unwrap_or("")
            ))
        ));
        note_y += 16.0;
    }
    svg.push_str("</svg>");
    svg
}

fn render_panel(
    svg: &mut String,
    title: &str,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    min_ratio: f64,
    max_ratio: f64,
    min_speed: f64,
    max_speed: f64,
    axis_label: &str,
    results: &[ResultRow],
    metric: impl Fn(&ResultRow) -> Option<f64>,
) {
    let bottom = top + height;
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" font-family=\"Georgia, serif\" font-size=\"20\" font-weight=\"700\" fill=\"#222\">{}</text>",
        left,
        top - 18.0,
        escape_xml(title)
    ));
    svg.push_str(&format!(
        "<rect x=\"{left}\" y=\"{top}\" width=\"{width}\" height=\"{height}\" rx=\"18\" fill=\"#fffdf8\" stroke=\"#d7cfbe\" stroke-width=\"1.5\"/>"
    ));

    for step in 0..=4 {
        let y = top + height - (height * step as f64 / 4.0);
        let value = min_ratio + (max_ratio - min_ratio) * step as f64 / 4.0;
        svg.push_str(&format!(
            "<line x1=\"{left}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"#e9e1cf\" stroke-width=\"1\"/>",
            left + width
        ));
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"12\" fill=\"#6d675d\" text-anchor=\"end\">{:.2}</text>",
            left - 8.0,
            y + 4.0,
            value
        ));
    }

    for step in 0..=4 {
        let x = left + width * step as f64 / 4.0;
        let value = min_speed + (max_speed - min_speed) * step as f64 / 4.0;
        svg.push_str(&format!(
            "<line x1=\"{x}\" y1=\"{top}\" x2=\"{x}\" y2=\"{bottom}\" stroke=\"#f0e9d7\" stroke-width=\"1\"/>"
        ));
        svg.push_str(&format!(
            "<text x=\"{x}\" y=\"{}\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"12\" fill=\"#6d675d\" text-anchor=\"middle\">{:.0}</text>",
            bottom + 20.0,
            value
        ));
    }

    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"13\" fill=\"#4b4b4b\" text-anchor=\"middle\">{}</text>",
        left + width / 2.0,
        bottom + 46.0,
        escape_xml(axis_label)
    ));
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"13\" fill=\"#4b4b4b\" text-anchor=\"middle\" transform=\"rotate(-90 {} {})\">Compression ratio</text>",
        left - 52.0,
        top + height / 2.0,
        left - 52.0,
        top + height / 2.0
    ));

    for row in results {
        let Some(speed) = metric(row) else {
            continue;
        };
        let Some(ratio) = row.ratio else {
            continue;
        };
        let x = scale(
            speed,
            min_speed,
            max_speed,
            left + 20.0,
            left + width - 20.0,
        );
        let y = scale(ratio, min_ratio, max_ratio, bottom - 20.0, top + 20.0);
        let label_y = if row.impl_kind == ImplementationKind::Ours {
            y - 12.0
        } else {
            y + 22.0
        };
        svg.push_str(&format!(
            "<circle cx=\"{x:.2}\" cy=\"{y:.2}\" r=\"8.5\" fill=\"{}\" stroke=\"#fff8ef\" stroke-width=\"2\"/>",
            row.impl_kind.color()
        ));
        svg.push_str(&format!(
            "<text x=\"{x:.2}\" y=\"{label_y:.2}\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"12\" fill=\"#222\" text-anchor=\"middle\">L{}</text>",
            row.level
        ));
    }
}

fn scale(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    if (from_max - from_min).abs() < f64::EPSILON {
        return (to_min + to_max) * 0.5;
    }
    let t = (value - from_min) / (from_max - from_min);
    to_min + t * (to_max - to_min)
}

fn format_ratio(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "failed".to_string())
}

fn format_speed(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1} MB/s"))
        .unwrap_or_else(|| "failed".to_string())
}

fn append_notes(output: &mut String, results: &[ResultRow]) {
    let failures = results
        .iter()
        .filter_map(|row| row.note.as_ref().map(|note| (row, note)))
        .collect::<Vec<_>>();
    if failures.is_empty() {
        return;
    }

    output.push_str("\nFailures:\n");
    for (row, note) in failures {
        output.push_str(&format!("- {}: {}\n", row.compressor_name, note));
    }
}

#[cfg(test)]
mod tests {
    use super::shorten_zstd_version;

    #[test]
    fn shortens_verbose_zstd_cli_banner() {
        assert_eq!(
            shorten_zstd_version("*** Zstandard CLI (64-bit) v1.5.5, by Yann Collet ***"),
            "zstd 1.5.5"
        );
    }

    #[test]
    fn preserves_simple_name_and_version() {
        assert_eq!(shorten_zstd_version("zstd 1.5.7"), "zstd 1.5.7");
    }

    #[test]
    fn falls_back_to_plain_name() {
        assert_eq!(shorten_zstd_version("zstd"), "zstd");
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
