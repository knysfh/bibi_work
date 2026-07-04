from __future__ import annotations

import json
import hashlib
from collections.abc import Mapping, Sequence
from typing import Any, Protocol


MAX_PREVIEW_ROWS = 20
MAX_COLUMNS = 24
MAX_JSON_PREVIEW_BYTES = 16_384
MAX_MARKDOWN_CHARS = 4000
ARTIFACT_THRESHOLD_BYTES = 4096


class ArtifactWriter(Protocol):
    def __call__(
        self, *, content: str, content_type: str, suffix: str
    ) -> dict[str, Any] | None: ...


def build_tool_result_views(
    value: Any,
    *,
    artifact_writer: ArtifactWriter | None = None,
    ui_hints: Mapping[str, Any] | None = None,
) -> list[dict[str, Any]]:
    preferred_kind = _preferred_view_kind(ui_hints)

    if preferred_kind == "table" and isinstance(value, list) and _is_table_like(value):
        return [_table_view(value, artifact_writer=artifact_writer)]

    if preferred_kind == "map" and isinstance(value, Mapping):
        map_view = _map_view(value, artifact_writer=artifact_writer)
        if map_view is not None:
            return [map_view]

    if preferred_kind == "chart" and isinstance(value, Mapping):
        chart = _chart_view(value, ui_hints=ui_hints)
        if chart is not None:
            return [chart]

    if preferred_kind == "json":
        return [_json_view(value, artifact_writer=artifact_writer)]

    if isinstance(value, list) and _is_table_like(value):
        return [_table_view(value, artifact_writer=artifact_writer)]

    if isinstance(value, Mapping):
        map_view = _map_view(value, artifact_writer=artifact_writer)
        if map_view is not None:
            return [map_view]
        chart = _chart_view(value, ui_hints=ui_hints)
        if chart is not None:
            return [chart]
        return [_json_view(value, artifact_writer=artifact_writer)]

    if isinstance(value, (list, tuple)):
        return [_json_view(list(value), artifact_writer=artifact_writer)]

    if isinstance(value, str) and value.strip():
        return [
            {
                "kind": "markdown",
                "text": value[:MAX_MARKDOWN_CHARS],
            }
        ]

    return []


def _preferred_view_kind(ui_hints: Mapping[str, Any] | None) -> str | None:
    ui_hints = normalize_ui_hints(ui_hints)
    if not ui_hints:
        return None
    value = ui_hints.get("view") or ui_hints.get("kind") or ui_hints.get("display")
    if isinstance(value, str) and value in {"table", "chart", "map", "json"}:
        return value
    for kind in ["table", "chart", "map", "json"]:
        if ui_hints.get(kind) is True:
            return kind
    return None


def normalize_ui_hints(value: Mapping[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(value, Mapping):
        return None

    for key in ["ui_hints", "x-ui-hints", "x_bibi_ui_hints"]:
        nested = value.get(key)
        if isinstance(nested, Mapping):
            return normalize_ui_hints(nested)

    renderer = value.get("renderer")
    if isinstance(renderer, str):
        return _view_hint(renderer)
    if isinstance(renderer, Mapping):
        return normalize_ui_hints(renderer)

    view = value.get("view") or value.get("kind") or value.get("display")
    if isinstance(view, str):
        return _view_hint(view)

    for key, kind in [
        ("table", "table"),
        ("chart", "chart"),
        ("graph", "chart"),
        ("map", "map"),
        ("geojson", "map"),
        ("json", "json"),
    ]:
        if value.get(key) is True:
            return {"view": kind}
    return None


def _view_hint(value: str) -> dict[str, str] | None:
    normalized = value.strip().replace("-", "_").lower()
    aliases = {
        "table": "table",
        "grid": "table",
        "data_grid": "table",
        "chart": "chart",
        "vega_lite": "chart",
        "vegalite": "chart",
        "graph": "chart",
        "map": "map",
        "geojson": "map",
        "json": "json",
    }
    kind = aliases.get(normalized)
    return {"view": kind} if kind else None


def _is_table_like(value: Sequence[Any]) -> bool:
    if not value:
        return False
    return all(isinstance(item, Mapping) for item in value[:MAX_PREVIEW_ROWS])


def _table_view(
    rows: Sequence[Mapping[str, Any]], *, artifact_writer: ArtifactWriter | None
) -> dict[str, Any]:
    columns = _columns_from_rows(rows)
    view: dict[str, Any] = {
        "kind": "table",
        "columns": columns,
        "rows_preview": [_project_row(row, columns) for row in rows[:MAX_PREVIEW_ROWS]],
    }
    data_ref = _artifact_for_table_rows(
        rows,
        artifact_writer=artifact_writer,
        force=len(rows) > MAX_PREVIEW_ROWS,
    )
    if data_ref:
        view["data_ref"] = data_ref
    return view


def _columns_from_rows(rows: Sequence[Mapping[str, Any]]) -> list[dict[str, str]]:
    keys: list[str] = []
    for row in rows[:MAX_PREVIEW_ROWS]:
        for key in row:
            text_key = str(key)
            if text_key not in keys:
                keys.append(text_key)
            if len(keys) >= MAX_COLUMNS:
                break
        if len(keys) >= MAX_COLUMNS:
            break
    return [{"key": key, "label": key, "type": _column_type(rows, key)} for key in keys]


def _column_type(rows: Sequence[Mapping[str, Any]], key: str) -> str:
    values = [
        row.get(key) for row in rows[:MAX_PREVIEW_ROWS] if row.get(key) is not None
    ]
    if values and all(isinstance(value, bool) for value in values):
        return "boolean"
    if values and all(
        isinstance(value, (int, float)) and not isinstance(value, bool)
        for value in values
    ):
        return "number"
    return "string"


def _project_row(
    row: Mapping[str, Any], columns: Sequence[Mapping[str, str]]
) -> dict[str, Any]:
    return {
        column["key"]: _json_safe_value(row.get(column["key"])) for column in columns
    }


def _chart_view(
    value: Mapping[str, Any], *, ui_hints: Mapping[str, Any] | None
) -> dict[str, Any] | None:
    spec = value.get("vega_lite_spec")
    if not isinstance(spec, Mapping):
        spec = value.get("vegaLiteSpec")
    if not isinstance(spec, Mapping) and _preferred_view_kind(ui_hints) == "chart":
        spec = value.get("spec")
    if not isinstance(spec, Mapping):
        return None
    return {
        "kind": "chart",
        "spec_kind": "vega_lite",
        "spec": _json_safe_value(spec),
    }


def _map_view(
    value: Mapping[str, Any], *, artifact_writer: ArtifactWriter | None
) -> dict[str, Any] | None:
    if value.get("type") not in {"FeatureCollection", "Feature"}:
        return None
    data_ref = _artifact_for_value(
        value,
        artifact_writer=artifact_writer,
        content_type="application/geo+json",
        suffix="map.geojson",
        force=True,
    )
    if not data_ref:
        return None
    view: dict[str, Any] = {"kind": "map", "format": "geojson", "data_ref": data_ref}
    encoded = json.dumps(value, ensure_ascii=True, sort_keys=True, default=str).encode(
        "utf-8"
    )
    if len(encoded) <= MAX_JSON_PREVIEW_BYTES:
        view["data_preview"] = _json_safe_value(value)
    return view


def _json_view(value: Any, *, artifact_writer: ArtifactWriter | None) -> dict[str, Any]:
    view: dict[str, Any] = {
        "kind": "json",
        "value_preview": _truncate_json_preview(_json_safe_value(value)),
    }
    data_ref = _artifact_for_value(
        value,
        artifact_writer=artifact_writer,
        content_type="application/json",
        suffix="data.json",
    )
    if data_ref:
        view["data_ref"] = data_ref
    return view


def _artifact_for_value(
    value: Any,
    *,
    artifact_writer: ArtifactWriter | None,
    content_type: str,
    suffix: str,
    force: bool = False,
) -> dict[str, Any] | None:
    if artifact_writer is None:
        return None
    content = json.dumps(
        _json_safe_value(value), ensure_ascii=True, sort_keys=True, default=str
    )
    if not force and len(content.encode("utf-8")) <= ARTIFACT_THRESHOLD_BYTES:
        return None
    try:
        return artifact_writer(
            content=content, content_type=content_type, suffix=suffix
        )
    except Exception:
        return None


def _artifact_for_table_rows(
    rows: Sequence[Mapping[str, Any]],
    *,
    artifact_writer: ArtifactWriter | None,
    force: bool = False,
) -> dict[str, Any] | None:
    if artifact_writer is None:
        return None
    safe_rows = [_json_safe_value(row) for row in rows]
    content = "\n".join(
        json.dumps(row, ensure_ascii=True, sort_keys=True, default=str)
        for row in safe_rows
    )
    if not force and len(content.encode("utf-8")) <= ARTIFACT_THRESHOLD_BYTES:
        return None
    try:
        return artifact_writer(
            content=content,
            content_type="application/x-ndjson",
            suffix="table.jsonl",
        )
    except Exception:
        return None


def _truncate_json_preview(value: Any) -> Any:
    encoded = json.dumps(value, ensure_ascii=True, sort_keys=True, default=str).encode(
        "utf-8"
    )
    if len(encoded) <= MAX_JSON_PREVIEW_BYTES:
        return value
    return {
        "truncated": True,
        "original_bytes": len(encoded),
        "content_hash": f"sha256:{hashlib.sha256(encoded).hexdigest()}",
        "preview": encoded[:MAX_JSON_PREVIEW_BYTES].decode("utf-8", errors="ignore"),
    }


def _json_safe_value(value: Any) -> Any:
    if isinstance(value, Mapping):
        return {str(key): _json_safe_value(item) for key, item in value.items()}
    if isinstance(value, tuple):
        return [_json_safe_value(item) for item in value]
    if isinstance(value, list):
        return [_json_safe_value(item) for item in value]
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    return str(value)
