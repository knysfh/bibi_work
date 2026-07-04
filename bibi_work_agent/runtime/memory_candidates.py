from __future__ import annotations

import json
import re
from collections.abc import Iterable, Mapping
from typing import Any


MAX_CANDIDATES = 12
MAX_CANDIDATE_CHARS = 1000
MAX_EXTRACTION_DEPTH = 8
MAX_ITERABLE_ITEMS = 100
DEFAULT_CONFIDENCE = 0.55
ALLOWED_LAYERS = {"core_profile", "episodic", "semantic", "procedural"}
ALLOWED_VISIBILITIES = {"private", "project", "tenant", "public"}
ALLOWED_SENSITIVITIES = {"normal", "sensitive"}

_SECRET_PATTERNS = (
    re.compile(r"(?i)(authorization\s*[:=]\s*bearer\s+)[^\s,;]+"),
    re.compile(r"(?i)\b(api[_-]?key|token|secret|password)\s*[:=]\s*([^\s,;]+)"),
    re.compile(r"(?i)\b(sk-[A-Za-z0-9_-]{12,}|xox[baprs]-[A-Za-z0-9-]{12,})\b"),
)
_MEMORY_HEADER_RE = re.compile(
    r"(?i)^\s*(memory candidates?|candidate memories|memories to remember)\s*:?\s*$"
)
_BULLET_RE = re.compile(r"^\s*(?:[-*]|\d+[.)])\s+(?P<content>.+?)\s*$")


class MemoryCandidateCollector:
    def __init__(self) -> None:
        self._candidates: list[dict[str, Any]] = []
        self._seen_content: set[str] = set()

    def observe(self, value: Any) -> None:
        for candidate in extract_memory_candidates(value):
            self.add(candidate)

    def add(self, candidate: Mapping[str, Any]) -> None:
        if len(self._candidates) >= MAX_CANDIDATES:
            return
        normalized = normalize_candidate(candidate)
        if normalized is None:
            return
        dedupe_key = normalized["content"].casefold()
        if dedupe_key in self._seen_content:
            return
        self._seen_content.add(dedupe_key)
        self._candidates.append(normalized)

    def candidates(self) -> list[dict[str, Any]]:
        return list(self._candidates)

    def is_full(self) -> bool:
        return len(self._candidates) >= MAX_CANDIDATES


def extract_memory_candidates(value: Any) -> list[dict[str, Any]]:
    collector = MemoryCandidateCollector()
    _extract_into(value, collector, depth=MAX_EXTRACTION_DEPTH)
    return collector.candidates()


def _extract_into(
    value: Any, collector: MemoryCandidateCollector, *, depth: int
) -> None:
    if depth <= 0 or collector.is_full():
        return
    if isinstance(value, str):
        _extract_from_text(value, collector)
        return
    if isinstance(value, Mapping):
        for candidate in _candidate_arrays(value):
            collector.add(candidate)

        payload = value.get("payload")
        if payload is not None and payload is not value:
            _extract_into(payload, collector, depth=depth - 1)

        result = value.get("result")
        if result is not None:
            _extract_into(result, collector, depth=depth - 1)
        return
    if isinstance(value, Iterable) and not isinstance(value, (bytes, bytearray)):
        for index, item in enumerate(value):
            if index >= MAX_ITERABLE_ITEMS or collector.is_full():
                break
            _extract_into(item, collector, depth=depth - 1)


def _candidate_arrays(value: Mapping[str, Any]) -> Iterable[Any]:
    candidates = value.get("memory_candidates") or value.get("candidate_memories")
    if isinstance(candidates, list):
        yield from candidates

    memory = value.get("memory")
    if isinstance(memory, Mapping):
        nested_candidates = memory.get("candidates")
        if isinstance(nested_candidates, list):
            yield from nested_candidates


def _extract_from_text(text: str, collector: MemoryCandidateCollector) -> None:
    parsed = _try_parse_json(text)
    if parsed is not None:
        _extract_into(parsed, collector, depth=MAX_EXTRACTION_DEPTH)
        return

    in_section = False
    for line in text.splitlines():
        if _MEMORY_HEADER_RE.match(line):
            in_section = True
            continue
        if not in_section:
            continue
        if not line.strip():
            break
        match = _BULLET_RE.match(line)
        if match:
            collector.add(
                {"content": match.group("content"), "confidence": DEFAULT_CONFIDENCE}
            )


def _try_parse_json(text: str) -> Any | None:
    stripped = text.strip()
    if not stripped or stripped[0] not in "[{":
        return None
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        return None


def normalize_candidate(candidate: Any) -> dict[str, Any] | None:
    if isinstance(candidate, str):
        raw: Mapping[str, Any] = {"content": candidate}
    elif isinstance(candidate, Mapping):
        raw = candidate
    else:
        return None

    content = _normalized_content(raw.get("content") or raw.get("text"))
    if content is None or _contains_secret(content):
        return None

    normalized: dict[str, Any] = {"content": content}

    layer = _normalized_choice(raw.get("layer"), ALLOWED_LAYERS)
    if layer:
        normalized["layer"] = layer

    confidence = _normalized_confidence(raw.get("confidence"))
    if confidence is not None:
        normalized["confidence"] = confidence

    visibility = _normalized_choice(raw.get("visibility"), ALLOWED_VISIBILITIES)
    if visibility:
        normalized["visibility"] = visibility

    sensitivity = _normalized_choice(raw.get("sensitivity"), ALLOWED_SENSITIVITIES)
    if sensitivity:
        normalized["sensitivity"] = sensitivity

    retention_policy = _normalized_short_text(raw.get("retention_policy"), limit=128)
    if retention_policy:
        normalized["retention_policy"] = retention_policy

    return normalized


def _normalized_content(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    content = " ".join(value.strip().split())
    if len(content) < 8:
        return None
    return content[:MAX_CANDIDATE_CHARS]


def _normalized_short_text(value: Any, *, limit: int) -> str | None:
    if not isinstance(value, str):
        return None
    text = value.strip()
    if not text:
        return None
    return text[:limit]


def _normalized_choice(value: Any, allowed: set[str]) -> str | None:
    if not isinstance(value, str):
        return None
    normalized = value.strip().lower()
    return normalized if normalized in allowed else None


def _normalized_confidence(value: Any) -> float | None:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return None
    return max(0.0, min(float(value), 1.0))


def _contains_secret(text: str) -> bool:
    return any(pattern.search(text) for pattern in _SECRET_PATTERNS)
