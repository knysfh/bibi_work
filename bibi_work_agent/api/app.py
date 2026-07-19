from __future__ import annotations

from fastapi import FastAPI
from opentelemetry.trace import SpanKind

from bibi_work_agent.api.internal_routes import router as internal_router
from bibi_work_agent.telemetry import configure_telemetry, extract_context, tracer


configure_telemetry()
app = FastAPI(title="Bibi Work Agent Runtime", version="0.1.0")


@app.middleware("http")
async def trace_http_request(request, call_next):
    parent_context = extract_context(dict(request.headers))
    attributes = {
        "http.request.method": request.method,
        "url.path": request.url.path,
    }
    with tracer.start_as_current_span(
        "http.request",
        context=parent_context,
        kind=SpanKind.SERVER,
        attributes=attributes,
    ) as span:
        response = await call_next(request)
        span.set_attribute("http.response.status_code", response.status_code)
        return response


app.include_router(internal_router)
