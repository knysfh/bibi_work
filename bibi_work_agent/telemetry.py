from __future__ import annotations

from collections.abc import Mapping, MutableMapping
from threading import Lock
from typing import Any

from opentelemetry import context, propagate, trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.resources import SERVICE_NAME, Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.sdk.trace.sampling import ParentBased, TraceIdRatioBased

from bibi_work_agent.settings import Settings, settings


_configure_lock = Lock()
_configured = False


def _traces_endpoint(endpoint: str) -> str:
    endpoint = endpoint.rstrip("/")
    if endpoint.endswith("/v1/traces"):
        return endpoint
    return f"{endpoint}/v1/traces"


def configure_telemetry(configuration: Settings = settings) -> None:
    global _configured
    with _configure_lock:
        if _configured:
            return
        if not configuration.otlp_enabled:
            _configured = True
            return
        if not 0.0 <= configuration.trace_sample_ratio <= 1.0:
            raise ValueError("trace_sample_ratio must be between 0 and 1")
        if not configuration.telemetry_service_name.strip():
            raise ValueError("telemetry_service_name must not be empty")
        if configuration.telemetry_timeout_sec <= 0:
            raise ValueError("telemetry_timeout_sec must be greater than zero")

        provider = TracerProvider(
            resource=Resource.create(
                {SERVICE_NAME: configuration.telemetry_service_name}
            ),
            sampler=ParentBased(TraceIdRatioBased(configuration.trace_sample_ratio)),
        )
        exporter = OTLPSpanExporter(
            endpoint=_traces_endpoint(configuration.otlp_endpoint),
            timeout=configuration.telemetry_timeout_sec,
        )
        provider.add_span_processor(BatchSpanProcessor(exporter))
        trace.set_tracer_provider(provider)
        _configured = True


def extract_context(headers: Mapping[str, Any]) -> context.Context:
    carrier: dict[str, str] = {}
    for key, value in headers.items():
        if not isinstance(key, str) or not isinstance(value, (str, bytes)):
            continue
        carrier[key.lower()] = value.decode("ascii") if isinstance(value, bytes) else value
    return propagate.extract(carrier)


def inject_context(carrier: MutableMapping[str, str]) -> MutableMapping[str, str]:
    propagate.inject(carrier)
    return carrier


def current_trace_headers() -> dict[str, str]:
    return dict(inject_context({}))


tracer = trace.get_tracer("bibi-work-agent")
