from __future__ import annotations

import os
from dataclasses import dataclass


def _env_bool(name: str, default: bool = False) -> bool:
    value = os.getenv(name)
    if value is None:
        return default
    return value.strip().lower() in {"1", "true", "yes", "on"}


@dataclass(frozen=True)
class Settings:
    rust_base_url: str = os.getenv("BIBI_AGENT__RUST_BASE_URL", "http://127.0.0.1:8361")
    internal_token: str = os.getenv("BIBI_AGENT__INTERNAL_TOKEN", "")
    embedding_endpoint: str = os.getenv(
        "BIBI_AGENT__EMBEDDING_ENDPOINT",
        "http://172.24.250.231:8335/embed",
    )
    qdrant_rest_url: str = os.getenv(
        "BIBI_AGENT__QDRANT_REST_URL",
        "http://127.0.0.1:6337",
    )
    qdrant_collection: str = os.getenv(
        "BIBI_AGENT__QDRANT_COLLECTION",
        "bibi_work_memories",
    )
    database_url: str = os.getenv(
        "BIBI_AGENT__DATABASE_URL",
        os.getenv(
            "DATABASE_URL", "postgresql://postgres:password@127.0.0.1:5433/bibi_work"
        ),
    )
    celery_broker_url: str = os.getenv(
        "BIBI_AGENT__CELERY_BROKER_URL", "redis://127.0.0.1:6380/1"
    )
    celery_result_backend: str = os.getenv(
        "BIBI_AGENT__CELERY_RESULT_BACKEND", "redis://127.0.0.1:6380/2"
    )
    request_timeout_sec: float = float(
        os.getenv("BIBI_AGENT__REQUEST_TIMEOUT_SEC", "30")
    )
    otlp_enabled: bool = _env_bool("BIBI_AGENT__OTLP_ENABLED")
    otlp_endpoint: str = os.getenv(
        "BIBI_AGENT__OTLP_ENDPOINT", "http://127.0.0.1:4318"
    )
    telemetry_service_name: str = os.getenv(
        "BIBI_AGENT__TELEMETRY_SERVICE_NAME", "bibi-work-agent"
    )
    trace_sample_ratio: float = float(
        os.getenv("BIBI_AGENT__TRACE_SAMPLE_RATIO", "1.0")
    )
    telemetry_timeout_sec: float = float(
        os.getenv("BIBI_AGENT__TELEMETRY_TIMEOUT_SEC", "5")
    )
    port: int = int(os.getenv("BIBI_AGENT__PORT", "8371"))


settings = Settings()
