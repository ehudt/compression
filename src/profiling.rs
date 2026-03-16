#[cfg(feature = "profiling")]
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const TESTS_ENV_VAR: &str = "ZSTD_RS_PROFILE_TESTS";
pub const DEFAULT_PROFILE_FREQUENCY_HZ: i32 = 1_000;
#[cfg(feature = "profiling")]
const MIN_USEFUL_SAMPLE_COUNT: usize = 50;

pub struct ProfileSession {
    #[cfg_attr(not(feature = "profiling"), allow(dead_code))]
    inner: Inner,
}

enum Inner {
    Disabled,
    #[cfg(feature = "profiling")]
    Active(ActiveProfileSession),
}

#[cfg(feature = "profiling")]
struct ActiveProfileSession {
    guard: pprof::ProfilerGuard<'static>,
    output_path: PathBuf,
}

#[cfg(feature = "profiling")]
pub struct ProfileArtifacts {
    pub svg_path: PathBuf,
    pub folded_path: PathBuf,
    pub summary_path: PathBuf,
}

impl ProfileSession {
    pub fn disabled() -> Self {
        Self {
            inner: Inner::Disabled,
        }
    }

    pub fn from_output_path(path: impl Into<PathBuf>, frequency_hz: i32) -> Result<Self, String> {
        #[cfg(feature = "profiling")]
        {
            let output_path = path.into();
            let guard = pprof::ProfilerGuard::new(frequency_hz)
                .map_err(|err| format!("profiling setup failed: {err}"))?;
            Ok(Self {
                inner: Inner::Active(ActiveProfileSession { guard, output_path }),
            })
        }

        #[cfg(not(feature = "profiling"))]
        {
            let _ = (path.into(), frequency_hz);
            Err("profiling support is not compiled in; rebuild with `--features profiling`".into())
        }
    }

    pub fn from_test_env(test_name: &str) -> Result<Self, String> {
        match std::env::var(TESTS_ENV_VAR) {
            Ok(dir) if !dir.trim().is_empty() => {
                let file_name = format!("{}.svg", sanitize_label(test_name));
                Self::from_output_path(
                    Path::new(&dir).join(file_name),
                    DEFAULT_PROFILE_FREQUENCY_HZ,
                )
            }
            Ok(_) => Ok(Self::disabled()),
            Err(std::env::VarError::NotPresent) => Ok(Self::disabled()),
            Err(err) => Err(format!("failed to read {TESTS_ENV_VAR}: {err}")),
        }
    }
}

impl Drop for ProfileSession {
    fn drop(&mut self) {
        #[cfg(feature = "profiling")]
        if let Inner::Active(active) = &self.inner {
            if let Err(err) = write_profile_outputs(active) {
                eprintln!(
                    "failed to write profile to {}: {err}",
                    active.output_path.display()
                );
            }
        }
    }
}

#[cfg(feature = "profiling")]
fn write_profile_outputs(active: &ActiveProfileSession) -> Result<(), String> {
    let report = active
        .guard
        .report()
        .build()
        .map_err(|err| format!("failed to build profile report: {err}"))?;

    write_report_outputs(&report, &active.output_path)?;
    Ok(())
}

#[cfg(feature = "profiling")]
pub fn write_report_outputs(
    report: &pprof::Report,
    output_path: &Path,
) -> Result<ProfileArtifacts, String> {
    let artifacts = profile_artifacts(output_path);
    let sample_count = total_samples(report);

    for path in [
        &artifacts.svg_path,
        &artifacts.folded_path,
        &artifacts.summary_path,
    ] {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
    }

    let file = std::fs::File::create(&artifacts.svg_path)
        .map_err(|err| format!("failed to create {}: {err}", artifacts.svg_path.display()))?;
    report
        .flamegraph(file)
        .map_err(|err| format!("failed to render flamegraph: {err}"))?;

    let folded = render_folded_stacks(report);
    std::fs::write(&artifacts.folded_path, folded)
        .map_err(|err| format!("failed to write {}: {err}", artifacts.folded_path.display()))?;

    let summary = render_summary(report);
    std::fs::write(&artifacts.summary_path, summary).map_err(|err| {
        format!(
            "failed to write {}: {err}",
            artifacts.summary_path.display()
        )
    })?;

    if let Some(warning) = sparse_profile_warning(sample_count, report.timing.frequency) {
        eprintln!("warning: {warning} ({})", artifacts.summary_path.display());
    }

    Ok(artifacts)
}

#[cfg(feature = "profiling")]
fn profile_artifacts(output_path: &Path) -> ProfileArtifacts {
    let is_svg = output_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
    let base = if is_svg {
        output_path.with_extension("")
    } else {
        output_path.to_path_buf()
    };

    ProfileArtifacts {
        svg_path: if is_svg {
            output_path.to_path_buf()
        } else {
            output_path.with_extension("svg")
        },
        folded_path: base.with_extension("folded"),
        summary_path: base.with_extension("summary.txt"),
    }
}

#[cfg(feature = "profiling")]
fn render_folded_stacks(report: &pprof::Report) -> String {
    let mut lines: Vec<(String, isize)> = report
        .data
        .iter()
        .map(|(frames, count)| (folded_stack_line(frames), *count))
        .collect();
    lines.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let mut output = String::new();
    for (line, count) in lines {
        output.push_str(&line);
        output.push(' ');
        output.push_str(&count.to_string());
        output.push('\n');
    }
    output
}

#[cfg(feature = "profiling")]
fn render_summary(report: &pprof::Report) -> String {
    let total_samples = total_samples(report);
    let mut inclusive: HashMap<String, isize> = HashMap::new();
    let mut leaf: HashMap<String, isize> = HashMap::new();
    let mut stacks: Vec<(String, isize)> = Vec::new();

    for (frames, count) in &report.data {
        let symbols = stack_symbols(frames);
        if let Some(last) = symbols.last() {
            *leaf.entry(last.clone()).or_insert(0) += *count;
        }
        let mut unique_symbols = HashSet::new();
        for symbol in &symbols {
            if !unique_symbols.insert(symbol.clone()) {
                continue;
            }
            *inclusive.entry(symbol.clone()).or_insert(0) += *count;
        }
        stacks.push((folded_stack_line(frames), *count));
    }

    let mut output = String::new();
    output.push_str("Profile summary\n");
    output.push_str(&format!("samples: {total_samples}\n"));
    output.push_str(&format!("frequency_hz: {}\n", report.timing.frequency));
    output.push_str(&format!(
        "duration_ms: {:.3}\n",
        report.timing.duration.as_secs_f64() * 1000.0
    ));
    output.push_str(&format!("unique_stacks: {}\n", report.data.len()));
    if let Some(warning) = sparse_profile_warning(total_samples, report.timing.frequency) {
        output.push_str(&format!("warning: {warning}\n"));
    }

    append_ranked_section(&mut output, "Top leaf symbols", &leaf, total_samples, 15);
    append_ranked_section(
        &mut output,
        "Top inclusive symbols",
        &inclusive,
        total_samples,
        15,
    );
    append_ranked_stacks(&mut output, &stacks, total_samples, 15);

    output
}

#[cfg(feature = "profiling")]
fn total_samples(report: &pprof::Report) -> usize {
    let total_samples: isize = report.data.values().copied().sum();
    total_samples.max(0) as usize
}

#[cfg(feature = "profiling")]
fn sparse_profile_warning(total_samples: usize, frequency_hz: i32) -> Option<String> {
    if total_samples >= MIN_USEFUL_SAMPLE_COUNT {
        return None;
    }

    Some(format!(
        "profile captured only {total_samples} samples at {frequency_hz} Hz; results are likely too sparse to trust. Increase --profile-repeat, use a larger input, or raise --profile-hz."
    ))
}

#[cfg(feature = "profiling")]
fn append_ranked_section(
    output: &mut String,
    title: &str,
    counts: &HashMap<String, isize>,
    total_samples: usize,
    limit: usize,
) {
    output.push('\n');
    output.push_str(title);
    output.push_str(":\n");

    let mut entries: Vec<_> = counts.iter().collect();
    entries.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));

    for (name, count) in entries.into_iter().take(limit) {
        output.push_str(&format_summary_line(name, *count, total_samples));
    }
}

#[cfg(feature = "profiling")]
fn append_ranked_stacks(
    output: &mut String,
    stacks: &[(String, isize)],
    total_samples: usize,
    limit: usize,
) {
    output.push('\n');
    output.push_str("Top stacks:\n");

    let mut entries = stacks.to_vec();
    entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    for (stack, count) in entries.into_iter().take(limit) {
        output.push_str(&format_summary_line(&stack, count, total_samples));
    }
}

#[cfg(feature = "profiling")]
fn format_summary_line(name: &str, count: isize, total_samples: usize) -> String {
    let percentage = if total_samples == 0 {
        0.0
    } else {
        (count.max(0) as f64 / total_samples as f64) * 100.0
    };
    format!("  {:>8} {:>6.2}% {name}\n", count, percentage)
}

#[cfg(feature = "profiling")]
fn folded_stack_line(frames: &pprof::Frames) -> String {
    stack_symbols(frames).join(";")
}

#[cfg(feature = "profiling")]
fn stack_symbols(frames: &pprof::Frames) -> Vec<String> {
    let mut symbols = Vec::new();
    symbols.push(sanitize_stack_component(&frames.thread_name_or_id()));

    for frame in frames.frames.iter().rev() {
        for symbol in frame.iter().rev() {
            symbols.push(sanitize_stack_component(&symbol.to_string()));
        }
    }

    symbols
}

#[cfg(feature = "profiling")]
fn sanitize_stack_component(component: &str) -> String {
    component
        .chars()
        .map(|ch| match ch {
            ';' | '\n' | '\r' | '\t' => '_',
            _ => ch,
        })
        .collect()
}

fn sanitize_label(label: &str) -> String {
    let mut sanitized = String::with_capacity(label.len());
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }

    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "profile".to_string()
    } else {
        sanitized.to_string()
    }
}
