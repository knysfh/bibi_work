from __future__ import annotations

import os
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
    ExportTraceServiceRequest,
)

from bibi_work_agent.telemetry import _traces_endpoint


def test_traces_endpoint_normalizes_collector_base_url() -> None:
    assert _traces_endpoint("http://collector:4318/") == (
        "http://collector:4318/v1/traces"
    )
    assert _traces_endpoint("http://collector:4318/v1/traces") == (
        "http://collector:4318/v1/traces"
    )


def test_real_otlp_export_preserves_incoming_trace_id() -> None:
    requests: list[tuple[str, str | None, bytes]] = []

    class CollectorHandler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:  # noqa: N802
            length = int(self.headers.get("content-length", "0"))
            requests.append(
                (
                    self.path,
                    self.headers.get("content-type"),
                    self.rfile.read(length),
                )
            )
            self.send_response(200)
            self.end_headers()

        def log_message(self, _format: str, *args: object) -> None:
            return None

    collector = ThreadingHTTPServer(("127.0.0.1", 0), CollectorHandler)
    thread = threading.Thread(target=collector.serve_forever, daemon=True)
    thread.start()
    endpoint = f"http://127.0.0.1:{collector.server_port}"
    environment = {
        **os.environ,
        "BIBI_AGENT__OTLP_ENABLED": "true",
        "BIBI_AGENT__OTLP_ENDPOINT": endpoint,
        "BIBI_AGENT__TELEMETRY_SERVICE_NAME": "bibi-work-agent-test",
    }
    source = """
from opentelemetry import trace
from opentelemetry.trace import SpanKind
from bibi_work_agent.telemetry import configure_telemetry, extract_context, tracer

configure_telemetry()
parent = extract_context({
    "traceparent": "00-0123456789abcdeffedcba9876543210-0123456789abcdef-01"
})
with tracer.start_as_current_span("python.contract", context=parent, kind=SpanKind.CONSUMER):
    pass
trace.get_tracer_provider().shutdown()
"""
    try:
        subprocess.run(
            [sys.executable, "-c", source],
            cwd=Path(__file__).parents[2],
            env=environment,
            check=True,
            timeout=10,
        )
    finally:
        collector.shutdown()
        collector.server_close()
        thread.join(timeout=2)

    assert len(requests) == 1
    path, content_type, body = requests[0]
    assert path == "/v1/traces"
    assert content_type == "application/x-protobuf"
    export = ExportTraceServiceRequest.FromString(body)
    spans = [
        span
        for resource_spans in export.resource_spans
        for scope_spans in resource_spans.scope_spans
        for span in scope_spans.spans
    ]
    assert [span.name for span in spans] == ["python.contract"]
    assert spans[0].trace_id == bytes.fromhex("0123456789abcdeffedcba9876543210")
