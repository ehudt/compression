use std::path::{Path, PathBuf};

pub const TESTS_ENV_VAR: &str = "ZSTD_RS_PROFILE_TESTS";

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

impl ProfileSession {
    pub fn disabled() -> Self {
        Self {
            inner: Inner::Disabled,
        }
    }

    pub fn from_output_path(path: impl Into<PathBuf>) -> Result<Self, String> {
        #[cfg(feature = "profiling")]
        {
            let output_path = path.into();
            let guard = pprof::ProfilerGuard::new(100)
                .map_err(|err| format!("profiling setup failed: {err}"))?;
            Ok(Self {
                inner: Inner::Active(ActiveProfileSession { guard, output_path }),
            })
        }

        #[cfg(not(feature = "profiling"))]
        {
            let _ = path.into();
            Err("profiling support is not compiled in; rebuild with `--features profiling`".into())
        }
    }

    pub fn from_test_env(test_name: &str) -> Result<Self, String> {
        match std::env::var(TESTS_ENV_VAR) {
            Ok(dir) if !dir.trim().is_empty() => {
                let file_name = format!("{}.svg", sanitize_label(test_name));
                Self::from_output_path(Path::new(&dir).join(file_name))
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
            if let Err(err) = write_flamegraph(active) {
                eprintln!(
                    "failed to write profile to {}: {err}",
                    active.output_path.display()
                );
            }
        }
    }
}

#[cfg(feature = "profiling")]
fn write_flamegraph(active: &ActiveProfileSession) -> Result<(), String> {
    if let Some(parent) = active.output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }

    let report = active
        .guard
        .report()
        .build()
        .map_err(|err| format!("failed to build profile report: {err}"))?;
    let file = std::fs::File::create(&active.output_path)
        .map_err(|err| format!("failed to create {}: {err}", active.output_path.display()))?;
    report
        .flamegraph(file)
        .map_err(|err| format!("failed to render flamegraph: {err}"))?;
    Ok(())
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
