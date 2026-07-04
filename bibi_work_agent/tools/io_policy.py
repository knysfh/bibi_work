from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from typing import Any


DEFAULT_REDACT_FIELDS = (
    "authorization",
    "api_key",
    "access_token",
    "refresh_token",
    "bearer",
    "password",
    "secret",
    "token",
)
DEFAULT_MAX_OUTPUT_BYTES = 1_048_576
INPUT_SUMMARY_CHARS = 500
OUTPUT_SUMMARY_CHARS = 1_000
REDACTED = "[REDACTED]"


def summarize_input(value: Any, *, redact_fields: Sequence[str] | None = None) -> str:
    redacted = redact_value(value, redact_fields=redact_fields)
    text = stable_json(redacted)
    return truncate_text(text, max_bytes=INPUT_SUMMARY_CHARS)


def summarize_output(value: Any, obligations: Mapping[str, Any] | None) -> str:
    governed = apply_output_policy(value, obligations)
    text = governed if isinstance(governed, str) else stable_json(governed)
    return truncate_text(text, max_bytes=OUTPUT_SUMMARY_CHARS)


def apply_output_policy(value: Any, obligations: Mapping[str, Any] | None) -> Any:
    redact_fields = _redact_fields(obligations)
    max_output_bytes = _max_output_bytes(obligations)
    redacted = redact_value(value, redact_fields=redact_fields)

    if max_output_bytes is None:
        return redacted
    if max_output_bytes <= 0:
        return ""
    if isinstance(redacted, str):
        return truncate_text(redacted, max_bytes=max_output_bytes)
    if isinstance(redacted, bytes):
        return redacted[:max_output_bytes]

    encoded = stable_json(redacted).encode("utf-8")
    if len(encoded) <= max_output_bytes:
        return redacted

    preview = truncate_text(encoded.decode("utf-8"), max_bytes=max_output_bytes)
    return {
        "truncated": True,
        "original_bytes": len(encoded),
        "max_output_bytes": max_output_bytes,
        "preview": preview,
    }


def redact_value(value: Any, *, redact_fields: Sequence[str] | None = None) -> Any:
    fields = tuple(field.lower() for field in (redact_fields or DEFAULT_REDACT_FIELDS))
    if isinstance(value, Mapping):
        return {
            key: REDACTED
            if _is_sensitive_key(str(key), fields)
            else redact_value(item, redact_fields=fields)
            for key, item in value.items()
        }
    if isinstance(value, tuple):
        return tuple(redact_value(item, redact_fields=fields) for item in value)
    if isinstance(value, list):
        return [redact_value(item, redact_fields=fields) for item in value]
    if isinstance(value, str):
        return redact_inline_secret(value, redact_fields=fields)
    return value


def stable_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=True, sort_keys=True, default=str)


def truncate_text(text: str, *, max_bytes: int) -> str:
    encoded = text.encode("utf-8")
    if len(encoded) <= max_bytes:
        return text
    if max_bytes <= 20:
        return encoded[:max_bytes].decode("utf-8", errors="ignore")
    suffix = "...[truncated]"
    prefix_bytes = max_bytes - len(suffix.encode("utf-8"))
    return encoded[:prefix_bytes].decode("utf-8", errors="ignore") + suffix


def redact_inline_secret(text: str, *, redact_fields: Sequence[str]) -> str:
    redacted = text
    for field in redact_fields:
        for marker in (f"{field}=", f"{field}:", f'"{field}":'):
            lower = redacted.lower()
            start = lower.find(marker)
            while start != -1:
                value_start = start + len(marker)
                value_end = _secret_value_end(redacted, value_start)
                redacted = redacted[:value_start] + REDACTED + redacted[value_end:]
                lower = redacted.lower()
                start = lower.find(marker, value_start + len(REDACTED))
    return redacted


def _secret_value_end(text: str, start: int) -> int:
    while start < len(text) and text[start] in {" ", "'", '"'}:
        start += 1
    end = start
    while end < len(text) and text[end] not in {
        " ",
        "\n",
        "\r",
        "\t",
        ",",
        "}",
        "]",
        "'",
        '"',
    }:
        end += 1
    return end


def _is_sensitive_key(key: str, fields: Sequence[str]) -> bool:
    lowered = key.lower()
    return any(field in lowered for field in fields)


def _redact_fields(obligations: Mapping[str, Any] | None) -> Sequence[str]:
    fields = (obligations or {}).get("redact_fields")
    if isinstance(fields, list) and all(isinstance(field, str) for field in fields):
        return fields
    return DEFAULT_REDACT_FIELDS


def _max_output_bytes(obligations: Mapping[str, Any] | None) -> int | None:
    value = (obligations or {}).get("max_output_bytes", DEFAULT_MAX_OUTPUT_BYTES)
    if value is None:
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return DEFAULT_MAX_OUTPUT_BYTES
