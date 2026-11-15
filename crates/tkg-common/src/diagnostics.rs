//! Structured diagnostic logging configured via `-v`/`-vv`/`-vvv` or
//! `RUST_LOG`, always directed to `stderr` so `stdout` wire protocols
//! (`tkg-mcp` JSON-RPC and `tkg-hooks` `ProtoJSON`) and TUI output remain
//! clean.

#[cfg(feature = "diagnostics")]
use tracing_subscriber::EnvFilter;

/// Default verbosity level when neither `-v` flags nor `RUST_LOG` are set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultVerbosity {
    /// Quiet default suitable for ephemeral hooks and interactive TUI screens.
    Warn,
    /// Lifecycle info default suitable for long-lived daemons (`tkg-mcp`).
    Info,
}

/// Counts `-v`, `-vv`, `-vvv`, and `--verbose` CLI arguments and returns
/// `(verbosity, stripped_args)` with verbosity flags removed (stopping at
/// `--`).
#[must_use]
pub fn extract_verbosity_and_strip_args(args: &[String]) -> (u8, Vec<String>) {
    let mut verbosity: u8 = 0;
    let mut stripped = Vec::with_capacity(args.len());
    let mut passthrough = false;

    for (idx, arg) in args.iter().enumerate() {
        if idx == 0 || passthrough {
            stripped.push(arg.clone());
            continue;
        }
        if arg == "--" {
            passthrough = true;
            stripped.push(arg.clone());
            continue;
        }
        if arg == "--verbose" {
            verbosity = verbosity.saturating_add(1).min(3);
            continue;
        }
        if arg.starts_with("-v") && arg.len() > 1 && arg.bytes().skip(1).all(|b| b == b'v') {
            let count = u8::try_from(arg.len().saturating_sub(1)).unwrap_or(3);
            verbosity = verbosity.saturating_add(count).min(3);
            continue;
        }
        stripped.push(arg.clone());
    }

    (verbosity, stripped)
}

/// Resolves the `EnvFilter` directive string from CLI verbosity and `RUST_LOG`.
#[must_use]
pub fn resolve_filter_directive(
    verbosity: u8,
    default: DefaultVerbosity,
    env_override: Option<&str>,
) -> String {
    match verbosity {
        0 => {
            if let Some(env_val) = env_override.map(str::trim).filter(|s| !s.is_empty()) {
                env_val.to_owned()
            } else {
                match default {
                    DefaultVerbosity::Warn => "warn".to_owned(),
                    DefaultVerbosity::Info => "info".to_owned(),
                }
            }
        },
        1 => "info".to_owned(),
        2 => "debug".to_owned(),
        _ => "trace".to_owned(),
    }
}

/// Initializes global `tracing` diagnostics directed to `stderr`, returning
/// the CLI arguments with `-v`/`--verbose` flags removed.
///
/// Subsequent invocations in the same process (e.g. across unit tests) are
/// safely ignored.
pub fn init_stderr_tracing(args: &[String], default: DefaultVerbosity) -> Vec<String> {
    let (verbosity, stripped) = extract_verbosity_and_strip_args(args);
    let env_override = std::env::var("RUST_LOG").ok();
    let directive = resolve_filter_directive(verbosity, default, env_override.as_deref());
    let show_targets = verbosity >= 2 || env_override.is_some();

    #[cfg(feature = "diagnostics")]
    {
        let filter = EnvFilter::try_new(directive).unwrap_or_else(|_| EnvFilter::new("warn"));
        if tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(filter)
            .with_target(show_targets)
            .without_time()
            .try_init()
            .is_err()
        {
            // Subscriber already installed earlier in process.
        }
    }

    #[cfg(not(feature = "diagnostics"))]
    {
        let subscriber = MinimalStderrSubscriber::from_directive(&directive, show_targets);
        if tracing::subscriber::set_global_default(subscriber).is_err() {
            // Subscriber already installed earlier in process.
        }
    }

    stripped
}

#[cfg(not(feature = "diagnostics"))]
struct MinimalStderrSubscriber {
    max_level: tracing::level_filters::LevelFilter,
    target_prefix: Option<String>,
    show_target: bool,
}

#[cfg(not(feature = "diagnostics"))]
impl MinimalStderrSubscriber {
    fn from_directive(directive: &str, show_target: bool) -> Self {
        let mut max_level = tracing::level_filters::LevelFilter::WARN;
        let mut target_prefix = None;

        for part in directive
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            let (target, level_str) = match part.split_once('=') {
                Some((t, l)) => (Some(t.trim()), l.trim()),
                None => (None, part),
            };
            let parsed = parse_level_filter(level_str);
            if target.is_none_or(|t| t.starts_with("tkg")) {
                max_level = parsed;
                if let Some(t) = target {
                    target_prefix = Some(t.to_owned());
                }
            }
        }

        Self {
            max_level,
            target_prefix,
            show_target,
        }
    }
}

#[cfg(not(feature = "diagnostics"))]
fn parse_level_filter(raw: &str) -> tracing::level_filters::LevelFilter {
    use tracing::level_filters::LevelFilter;
    match raw.to_ascii_lowercase().as_str() {
        "off" => LevelFilter::OFF,
        "error" => LevelFilter::ERROR,
        "info" => LevelFilter::INFO,
        "debug" => LevelFilter::DEBUG,
        "trace" => LevelFilter::TRACE,
        _ => LevelFilter::WARN,
    }
}

#[cfg(not(feature = "diagnostics"))]
struct LineVisitor {
    buf: String,
    fields: String,
}

#[cfg(not(feature = "diagnostics"))]
impl tracing::field::Visit for LineVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.buf.push_str(&format!("{value:?}"));
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            self.fields.push_str(&format!("{}={value:?}", field.name()));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.buf.push_str(value);
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            self.fields.push_str(field.name());
            self.fields.push('=');
            self.fields.push_str(value);
        }
    }
}

#[cfg(not(feature = "diagnostics"))]
impl tracing::Subscriber for MinimalStderrSubscriber {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        let Some(level) = self.max_level.into_level() else {
            return false;
        };
        if *metadata.level() > level {
            return false;
        }
        match &self.target_prefix {
            Some(prefix) => metadata.target().starts_with(prefix),
            None => true,
        }
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        use std::io::Write as _;
        let mut visitor = LineVisitor {
            buf: String::new(),
            fields: String::new(),
        };
        event.record(&mut visitor);

        let meta = event.metadata();
        let mut line = if self.show_target {
            format!("{} {}: {}", meta.level(), meta.target(), visitor.buf)
        } else {
            format!("{} {}", meta.level(), visitor.buf)
        };
        if !visitor.fields.is_empty() {
            line.push(' ');
            line.push_str(&visitor.fields);
        }
        line.push('\n');

        let mut stderr = std::io::stderr().lock();
        if stderr.write_all(line.as_bytes()).is_err() {
            // Broken stderr pipe is ignored during best-effort diagnostics.
        }
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_verbosity_flags_preserves_subcommands_and_passthrough() {
        let args: Vec<String> = ["tkg", "-vv", "launch", "coder", "--", "-vvv"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let (v, rest) = extract_verbosity_and_strip_args(&args);
        assert_eq!(v, 2);
        assert_eq!(rest, vec!["tkg", "launch", "coder", "--", "-vvv"]);
    }

    #[test]
    fn test_extract_verbosity_accumulates_mixed_flags_and_clamps() {
        let args: Vec<String> = ["tkg-hooks", "-v", "--verbose", "-vv", "detect-wip"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let (v, rest) = extract_verbosity_and_strip_args(&args);
        assert_eq!(v, 3);
        assert_eq!(rest, vec!["tkg-hooks", "detect-wip"]);
    }

    #[test]
    fn test_resolve_filter_directive_precedence() {
        assert_eq!(
            resolve_filter_directive(0, DefaultVerbosity::Warn, None),
            "warn"
        );
        assert_eq!(
            resolve_filter_directive(0, DefaultVerbosity::Info, None),
            "info"
        );
        assert_eq!(
            resolve_filter_directive(0, DefaultVerbosity::Warn, Some("tkg_mcp=debug")),
            "tkg_mcp=debug"
        );
        assert_eq!(
            resolve_filter_directive(1, DefaultVerbosity::Warn, Some("error")),
            "info"
        );
        assert_eq!(
            resolve_filter_directive(2, DefaultVerbosity::Warn, None),
            "debug"
        );
        assert_eq!(
            resolve_filter_directive(3, DefaultVerbosity::Warn, None),
            "trace"
        );
    }
}
