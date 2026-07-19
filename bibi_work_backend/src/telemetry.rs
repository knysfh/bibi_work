use std::collections::HashMap;

use axum::{extract::Request, http::HeaderMap, middleware::Next, response::Response};
use opentelemetry::{
    global,
    propagation::{Extractor, Injector},
    trace::TracerProvider as _,
};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{
    Resource,
    propagation::TraceContextPropagator,
    trace::{Sampler, SdkTracerProvider},
};
use tokio::task::JoinHandle;
use tracing::{Instrument, field};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{
    Layer, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

use crate::configuration::TelemetrySettings;

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    _file_guard: WorkerGuard,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = &self.tracer_provider
            && let Err(error) = provider.shutdown()
        {
            eprintln!("failed to flush OpenTelemetry traces: {error}");
        }
    }
}

pub fn init_subscriber(settings: &TelemetrySettings) -> Result<TelemetryGuard, anyhow::Error> {
    settings.validate()?;
    global::set_text_map_propagator(TraceContextPropagator::new());

    let file_appender = tracing_appender::rolling::daily("logs", "app.log");
    let (non_blocking, file_guard) = tracing_appender::non_blocking(file_appender);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(false)
        .with_ansi(false)
        .with_writer(non_blocking)
        .with_filter(LevelFilter::TRACE);

    let tracer_provider = if settings.otlp_enabled {
        let endpoint = settings
            .otlp_endpoint
            .as_deref()
            .expect("validated OTLP endpoint");
        let traces_endpoint = otlp_traces_endpoint(endpoint);
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(traces_endpoint)
            .with_timeout(settings.timeout())
            .build()?;
        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
                settings.trace_sample_ratio,
            ))))
            .with_resource(
                Resource::builder()
                    .with_service_name(settings.service_name.clone())
                    .build(),
            )
            .build();
        let tracer = provider.tracer("bibi-work-backend");
        let otlp_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_filter(LevelFilter::INFO);
        tracing_subscriber::registry()
            .with(file_layer)
            .with(otlp_layer)
            .init();
        Some(provider)
    } else {
        tracing_subscriber::registry().with(file_layer).init();
        None
    };

    Ok(TelemetryGuard {
        tracer_provider,
        _file_guard: file_guard,
    })
}

fn otlp_traces_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint.ends_with("/v1/traces") {
        endpoint.to_owned()
    } else {
        format!("{endpoint}/v1/traces")
    }
}

struct HeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|key| key.as_str()).collect()
    }
}

struct HeaderInjector<'a>(&'a mut HashMap<String, String>);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
}

pub fn current_trace_headers() -> HashMap<String, String> {
    let context = tracing::Span::current().context();
    let mut headers = HashMap::new();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(&mut headers));
    });
    headers
}

pub async fn http_trace_middleware(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let parent_context = global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(request.headers()))
    });
    let span = tracing::info_span!(
        "http.request",
        otel.kind = "server",
        http.request.method = %method,
        url.path = %path,
        http.response.status_code = field::Empty,
    );
    let _ = span.set_parent(parent_context);
    let response = next.run(request).instrument(span.clone()).await;
    span.record("http.response.status_code", response.status().as_u16());
    response
}

pub fn spawn_blocking_with_tracing<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let current_span = tracing::Span::current();
    tokio::task::spawn_blocking(move || current_span.in_scope(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(enabled: bool, endpoint: Option<&str>, ratio: f64) -> TelemetrySettings {
        TelemetrySettings {
            otlp_enabled: enabled,
            otlp_endpoint: endpoint.map(str::to_owned),
            service_name: "test-service".to_owned(),
            trace_sample_ratio: ratio,
            timeout_milliseconds: 1_000,
        }
    }

    #[test]
    fn rejects_enabled_otlp_without_endpoint() {
        let error = settings(true, None, 1.0).validate().unwrap_err();
        assert!(error.to_string().contains("otlp_endpoint"));
    }

    #[test]
    fn rejects_invalid_sample_ratio() {
        let error = settings(false, None, 1.1).validate().unwrap_err();
        assert!(error.to_string().contains("trace_sample_ratio"));
    }

    #[test]
    fn normalizes_otlp_http_traces_endpoint() {
        assert_eq!(
            otlp_traces_endpoint("http://collector:4318/"),
            "http://collector:4318/v1/traces"
        );
        assert_eq!(
            otlp_traces_endpoint("http://collector:4318/v1/traces"),
            "http://collector:4318/v1/traces"
        );
    }
}
