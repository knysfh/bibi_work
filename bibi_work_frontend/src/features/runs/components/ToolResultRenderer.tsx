import { useEffect, useMemo, useRef, useState } from "react";
import {
  flexRender,
  getCoreRowModel,
  getPaginationRowModel,
  useReactTable,
  type ColumnDef
} from "@tanstack/react-table";
import { useMeQuery } from "../../auth/api/auth.queries";
import { useToolResultArtifactQuery } from "../../projects/api/project.queries";
import type { JsonRecord, JsonValue } from "../../../shared/types/json";
import { useI18n, type I18nKey } from "../../../shared/i18n";
import type {
  ArtifactToolResultView,
  ChartToolResultView,
  FileDiffToolResultView,
  JsonToolResultView,
  MapToolResultView,
  TableToolResultView,
  ToolResultArtifactRef,
  ToolResultView
} from "../domain/tool-result-view.types";
import { MessageContentRenderer } from "./MessageContentRenderer";

const ARTIFACT_TABLE_PAGE_SIZE = 500;
const VIRTUAL_ROW_HEIGHT = 34;
const VIRTUAL_VIEWPORT_HEIGHT = 360;
const VIRTUAL_OVERSCAN_ROWS = 6;

export function ToolResultRenderer({ views }: { views: ToolResultView[] }) {
  const { t } = useI18n();
  if (!views.length) {
    return null;
  }

  return (
    <div className="tool-result-stack" aria-label={t("run.toolResults")}>
      {views.map((view, index) => (
        <ToolResultViewRenderer key={`${view.kind}.${index}`} view={view} />
      ))}
    </div>
  );
}

function ToolResultViewRenderer({ view }: { view: ToolResultView }) {
  switch (view.kind) {
    case "table":
      return <TableResultView view={view} />;
    case "chart":
      return <ChartResultView view={view} />;
    case "map":
      return <MapResultView view={view} />;
    case "json":
      return <JsonResultView view={view} />;
    case "file_diff":
      return <FileDiffResultView view={view} />;
    case "markdown":
      return (
        <section className="tool-result-view">
          <ToolResultTitle title={view.title} fallbackKey="run.toolResult.markdown" />
          <MessageContentRenderer content={view.text} />
        </section>
      );
    case "artifact":
      return <ArtifactResultView view={view} />;
  }
}

function TableResultView({ view }: { view: TableToolResultView }) {
  const { t } = useI18n();
  const [artifactLoaded, setArtifactLoaded] = useState(false);
  const [artifactPage, setArtifactPage] = useState(0);
  const [tableScrollTop, setTableScrollTop] = useState(0);
  const tableWrapRef = useRef<HTMLDivElement | null>(null);
  const artifactRows = useArtifactRows(
    view.dataRef,
    artifactPage * ARTIFACT_TABLE_PAGE_SIZE,
    ARTIFACT_TABLE_PAGE_SIZE,
    artifactLoaded
  );
  const rows = artifactRows.rows ?? view.rowsPreview;
  const columns = useMemo<ColumnDef<JsonRecord>[]>(
    () =>
      view.columns.map((column) => ({
        id: column.key,
        accessorFn: (row) => row[column.key],
        header: column.label || column.key,
        cell: (info) => formatCell(info.getValue() as JsonValue | undefined)
      })),
    [view.columns]
  );
  const table = useReactTable({
    data: rows,
    columns,
    getCoreRowModel: getCoreRowModel(),
    getPaginationRowModel: getPaginationRowModel(),
    initialState: { pagination: { pageSize: 10 } }
  });
  const tableRows = artifactLoaded
    ? table.getPrePaginationRowModel().rows
    : table.getRowModel().rows;
  const virtualRows = virtualRowWindow(tableRows.length, tableScrollTop, artifactLoaded);
  const visibleRows = virtualRows.enabled
    ? tableRows.slice(virtualRows.startIndex, virtualRows.endIndex)
    : tableRows;

  useEffect(() => {
    setTableScrollTop(0);
    if (tableWrapRef.current) {
      tableWrapRef.current.scrollTop = 0;
    }
  }, [artifactLoaded, artifactPage]);

  return (
    <section className="tool-result-view">
      <ToolResultTitle title={view.title} fallbackKey="run.toolResult.table" />
      <div
        className="tool-result-table-wrap"
        data-virtualized={virtualRows.enabled ? "true" : undefined}
        onScroll={(event) => setTableScrollTop(event.currentTarget.scrollTop)}
        ref={tableWrapRef}
      >
        <table className="tool-result-table">
          <thead>
            {table.getHeaderGroups().map((headerGroup) => (
              <tr key={headerGroup.id}>
                {headerGroup.headers.map((header) => (
                  <th key={header.id}>
                    {header.isPlaceholder
                      ? null
                      : flexRender(header.column.columnDef.header, header.getContext())}
                  </th>
                ))}
              </tr>
            ))}
          </thead>
          <tbody>
            {virtualRows.enabled && virtualRows.topSpacerHeight > 0 ? (
              <tr aria-hidden="true">
                <td colSpan={view.columns.length} style={{ height: virtualRows.topSpacerHeight }} />
              </tr>
            ) : null}
            {visibleRows.map((row) => (
              <tr key={row.id}>
                {row.getVisibleCells().map((cell) => (
                  <td key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</td>
                ))}
              </tr>
            ))}
            {virtualRows.enabled && virtualRows.bottomSpacerHeight > 0 ? (
              <tr aria-hidden="true">
                <td
                  colSpan={view.columns.length}
                  style={{ height: virtualRows.bottomSpacerHeight }}
                />
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      {!artifactLoaded && view.rowsPreview.length > 10 ? (
        <div className="tool-result-pagination">
          <button
            type="button"
            onClick={() => table.previousPage()}
            disabled={!table.getCanPreviousPage()}
          >
            {t("run.toolResult.previousPage")}
          </button>
          <span>
            {table.getState().pagination.pageIndex + 1} / {table.getPageCount()}
          </span>
          <button type="button" onClick={() => table.nextPage()} disabled={!table.getCanNextPage()}>
            {t("run.toolResult.nextPage")}
          </button>
        </div>
      ) : null}
      {view.dataRef?.objectReferenceId ? (
        <div className="tool-result-actions">
          {!artifactLoaded ? (
            <button type="button" onClick={() => setArtifactLoaded(true)}>
              {t("run.toolResult.loadArtifact")}
            </button>
          ) : (
            <>
              <button
                type="button"
                onClick={() => setArtifactPage((current) => Math.max(0, current - 1))}
                disabled={artifactPage === 0 || artifactRows.isLoading}
              >
                {t("run.toolResult.previousPage")}
              </button>
              <span>
                {artifactPage + 1}
                {artifactRows.totalPages ? ` / ${artifactRows.totalPages}` : ""}
              </span>
              <button
                type="button"
                onClick={() => setArtifactPage((current) => current + 1)}
                disabled={!artifactRows.hasNextPage || artifactRows.isLoading}
              >
                {t("run.toolResult.nextPage")}
              </button>
            </>
          )}
        </div>
      ) : null}
      {artifactRows.error ? (
        <p className="tool-result-muted">{t("run.toolResult.artifactLoadError")}</p>
      ) : null}
      {view.dataRef ? <ArtifactRefSummary refValue={view.dataRef} /> : null}
      {rows.length === 0 ? (
        <p className="tool-result-muted">{t("run.toolResult.emptyTable")}</p>
      ) : null}
    </section>
  );
}

function ChartResultView({ view }: { view: ChartToolResultView }) {
  const { t } = useI18n();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    let cancelled = false;
    let chartView: { finalize?: () => void } | undefined;
    setError(null);

    void import("vega-embed")
      .then(({ default: embed }) =>
        embed(container, view.spec, { actions: false, renderer: "canvas" })
      )
      .then((result) => {
        if (cancelled) {
          result.view.finalize();
          return;
        }
        chartView = result.view;
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      });

    return () => {
      cancelled = true;
      chartView?.finalize?.();
      container.replaceChildren();
    };
  }, [view.spec]);

  return (
    <section className="tool-result-view">
      <ToolResultTitle title={view.title} fallbackKey="run.toolResult.chart" />
      <div className="tool-result-chart" ref={containerRef} />
      {error ? (
        <>
          <p className="tool-result-muted">{t("run.toolResult.chartError")}</p>
          <JsonPreview value={{ error, spec_kind: view.specKind, spec: view.spec }} />
        </>
      ) : null}
      {view.dataRef ? <ArtifactRefSummary refValue={view.dataRef} /> : null}
    </section>
  );
}

function MapResultView({ view }: { view: MapToolResultView }) {
  const { t } = useI18n();
  const [artifactLoaded, setArtifactLoaded] = useState(false);
  const artifactValue = useArtifactValue(view.dataRef, artifactLoaded);
  const mapData = view.dataPreview ?? artifactValue.valueAsRecord;

  return (
    <section className="tool-result-view">
      <ToolResultTitle title={view.title} fallbackKey="run.toolResult.map" />
      {mapData ? <MapCanvas data={mapData} /> : null}
      {!mapData ? (
        <div className="tool-result-actions">
          <button type="button" onClick={() => setArtifactLoaded(true)}>
            {t("run.toolResult.loadArtifact")}
          </button>
        </div>
      ) : null}
      {artifactValue.error ? (
        <p className="tool-result-muted">{t("run.toolResult.artifactLoadError")}</p>
      ) : null}
      <ArtifactRefSummary refValue={view.dataRef} />
      {view.styleRef ? <code className="tool-result-inline">{view.styleRef}</code> : null}
    </section>
  );
}

function MapCanvas({ data }: { data: JsonRecord }) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    let cancelled = false;
    let map: { remove?: () => void } | undefined;
    setError(null);

    void import("maplibre-gl")
      .then(({ default: maplibregl }) => {
        if (cancelled) {
          return;
        }
        map = new maplibregl.Map({
          container,
          attributionControl: false,
          interactive: true,
          center: [0, 0],
          zoom: 1,
          style: {
            version: 8,
            sources: {
              result: {
                type: "geojson",
                data: data as unknown as GeoJSON.GeoJSON
              }
            },
            layers: [
              { id: "background", type: "background", paint: { "background-color": "#f8fafc" } },
              {
                id: "result-fill",
                type: "fill",
                source: "result",
                paint: { "fill-color": "#2f6f6b", "fill-opacity": 0.22 },
                filter: ["==", ["geometry-type"], "Polygon"]
              },
              {
                id: "result-line",
                type: "line",
                source: "result",
                paint: { "line-color": "#2f6f6b", "line-width": 2 }
              },
              {
                id: "result-points",
                type: "circle",
                source: "result",
                paint: { "circle-color": "#b7433d", "circle-radius": 5 }
              }
            ]
          }
        });
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      });

    return () => {
      cancelled = true;
      map?.remove?.();
      container.replaceChildren();
    };
  }, [data]);

  return (
    <>
      <div className="tool-result-map" ref={containerRef} />
      {error ? <p className="tool-result-muted">{error}</p> : null}
    </>
  );
}

function JsonResultView({ view }: { view: JsonToolResultView }) {
  const { t } = useI18n();
  const [artifactLoaded, setArtifactLoaded] = useState(false);
  const artifactValue = useArtifactValue(view.dataRef, artifactLoaded);
  return (
    <section className="tool-result-view">
      <ToolResultTitle title={view.title} fallbackKey="run.toolResult.json" />
      <JsonPreview value={artifactValue.value ?? view.valuePreview} />
      {view.dataRef?.objectReferenceId ? (
        <div className="tool-result-actions">
          <button type="button" onClick={() => setArtifactLoaded(true)}>
            {t("run.toolResult.loadArtifact")}
          </button>
        </div>
      ) : null}
      {artifactValue.error ? (
        <p className="tool-result-muted">{t("run.toolResult.artifactLoadError")}</p>
      ) : null}
      {view.dataRef ? <ArtifactRefSummary refValue={view.dataRef} /> : null}
    </section>
  );
}

function FileDiffResultView({ view }: { view: FileDiffToolResultView }) {
  return (
    <section className="tool-result-view">
      <ToolResultTitle title={view.title} fallbackKey="run.toolResult.fileDiff" />
      <JsonPreview value={view.files} />
    </section>
  );
}

function ArtifactResultView({ view }: { view: ArtifactToolResultView }) {
  const { t } = useI18n();
  const [artifactLoaded, setArtifactLoaded] = useState(false);
  const artifactValue = useArtifactValue(view.artifactRef, artifactLoaded);
  return (
    <section className="tool-result-view">
      <ToolResultTitle title={view.title} fallbackKey="run.toolResult.artifact" />
      {artifactValue.value ? <JsonPreview value={artifactValue.value} /> : null}
      {view.artifactRef.objectReferenceId ? (
        <div className="tool-result-actions">
          <button type="button" onClick={() => setArtifactLoaded(true)}>
            {t("run.toolResult.loadArtifact")}
          </button>
        </div>
      ) : null}
      {artifactValue.error ? (
        <p className="tool-result-muted">{t("run.toolResult.artifactLoadError")}</p>
      ) : null}
      <ArtifactRefSummary refValue={view.artifactRef} />
    </section>
  );
}

function useArtifactRows(
  refValue: ToolResultArtifactRef | undefined,
  offset: number,
  limit: number,
  enabled: boolean
) {
  const me = useMeQuery(Boolean(enabled && refValue?.objectReferenceId));
  const query = useToolResultArtifactQuery(
    me.data?.tenantId,
    refValue?.objectReferenceId,
    offset,
    limit,
    Boolean(enabled && me.data?.tenantId && refValue?.objectReferenceId)
  );
  const content = asJsonRecord(query.data?.content);
  const rows =
    content?.kind === "json_rows" && Array.isArray(content.rows)
      ? content.rows.filter(isJsonRecord)
      : undefined;
  const totalRows = typeof content?.total_rows === "number" ? content.total_rows : undefined;
  return {
    rows,
    totalPages: totalRows ? Math.ceil(totalRows / limit) : undefined,
    hasNextPage: totalRows === undefined ? false : offset + limit < totalRows,
    isLoading: me.isLoading || query.isLoading,
    error: me.error || query.error
  };
}

function useArtifactValue(refValue: ToolResultArtifactRef | undefined, enabled: boolean) {
  const me = useMeQuery(Boolean(enabled && refValue?.objectReferenceId));
  const query = useToolResultArtifactQuery(
    me.data?.tenantId,
    refValue?.objectReferenceId,
    0,
    50,
    Boolean(enabled && me.data?.tenantId && refValue?.objectReferenceId)
  );
  const content = asJsonRecord(query.data?.content);
  let value: JsonValue | undefined;
  if (content?.kind === "json_value") {
    value = content.value;
  } else if (content?.kind === "json_rows") {
    value = content;
  } else if (content) {
    value = content;
  }
  return {
    value,
    valueAsRecord: asJsonRecord(value),
    error: me.error || query.error
  };
}

function ToolResultTitle({ title, fallbackKey }: { title?: string; fallbackKey: I18nKey }) {
  const { t } = useI18n();
  return <h4>{title || t(fallbackKey)}</h4>;
}

function ArtifactRefSummary({ refValue }: { refValue: ToolResultArtifactRef }) {
  const { t } = useI18n();
  return (
    <dl className="tool-result-ref">
      <dt>{t("run.toolResult.refId")}</dt>
      <dd>{refValue.artifactId}</dd>
      <dt>{t("run.toolResult.refType")}</dt>
      <dd>{refValue.contentType}</dd>
      <dt>{t("run.toolResult.refHash")}</dt>
      <dd>{refValue.contentHash}</dd>
      <dt>{t("run.toolResult.refBytes")}</dt>
      <dd>{refValue.sizeBytes}</dd>
    </dl>
  );
}

function JsonPreview({ value }: { value: JsonValue }) {
  return <pre className="tool-result-json">{JSON.stringify(value, null, 2)}</pre>;
}

function formatCell(value: JsonValue | undefined): string {
  if (value === undefined || value === null) {
    return "";
  }
  if (typeof value === "string") {
    return value;
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  return JSON.stringify(value);
}

function virtualRowWindow(rowCount: number, scrollTop: number, enabled: boolean) {
  if (!enabled || rowCount <= 50) {
    return {
      enabled: false,
      startIndex: 0,
      endIndex: rowCount,
      topSpacerHeight: 0,
      bottomSpacerHeight: 0
    };
  }
  const visibleCount = Math.ceil(VIRTUAL_VIEWPORT_HEIGHT / VIRTUAL_ROW_HEIGHT);
  const startIndex = Math.max(
    0,
    Math.floor(scrollTop / VIRTUAL_ROW_HEIGHT) - VIRTUAL_OVERSCAN_ROWS
  );
  const endIndex = Math.min(rowCount, startIndex + visibleCount + VIRTUAL_OVERSCAN_ROWS * 2);
  return {
    enabled: true,
    startIndex,
    endIndex,
    topSpacerHeight: startIndex * VIRTUAL_ROW_HEIGHT,
    bottomSpacerHeight: Math.max(0, (rowCount - endIndex) * VIRTUAL_ROW_HEIGHT)
  };
}

function asJsonRecord(value: JsonValue | undefined): JsonRecord | undefined {
  return isJsonRecord(value) ? value : undefined;
}

function isJsonRecord(value: JsonValue | undefined): value is JsonRecord {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}
