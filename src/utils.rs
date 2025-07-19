use std::{path::PathBuf, sync::LazyLock};

use time::{UtcOffset, format_description::well_known};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{EnvFilter, fmt::time::OffsetTime};

pub static LOCAL_OFFSET: LazyLock<UtcOffset> =
    LazyLock::new(|| match time::UtcOffset::current_local_offset() {
        Ok(offset) => offset,
        Err(e) => {
            tracing::error!("failed to get local offset: {}", e);
            time::UtcOffset::UTC
        }
    });

pub fn now_local() -> time::OffsetDateTime {
    // time::OffsetDateTime::now_local() is hard to use and has performance issue
    time::OffsetDateTime::now_utc().to_offset(*LOCAL_OFFSET)
}

static LOG_FILE_NAME: &str = "trading_engine.log";

/// Initialize the logging system
///
/// # Log Level Configuration
///
/// The log level can be configured through environment variables:
///
/// - **RUST_LOG**: Set the global log level (e.g., `RUST_LOG=debug`, `RUST_LOG=info`, `RUST_LOG=warn`, `RUST_LOG=error`)
/// - **Module-specific levels**: Set different levels for specific modules (e.g., `RUST_LOG=trading_engine=debug,market=info`)
///
/// # Examples
///
/// ```bash
/// # Set global debug level
/// RUST_LOG=debug cargo run
///
/// # Set different levels for different modules
/// RUST_LOG=trading_engine=debug,market=info,order_book=warn cargo run
///
/// # Set level for specific crate
/// RUST_LOG=trading_system_demo=debug cargo run
/// ```
///
/// # Default Behavior
///
/// If no `RUST_LOG` environment variable is set, the default level is `INFO`.
///
/// # Parameters
///
/// - `log_dir`: Optional directory path for log files. If provided, logs will be written to daily rotating files.
///   If `None`, logs will be written to stderr with ANSI colors.
///
/// # Returns
///
/// Returns a `WorkerGuard` that should be kept alive for the duration of the program
/// to ensure all log messages are flushed.
pub fn init_log(log_dir: Option<PathBuf>) -> tracing_appender::non_blocking::WorkerGuard {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    let mut subscriber_builder = tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(env_filter)
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_thread_names(true)
        .with_timer(OffsetTime::new(*LOCAL_OFFSET, well_known::Rfc3339));
    let (non_blocking, guard) = if let Some(log_dir) = log_dir {
        // output to file，daily rotate, non-blocking
        if !log_dir.is_dir() {
            panic!("log path is not a directory");
        }
        let file_appender = tracing_appender::rolling::daily(log_dir, LOG_FILE_NAME);
        tracing_appender::non_blocking(file_appender)
    } else {
        subscriber_builder = subscriber_builder.with_ansi(true);
        // output to stderr
        tracing_appender::non_blocking(std::io::stderr())
    };
    let subscriber = subscriber_builder.with_writer(non_blocking).finish();
    tracing::subscriber::set_global_default(subscriber).expect("init log failed");
    guard
}
