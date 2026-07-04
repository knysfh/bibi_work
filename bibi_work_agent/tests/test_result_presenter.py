from __future__ import annotations

from bibi_work_agent.tools.result_presenter import (
    build_tool_result_views,
    normalize_ui_hints,
)


def test_build_tool_result_views_for_table_like_rows() -> None:
    views = build_tool_result_views(
        [
            {"name": "alice", "score": 3},
            {"name": "bob", "score": 4},
        ]
    )

    assert views == [
        {
            "kind": "table",
            "columns": [
                {"key": "name", "label": "name", "type": "string"},
                {"key": "score", "label": "score", "type": "number"},
            ],
            "rows_preview": [
                {"name": "alice", "score": 3},
                {"name": "bob", "score": 4},
            ],
        }
    ]


def test_build_tool_result_views_for_vega_lite_spec() -> None:
    views = build_tool_result_views(
        {
            "vega_lite_spec": {
                "mark": "bar",
                "encoding": {"x": {"field": "name"}, "y": {"field": "score"}},
            }
        }
    )

    assert views[0]["kind"] == "chart"
    assert views[0]["spec_kind"] == "vega_lite"


def test_build_tool_result_views_uses_chart_ui_hints() -> None:
    views = build_tool_result_views(
        {
            "spec": {
                "mark": "line",
                "encoding": {"x": {"field": "day"}, "y": {"field": "count"}},
            }
        },
        ui_hints={"view": "chart"},
    )

    assert views[0]["kind"] == "chart"
    assert views[0]["spec_kind"] == "vega_lite"


def test_normalize_ui_hints_accepts_renderer_aliases() -> None:
    assert normalize_ui_hints({"renderer": {"kind": "vega-lite"}}) == {"view": "chart"}
    assert normalize_ui_hints({"x-ui-hints": {"view": "data-grid"}}) == {
        "view": "table"
    }


def test_build_tool_result_views_falls_back_to_json() -> None:
    views = build_tool_result_views({"status": "ok", "count": 2})

    assert views == [{"kind": "json", "value_preview": {"status": "ok", "count": 2}}]


def test_build_tool_result_views_writes_large_table_artifact() -> None:
    writes: list[dict] = []

    def writer(**payload: str) -> dict:
        writes.append(payload)
        return {
            "artifact_id": "artifact-1",
            "content_type": payload["content_type"],
            "content_hash": "sha256:abc",
            "size_bytes": len(payload["content"]),
        }

    views = build_tool_result_views(
        [{"name": f"user-{index}"} for index in range(25)],
        artifact_writer=writer,
    )

    assert views[0]["kind"] == "table"
    assert views[0]["data_ref"]["artifact_id"] == "artifact-1"
    assert writes[0]["content_type"] == "application/x-ndjson"
    assert writes[0]["suffix"] == "table.jsonl"
    assert writes[0]["content"].splitlines()[0] == '{"name": "user-0"}'


def test_build_tool_result_views_maps_geojson_only_with_artifact_writer() -> None:
    geojson = {"type": "FeatureCollection", "features": []}

    assert build_tool_result_views(geojson) == [
        {"kind": "json", "value_preview": geojson}
    ]
    assert build_tool_result_views(
        geojson,
        artifact_writer=lambda **_: {
            "artifact_id": "map-1",
            "content_type": "application/geo+json",
            "content_hash": "sha256:abc",
            "size_bytes": 10,
        },
    ) == [
        {
            "kind": "map",
            "format": "geojson",
            "data_ref": {
                "artifact_id": "map-1",
                "content_type": "application/geo+json",
                "content_hash": "sha256:abc",
                "size_bytes": 10,
            },
            "data_preview": geojson,
        }
    ]
