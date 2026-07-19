from __future__ import annotations

import json
import hashlib
from collections.abc import Mapping, Sequence
from typing import Any, Protocol

from bibi_work_agent.runtime.cancellation import RunCancelled


MAX_PREVIEW_ROWS = 20
MAX_COLUMNS = 24
MAX_JSON_PREVIEW_BYTES = 16_384
MAX_MARKDOWN_CHARS = 4000
MAX_FILE_DIFF_CHARS = 24_000
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
    normalized_ui_hints = normalize_ui_hints(ui_hints)
    preferred_kind = _preferred_view_kind(normalized_ui_hints)

    if preferred_kind == "table" and isinstance(value, list) and _is_table_like(value):
        return _decorate_tool_result_views(
            [_table_view(value, artifact_writer=artifact_writer)], normalized_ui_hints
        )

    if preferred_kind == "map" and isinstance(value, Mapping):
        map_view = _map_view(value, artifact_writer=artifact_writer)
        if map_view is not None:
            return _decorate_tool_result_views([map_view], normalized_ui_hints)

    if preferred_kind == "chart" and isinstance(value, Mapping):
        chart = _chart_view(value, ui_hints=normalized_ui_hints)
        if chart is not None:
            return _decorate_tool_result_views([chart], normalized_ui_hints)

    if preferred_kind == "file_diff":
        file_diff = _file_diff_view(value)
        if file_diff is not None:
            return _decorate_tool_result_views([file_diff], normalized_ui_hints)

    if preferred_kind == "artifact":
        artifact = _artifact_view(
            value, artifact_writer=artifact_writer, ui_hints=normalized_ui_hints
        )
        if artifact is not None:
            return _decorate_tool_result_views([artifact], normalized_ui_hints)

    if preferred_kind == "json":
        return _decorate_tool_result_views(
            [_json_view(value, artifact_writer=artifact_writer)], normalized_ui_hints
        )

    if preferred_kind == "markdown" and isinstance(value, str) and value.strip():
        return _decorate_tool_result_views([_markdown_view(value)], normalized_ui_hints)

    if isinstance(value, list) and _is_table_like(value):
        return _decorate_tool_result_views(
            [_table_view(value, artifact_writer=artifact_writer)], normalized_ui_hints
        )

    if isinstance(value, Mapping):
        file_diff = _file_diff_view(value)
        if file_diff is not None:
            return _decorate_tool_result_views([file_diff], normalized_ui_hints)
        map_view = _map_view(value, artifact_writer=artifact_writer)
        if map_view is not None:
            return _decorate_tool_result_views([map_view], normalized_ui_hints)
        chart = _chart_view(value, ui_hints=normalized_ui_hints)
        if chart is not None:
            return _decorate_tool_result_views([chart], normalized_ui_hints)
        artifact = _artifact_view(
            value, artifact_writer=artifact_writer, ui_hints=normalized_ui_hints
        )
        if artifact is not None:
            return _decorate_tool_result_views([artifact], normalized_ui_hints)
        return _decorate_tool_result_views(
            [_json_view(value, artifact_writer=artifact_writer)], normalized_ui_hints
        )

    if isinstance(value, (list, tuple)):
        return _decorate_tool_result_views(
            [_json_view(list(value), artifact_writer=artifact_writer)],
            normalized_ui_hints,
        )

    if isinstance(value, str) and value.strip():
        return _decorate_tool_result_views([_markdown_view(value)], normalized_ui_hints)

    return []


def _preferred_view_kind(ui_hints: Mapping[str, Any] | None) -> str | None:
    ui_hints = normalize_ui_hints(ui_hints)
    if not ui_hints:
        return None
    value = ui_hints.get("view") or ui_hints.get("kind") or ui_hints.get("display")
    if isinstance(value, str) and value in {
        "table",
        "chart",
        "map",
        "json",
        "file_diff",
        "markdown",
        "artifact",
    }:
        return value
    for kind in ["table", "chart", "map", "json", "file_diff", "markdown", "artifact"]:
        if ui_hints.get(kind) is True:
            return kind
    return None


def normalize_ui_hints(value: Mapping[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(value, Mapping):
        return None

    for key in ["ui_hints", "x-ui-hints", "x_bibi_ui_hints"]:
        nested = value.get(key)
        if isinstance(nested, Mapping):
            return _with_display_metadata(normalize_ui_hints(nested), value)

    renderer = value.get("renderer")
    if isinstance(renderer, str):
        return _with_display_metadata(_view_hint(renderer), value)
    if isinstance(renderer, Mapping):
        return _with_display_metadata(normalize_ui_hints(renderer), value)

    view = value.get("view") or value.get("kind") or value.get("display")
    if isinstance(view, str):
        return _with_display_metadata(_view_hint(view), value)

    for key, kind in [
        ("table", "table"),
        ("chart", "chart"),
        ("graph", "chart"),
        ("map", "map"),
        ("geojson", "map"),
        ("json", "json"),
        ("file_diff", "file_diff"),
        ("diff", "file_diff"),
        ("markdown", "markdown"),
        ("artifact", "artifact"),
    ]:
        if value.get(key) is True:
            return _with_display_metadata({"view": kind}, value)
    return None


def _with_display_metadata(
    hints: dict[str, Any] | None, source: Mapping[str, Any]
) -> dict[str, Any] | None:
    if not hints:
        return None
    title = _first_string(source, "title", "name", "label")
    if not title or "title" in hints:
        return hints
    enriched = dict(hints)
    enriched["title"] = title[:240]
    return enriched


def _decorate_tool_result_views(
    views: list[dict[str, Any]], ui_hints: Mapping[str, Any] | None
) -> list[dict[str, Any]]:
    title = _first_string(ui_hints or {}, "title")
    if not title:
        return views
    decorated: list[dict[str, Any]] = []
    for view in views:
        if "title" in view:
            decorated.append(view)
            continue
        item = dict(view)
        item["title"] = title
        decorated.append(item)
    return decorated


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
        "file_diff": "file_diff",
        "filediff": "file_diff",
        "diff": "file_diff",
        "patch": "file_diff",
        "markdown": "markdown",
        "md": "markdown",
        "artifact": "artifact",
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


def _markdown_view(value: str) -> dict[str, Any]:
    return {
        "kind": "markdown",
        "text": value[:MAX_MARKDOWN_CHARS],
    }


def _file_diff_view(value: Any) -> dict[str, Any] | None:
    files = _file_diff_files(value)
    if not files:
        return None
    return {
        "kind": "file_diff",
        "files": files,
    }


def _file_diff_files(value: Any) -> list[dict[str, Any]]:
    raw_files: list[Any]
    if isinstance(value, Mapping) and isinstance(value.get("files"), list):
        raw_files = list(value["files"])
    elif isinstance(value, list):
        raw_files = value
    else:
        raw_files = [value]

    files: list[dict[str, Any]] = []
    for raw_file in raw_files:
        if not isinstance(raw_file, Mapping):
            continue
        diff = (
            raw_file.get("file_diff")
            or raw_file.get("diff")
            or raw_file.get("patch")
        )
        if not isinstance(diff, str) or not diff:
            continue
        path = _first_string(
            raw_file,
            "path",
            "file_path",
            "full_path",
            "relative_path",
        )
        file_name = _first_string(raw_file, "file_name", "name")
        if not file_name and path:
            file_name = path.rstrip("/").rsplit("/", 1)[-1]
        item: dict[str, Any] = {
            "file_diff": diff[:MAX_FILE_DIFF_CHARS],
            "file_name": file_name or "changes.diff",
        }
        if path:
            item["path"] = path
        operation = _first_string(raw_file, "operation", "change_type", "status")
        if operation:
            item["operation"] = operation
        if len(diff) > MAX_FILE_DIFF_CHARS:
            item["truncated"] = True
        files.append(item)
    return files


def _artifact_view(
    value: Any,
    *,
    artifact_writer: ArtifactWriter | None,
    ui_hints: Mapping[str, Any] | None,
) -> dict[str, Any] | None:
    artifact_ref = _artifact_ref_from_value(value)
    if artifact_ref is None:
        artifact_ref = _write_artifact_ref(value, artifact_writer=artifact_writer)
    if artifact_ref is None:
        return None
    view: dict[str, Any] = {
        "kind": "artifact",
        "artifact_ref": artifact_ref,
    }
    title = _first_string(ui_hints or {}, "title", "name", "label")
    if title:
        view["title"] = title
    return view


def _artifact_ref_from_value(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, Mapping):
        return None
    raw_ref = value.get("artifact_ref") or value.get("data_ref")
    if not isinstance(raw_ref, Mapping):
        raw_ref = value
    required = ["artifact_id", "content_type", "content_hash", "size_bytes"]
    if not all(key in raw_ref for key in required):
        return None
    artifact_ref = {
        "artifact_id": str(raw_ref["artifact_id"]),
        "content_type": str(raw_ref["content_type"]),
        "content_hash": str(raw_ref["content_hash"]),
        "size_bytes": int(raw_ref["size_bytes"]),
    }
    object_reference_id = raw_ref.get("object_reference_id")
    if object_reference_id:
        artifact_ref["object_reference_id"] = str(object_reference_id)
    return artifact_ref


def _write_artifact_ref(
    value: Any, *, artifact_writer: ArtifactWriter | None
) -> dict[str, Any] | None:
    if artifact_writer is None:
        return None
    if isinstance(value, str):
        content = value
        content_type = "text/markdown; charset=utf-8"
        suffix = "artifact.md"
    else:
        content = json.dumps(
            _json_safe_value(value), ensure_ascii=True, sort_keys=True, default=str
        )
        content_type = "application/json"
        suffix = "artifact.json"
    try:
        return artifact_writer(content=content, content_type=content_type, suffix=suffix)
    except RunCancelled:
        raise
    except Exception:
        return None


def _first_string(value: Mapping[str, Any], *keys: str) -> str | None:
    for key in keys:
        item = value.get(key)
        if isinstance(item, str) and item.strip():
            return item.strip()
    return None


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
    except RunCancelled:
        raise
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
    except RunCancelled:
        raise
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
