use tokio::task::JoinHandle;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::FmtSubscriber;

pub fn init_subscriber() -> WorkerGuard {
    let file_appender = tracing_appender::rolling::daily("logs", "app.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(false)
        .with_ansi(false)
        .with_writer(non_blocking)
        .init();

    _guard
}

pub fn spawn_blocking_with_tracing<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let current_span = tracing::Span::current();
    tokio::task::spawn_blocking(move || current_span.in_scope(f))
}
